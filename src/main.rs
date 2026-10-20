//! dreeve-mcp — an MCP server exposing the self-hosted Dreeve HTTP API.
//!
//! Serves MCP over Streamable HTTP on `/mcp`.

mod api;
mod server;

use std::env;

use anyhow::Context;
use axum::{
    extract::Request,
    http::{HeaderMap, StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Response},
};
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

    let bind_addr = env::var("MCP_BIND_ADDR").unwrap_or_else(|_| DEFAULT_BIND_ADDR.to_string());

    // No API key lives in this process: every MCP client sends its own key in
    // the `Authorization: Bearer …` header of its requests, and it is
    // forwarded verbatim to the Dreeve API on each tool call.
    let api = DreeveApi::new(base_url);

    let shutdown = CancellationToken::new();
    let service = StreamableHttpService::new(
        move || Ok(DreeveServer::new(api.clone())),
        LocalSessionManager::default().into(),
        apply_allowed_hosts(
            StreamableHttpServerConfig::default().with_cancellation_token(shutdown.child_token()),
            env::var("MCP_ALLOWED_HOSTS").ok().as_deref(),
        ),
    );

    let router = axum::Router::new()
        .nest_service(MCP_PATH, service)
        .layer(axum::middleware::from_fn(require_bearer));
    let listener = tokio::net::TcpListener::bind(&bind_addr)
        .await
        .with_context(|| format!("cannot bind to {bind_addr}"))?;
    tracing::info!("MCP endpoint ready at http://{bind_addr}{MCP_PATH}");

    axum::serve(listener, router)
        .with_graceful_shutdown(shutdown_signal(shutdown))
        .await?;
    Ok(())
}

/// Rejects `POST`s without a usable `Authorization: Bearer …` header with
/// `401 Unauthorized` before the request reaches the MCP service, so an
/// unauthorized client cannot even open a session. `GET` (SSE) and `DELETE`
/// are left alone: no tool can be invoked without a `POST` anyway, and some
/// clients only attach headers to `POST`s.
async fn require_bearer(req: Request, next: Next) -> Response {
    if req.method() == axum::http::Method::POST && !has_bearer(req.headers()) {
        return (
            StatusCode::UNAUTHORIZED,
            [(header::WWW_AUTHENTICATE, "Bearer")],
            "Missing or invalid Authorization header. Send the Dreeve API key as \
             `Authorization: Bearer drv_…` on every request to the MCP endpoint.",
        )
            .into_response();
    }
    next.run(req).await
}

/// Widens the `Host`-header allow-list (DNS-rebinding protection) with the
/// comma-separated authorities in `MCP_ALLOWED_HOSTS` — `host` or `host:port`,
/// e.g. `dreeve.example.com,203.0.113.10:8081`. The loopback defaults always
/// stay; `*` (or an empty value) accepts any `Host`. When the variable is
/// unset, only the loopback defaults apply.
fn apply_allowed_hosts(
    config: StreamableHttpServerConfig,
    raw: Option<&str>,
) -> StreamableHttpServerConfig {
    let Some(raw) = raw else {
        return config; // unset: keep rmcp's loopback-only default
    };
    let raw = raw.trim();
    if raw.is_empty() || raw == "*" {
        return config.disable_allowed_hosts();
    }
    let mut hosts = vec![
        "localhost".to_string(),
        "127.0.0.1".to_string(),
        "::1".to_string(),
    ];
    hosts.extend(
        raw.split(',')
            .map(str::trim)
            .filter(|h| !h.is_empty())
            .map(String::from),
    );
    config.with_allowed_hosts(hosts)
}

/// `true` when `headers` carries a non-empty `Bearer` token, whatever its
/// case (`bearer`, `BEARER`, …).
fn has_bearer(headers: &HeaderMap) -> bool {
    headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| {
            let v = v.trim();
            v.split_once(' ').is_some_and(|(scheme, token)| {
                scheme.eq_ignore_ascii_case("bearer") && !token.trim().is_empty()
            })
        })
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

#[cfg(test)]
mod tests {
    use super::*;

    fn headers_with(value: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(header::AUTHORIZATION, value.parse().expect("header value"));
        headers
    }

    #[test]
    fn has_bearer_accepts_a_non_empty_bearer_token() {
        assert!(has_bearer(&headers_with("Bearer drv_abc123")));
        assert!(has_bearer(&headers_with("bearer drv_abc123")));
        assert!(has_bearer(&headers_with("BEARER   drv_abc123  ")));
    }

    #[test]
    fn has_bearer_rejects_missing_or_malformed_values() {
        assert!(!has_bearer(&HeaderMap::new()));
        assert!(!has_bearer(&headers_with("drv_abc123")));
        assert!(!has_bearer(&headers_with("Basic Zm9vOmJhcg==")));
        assert!(!has_bearer(&headers_with("Bearer ")));
        assert!(!has_bearer(&headers_with("Bearer")));
    }

    #[test]
    fn allowed_hosts_unset_keeps_loopback_defaults() {
        let config = apply_allowed_hosts(StreamableHttpServerConfig::default(), None);
        assert_eq!(config.allowed_hosts, vec!["localhost", "127.0.0.1", "::1"]);
    }

    #[test]
    fn allowed_hosts_appends_to_the_loopback_defaults() {
        let config = apply_allowed_hosts(
            StreamableHttpServerConfig::default(),
            Some("dreeve.example.com, 203.0.113.10:8081"),
        );
        for host in [
            "localhost",
            "::1",
            "dreeve.example.com",
            "203.0.113.10:8081",
        ] {
            assert!(config.allowed_hosts.iter().any(|h| h == host), "{host}");
        }
    }

    #[test]
    fn allowed_hosts_star_or_empty_accepts_any_host() {
        for raw in ["*", "", "   "] {
            let config = apply_allowed_hosts(StreamableHttpServerConfig::default(), Some(raw));
            assert!(config.allowed_hosts.is_empty(), "{raw:?}");
        }
    }
}
