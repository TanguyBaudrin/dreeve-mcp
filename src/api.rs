//! Thin HTTP client for the Dreeve v1 API.
//!
//! Every endpoint is authenticated with a bearer token and returns JSON.
//! Errors share a stable `{ "error": code, "message": text }` shape, which
//! [`ApiError`] unwraps so the human readable message reaches the MCP caller.

use std::{fmt, time::Duration};

use reqwest::{Client, Method, RequestBuilder, Response, StatusCode};
use serde_json::Value;

/// Fallback matching the stock Dreeve compose stack (`APP_URL`).
pub const DEFAULT_BASE_URL: &str = "http://localhost:8080";

/// A Dreeve API failure. Its display form is the API's human readable message.
#[derive(Debug)]
pub struct ApiError(String);

impl fmt::Display for ApiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ApiError {}

#[derive(Clone)]
pub struct DreeveApi {
    http: Client,
    base_url: String,
    api_key: String,
}

impl DreeveApi {
    pub fn new(base_url: String, api_key: String) -> Self {
        let http = Client::builder()
            .timeout(Duration::from_secs(60))
            .user_agent(concat!("dreeve-mcp/", env!("CARGO_PKG_VERSION")))
            .build()
            .expect("reqwest client");
        Self {
            http,
            base_url: base_url.trim_end_matches('/').to_string(),
            api_key,
        }
    }

    /// `GET /api/v1/status` — cheapest way to check URL and key.
    pub async fn status(&self) -> Result<Value, ApiError> {
        self.get("/api/v1/status", &[]).await
    }

    pub async fn get(&self, path: &str, query: &[(&str, String)]) -> Result<Value, ApiError> {
        let resp = self
            .send(self.request(Method::GET, path).query(query))
            .await?;
        Self::interpret(resp).await
    }

    /// GET an endpoint that answers with a non-JSON body (the GPX export).
    pub async fn get_text(&self, path: &str) -> Result<String, ApiError> {
        let resp = self.send(self.request(Method::GET, path)).await?;
        let status = resp.status();
        if !status.is_success() {
            return Err(Self::error_from(status, resp.text().await.unwrap_or_default()).await);
        }
        resp.text()
            .await
            .map_err(|e| ApiError(format!("cannot read response body: {e}")))
    }

    pub async fn patch(&self, path: &str, body: &Value) -> Result<Value, ApiError> {
        let resp = self
            .send(
                self.request(Method::PATCH, path)
                    .header(reqwest::header::CONTENT_TYPE, "application/json")
                    .json(body),
            )
            .await?;
        Self::interpret(resp).await
    }

    /// POST a JSON body.
    pub async fn post_json(&self, path: &str, body: &Value) -> Result<Value, ApiError> {
        let resp = self
            .send(
                self.request(Method::POST, path)
                    .header(reqwest::header::CONTENT_TYPE, "application/json")
                    .json(body),
            )
            .await?;
        Self::interpret(resp).await
    }

    /// DELETE an endpoint answering `204 No Content`.
    pub async fn delete(&self, path: &str) -> Result<(), ApiError> {
        let resp = self.send(self.request(Method::DELETE, path)).await?;
        Self::interpret(resp).await.map(|_| ())
    }

    /// POST `multipart/form-data` with a local file in the `file` part.
    pub async fn post_file(&self, path: &str, file_path: &str) -> Result<Value, ApiError> {
        let bytes = tokio::fs::read(file_path).await.map_err(|e| {
            ApiError(format!(
                "cannot read {file_path}: {e} — the file must be readable from where dreeve-mcp runs (e.g. a mounted volume)"
            ))
        })?;
        let file_name = std::path::Path::new(file_path)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("upload")
            .to_string();
        let form = reqwest::multipart::Form::new().part(
            "file",
            reqwest::multipart::Part::bytes(bytes).file_name(file_name),
        );
        let resp = self
            .send(self.request(Method::POST, path).multipart(form))
            .await?;
        Self::interpret(resp).await
    }

    fn request(&self, method: Method, path: &str) -> RequestBuilder {
        self.http
            .request(method, format!("{}{}", self.base_url, path))
            .bearer_auth(&self.api_key)
    }

    async fn send(&self, rb: RequestBuilder) -> Result<Response, ApiError> {
        rb.send()
            .await
            .map_err(|e| ApiError(format!("request failed: {e}")))
    }

    /// Turn a response into JSON, mapping non-2xx to an [`ApiError`] built
    /// from the API's `{ error, message }` body.
    async fn interpret(resp: Response) -> Result<Value, ApiError> {
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(Self::error_from(status, body).await);
        }
        if status == StatusCode::NO_CONTENT {
            return Ok(Value::Null);
        }
        let body = resp
            .text()
            .await
            .map_err(|e| ApiError(format!("cannot read response body: {e}")))?;
        if body.trim().is_empty() {
            return Ok(Value::Null);
        }
        match serde_json::from_str(&body) {
            Ok(value) => Ok(value),
            // Non-JSON success body: hand it back as a plain string.
            Err(_) => Ok(Value::String(body)),
        }
    }

    async fn error_from(status: StatusCode, body: String) -> ApiError {
        ApiError(Self::interpret_error(status, body))
    }

    /// Extract the API's human readable message from an error body.
    fn interpret_error(status: StatusCode, body: String) -> String {
        let api_message = serde_json::from_str::<Value>(&body).ok().and_then(|v| {
            v.get("message")
                .and_then(|m| m.as_str())
                .map(str::to_string)
        });
        match api_message {
            Some(message) => message,
            None if body.trim().is_empty() => format!("HTTP {status}"),
            None => format!("HTTP {status}: {}", body.trim()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_body_is_unwrapped_to_its_message() {
        let body = r#"{"error":"invalid_token","message":"A valid API token is required."}"#;
        let msg = DreeveApi::interpret_error(StatusCode::UNAUTHORIZED, body.into());
        assert_eq!(msg, "A valid API token is required.");
    }

    #[test]
    fn error_without_json_body_falls_back_to_status() {
        let msg = DreeveApi::interpret_error(StatusCode::INTERNAL_SERVER_ERROR, String::new());
        assert_eq!(msg, "HTTP 500 Internal Server Error");
    }

    #[test]
    fn base_url_trailing_slash_is_stripped() {
        let api = DreeveApi::new("https://dreeve.example.com/".into(), "k".into());
        assert_eq!(api.base_url, "https://dreeve.example.com");
    }
}
