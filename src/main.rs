//! dreeve-mcp — an MCP server exposing the self-hosted Dreeve HTTP API.
//!
//! Serves MCP over Streamable HTTP on `/mcp`.

mod api;
mod server;

use std::env;

use anyhow::{Context, bail};
use rmcp::transport::streamable_http_server::{
    StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
};
use tokio_util::sync::CancellationToken;
use tracing_subscriber::{EnvFilter, layer::SubscriberExt, util::SubscriberInitExt};

use crate::{api::DreeveApi, server::DreeveServer};

const DEFAULT_DREEVE_URL: &str = api::DEFAULT_BASE_URL;
const DEFAULT_BIND_ADDR: &str = "0.0.0.0:8080";
const MCP_PATH: &str = "/mcp";

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::registry()
        .with(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .with(tracing_subscriber::fmt::layer().with_target(false))
        .init();

    let base_url = env::var("DREEVE_API_URL")
        .unwrap_or_else(|_| DEFAULT_DREEVE_URL.to_string())
        .trim_end_matches('/')
        .to_string();

    let api_key = match env::var("DREEVE_API_KEY") {
        Ok(key) if !key.trim().is_empty() => key,
        _ => bail!(
            "DREEVE_API_KEY is not set.\n\
             Generate a key in Dreeve under Settings → Security → API access \
             and pass it as the DREEVE_API_KEY environment variable."
        ),
    };

    let bind_addr = env::var("MCP_BIND_ADDR").unwrap_or_else(|_| DEFAULT_BIND_ADDR.to_string());

    let api = DreeveApi::new(base_url.clone(), api_key);

    // One cheap startup probe so a wrong URL or key is visible in the logs.
    // Non-fatal: the instance may just not be up yet.
    match api.status().await {
        Ok(status) => tracing::info!(
            "connected to Dreeve {} (uploads enabled: {})",
            status["version"].as_str().unwrap_or("unknown"),
            status["canUpload"].as_bool().unwrap_or(false),
        ),
        Err(e) => tracing::warn!(
            "cannot reach Dreeve at {base_url} yet: {e} (tools will retry on each call)"
        ),
    }

    let shutdown = CancellationToken::new();
    let service = StreamableHttpService::new(
        move || Ok(DreeveServer::new(api.clone())),
        LocalSessionManager::default().into(),
        StreamableHttpServerConfig::default().with_cancellation_token(shutdown.child_token()),
    );

    let router = axum::Router::new().nest_service(MCP_PATH, service);
    let listener = tokio::net::TcpListener::bind(&bind_addr)
        .await
        .with_context(|| format!("cannot bind to {bind_addr}"))?;
    tracing::info!("MCP endpoint ready at http://{bind_addr}{MCP_PATH}");

    axum::serve(listener, router)
        .with_graceful_shutdown(shutdown_signal(shutdown))
        .await?;
    Ok(())
}

/// Resolves on Ctrl-C or SIGTERM (what `docker stop` sends), then cancels the
/// session manager so in-flight sessions end cleanly.
async fn shutdown_signal(token: CancellationToken) {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut sig) => {
                sig.recv().await;
            }
            Err(_) => std::future::pending::<()>().await,
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {}
        _ = terminate => {}
    }
    tracing::info!("shutting down");
    token.cancel();
}
