//! OAuth-aware MCP HTTP requests. Keep the manager alive for per-request expiry
//! checks and retry only requests explicitly rejected with HTTP 401, once.

use futures::{stream::BoxStream, StreamExt};
use http::{HeaderName, HeaderValue};
use rmcp::model::{ClientJsonRpcMessage, ServerJsonRpcMessage};
use rmcp::transport::auth::{
    AuthClient, AuthError, AuthorizationManager, OAuthHttpClient, OAuthHttpClientError,
    OAuthHttpClientFuture, OAuthHttpRequest,
};
use rmcp::transport::streamable_http_client::{
    AuthRequiredError, SseError, StreamableHttpClient, StreamableHttpError,
    StreamableHttpPostResponse,
};
use sse_stream::{Sse, SseStream};
use std::collections::HashMap;
use std::future::Future;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// The SDK flattens token-request failures into AuthorizationRequired. Keep the
/// HTTP classification so an outage is not confused with revoked credentials.
pub(super) struct OAuthSession {
    pub manager: AuthorizationManager,
    pub transient_failure: Arc<AtomicBool>,
}

pub(super) fn is_auth_connect_error(err: &anyhow::Error) -> bool {
    if matches!(
        err.downcast_ref::<AuthError>(),
        Some(AuthError::AuthorizationRequired)
    ) {
        return true;
    }
    let Some(rmcp::service::ClientInitializeError::TransportError { error, .. }) =
        err.downcast_ref::<rmcp::service::ClientInitializeError>()
    else {
        return false;
    };
    match error.error.downcast_ref::<HttpError>() {
        Some(
            StreamableHttpError::Auth(AuthError::AuthorizationRequired)
            | StreamableHttpError::AuthRequired(_),
        ) => true,
        Some(StreamableHttpError::Client(err)) => {
            err.status() == Some(reqwest_mcp::StatusCode::UNAUTHORIZED)
        }
        Some(StreamableHttpError::UnexpectedServerResponse(msg)) => msg.starts_with("HTTP 401 "),
        _ => false,
    }
}

pub(super) struct OAuthRequestClient {
    client: reqwest_mcp::Client,
    pub transient_failure: Arc<AtomicBool>,
}

impl OAuthRequestClient {
    pub fn new() -> Result<Self, reqwest_mcp::Error> {
        Ok(Self {
            client: reqwest_mcp::Client::builder()
                .redirect(reqwest_mcp::redirect::Policy::none())
                .build()?,
            transient_failure: Arc::new(AtomicBool::new(false)),
        })
    }
}

impl OAuthHttpClient for OAuthRequestClient {
    fn execute(&self, request: OAuthHttpRequest) -> OAuthHttpClientFuture<'_> {
        Box::pin(async move {
            let result = async {
                let mut request_builder = self
                    .client
                    .request(
                        request.request.method().clone(),
                        request.request.uri().to_string(),
                    )
                    .headers(request.request.headers().clone())
                    .body(request.request.body().clone());
                request_builder = request_builder.timeout(
                    request
                        .timeout
                        .unwrap_or(std::time::Duration::from_secs(30)),
                );
                let response = request_builder
                    .send()
                    .await
                    .map_err(|err| OAuthHttpClientError::new(err.to_string()))?;
                let status = response.status();
                let mut builder = http::Response::builder()
                    .status(status)
                    .version(response.version());
                for (name, value) in response.headers() {
                    builder = builder.header(name, value);
                }
                let mut stream = response.bytes_stream();
                let mut body = Vec::new();
                while let Some(chunk) = stream.next().await {
                    let chunk = chunk.map_err(|err| OAuthHttpClientError::new(err.to_string()))?;
                    if body.len() + chunk.len() > 1024 * 1024 {
                        return Err(OAuthHttpClientError::new("OAuth response exceeds 1 MiB"));
                    }
                    body.extend_from_slice(&chunk);
                }
                if !status.is_success() {
                    let error = serde_json::from_slice::<serde_json::Value>(&body).ok();
                    let credential_rejection = status.is_client_error()
                        && matches!(
                            error
                                .as_ref()
                                .and_then(|value| value.get("error"))
                                .and_then(|error| error.as_str()),
                            Some("invalid_grant" | "invalid_client" | "unauthorized_client")
                        );
                    self.transient_failure
                        .store(!credential_rejection, Ordering::Relaxed);
                }
                builder
                    .body(body)
                    .map_err(|err| OAuthHttpClientError::new(err.to_string()))
            }
            .await;
            if result.is_err() {
                self.transient_failure.store(true, Ordering::Relaxed);
            }
            result
        })
    }
}

type HttpError = StreamableHttpError<reqwest_mcp::Error>;

#[cfg(test)]
mod tests;

#[derive(Clone)]
pub(super) struct RefreshingHttpClient {
    client: AuthClient<reqwest_mcp::Client>,
    refresh_lock: Option<PathBuf>,
    transient_failure: Arc<AtomicBool>,
}

pub(super) fn is_auth_service_error(err: &rmcp::service::ServiceError) -> bool {
    let rmcp::service::ServiceError::TransportSend(err) = err else {
        return false;
    };
    matches!(
        err.error.downcast_ref::<HttpError>(),
        Some(StreamableHttpError::Auth(AuthError::AuthorizationRequired))
    )
}

impl RefreshingHttpClient {
    pub fn new(session: OAuthSession) -> Result<Self, reqwest_mcp::Error> {
        // Preserve rmcp's no-redirect policy: arbitrary custom headers may
        // contain credentials and must never be replayed to redirect targets.
        let http_client = reqwest_mcp::Client::builder()
            .redirect(reqwest_mcp::redirect::Policy::none())
            .build()?;
        Ok(Self {
            client: AuthClient::new(http_client, session.manager),
            refresh_lock: None,
            transient_failure: session.transient_failure,
        })
    }

    pub fn with_refresh_lock(mut self, path: PathBuf) -> Self {
        self.refresh_lock = Some(path);
        self
    }

    async fn access_token(&self, rejected: Option<&str>) -> Result<String, HttpError> {
        // Once refresh begins, let its exchange + save finish under the lock
        // even when the originating request is cancelled mid-rotation.
        let client = self.clone();
        let rejected = rejected.map(str::to_owned);
        tokio::spawn(async move { client.access_token_inner(rejected.as_deref()).await })
            .await
            .map_err(|err| AuthError::InternalError(err.to_string()))?
    }

    async fn access_token_inner(&self, rejected: Option<&str>) -> Result<String, HttpError> {
        let _file_guard = match &self.refresh_lock {
            Some(path) => Some(super::credentials::lock_refresh(path).await?),
            None => None,
        };
        let manager = self.client.auth_manager.lock().await;
        self.transient_failure.store(false, Ordering::Relaxed);
        let current = manager
            .get_access_token()
            .await
            .map_err(|err| self.classify_refresh_error(err))?;
        if rejected == Some(current.as_str()) {
            manager
                .refresh_token()
                .await
                .map_err(|err| self.classify_refresh_error(err))?;
            return Ok(manager
                .get_access_token()
                .await
                .map_err(|err| self.classify_refresh_error(err))?);
        }
        Ok(current)
    }

    fn classify_refresh_error(&self, err: AuthError) -> AuthError {
        if self.transient_failure.load(Ordering::Relaxed) {
            return AuthError::InternalError(
                "MCP token refresh temporarily failed; retry later".into(),
            );
        }
        refresh_error(err)
    }

    async fn request<T, F, Fut>(&self, send: F) -> Result<T, HttpError>
    where
        F: Fn(String) -> Fut,
        Fut: Future<Output = Result<T, HttpError>>,
    {
        let token = self.access_token(None).await?;
        match send(token.clone()).await {
            Err(err) if is_unauthorized(&err) => {
                // A parallel request (or another process) may already have
                // replaced the rejected token. Do not rotate it twice.
                let token = self.access_token(Some(&token)).await?;
                match send(token).await {
                    Err(err) if is_unauthorized(&err) => {
                        Err(AuthError::AuthorizationRequired.into())
                    }
                    result => result,
                }
            }
            result => result,
        }
    }

    async fn post(
        &self,
        uri: &str,
        message: &ClientJsonRpcMessage,
        session_id: Option<&str>,
        headers: &HashMap<HeaderName, HeaderValue>,
        token: String,
    ) -> Result<StreamableHttpPostResponse, HttpError> {
        // Inspect the raw status before JSON-RPC decoding. The SDK's default
        // client only recognizes 401 with WWW-Authenticate and may otherwise
        // turn an unauthorized JSON-RPC body into an ordinary tool error.
        let mut request = self.client.http_client.post(uri).bearer_auth(token);
        for (name, value) in headers {
            if matches!(name.as_str(), "accept" | "mcp-session-id" | "last-event-id") {
                return Err(StreamableHttpError::ReservedHeaderConflict(
                    name.to_string(),
                ));
            }
            request = request.header(name, value);
        }
        request = request.header("accept", "application/json, text/event-stream");
        if let Some(session_id) = session_id {
            request = request.header("mcp-session-id", session_id);
        }
        let response = request
            .json(message)
            .send()
            .await
            .map_err(StreamableHttpError::Client)?;
        let status = response.status();
        if status == reqwest_mcp::StatusCode::UNAUTHORIZED {
            return Err(StreamableHttpError::AuthRequired(AuthRequiredError::new(
                response
                    .headers()
                    .get("www-authenticate")
                    .and_then(|h| h.to_str().ok())
                    .unwrap_or_default()
                    .to_owned(),
            )));
        }
        if status == reqwest_mcp::StatusCode::NOT_FOUND && session_id.is_some() {
            return Err(StreamableHttpError::SessionExpired);
        }
        if matches!(
            status,
            reqwest_mcp::StatusCode::ACCEPTED | reqwest_mcp::StatusCode::NO_CONTENT
        ) {
            return Ok(StreamableHttpPostResponse::Accepted);
        }
        let session = response
            .headers()
            .get("mcp-session-id")
            .and_then(|h| h.to_str().ok())
            .map(str::to_owned);
        let content_type = response
            .headers()
            .get("content-type")
            .and_then(|h| h.to_str().ok())
            .unwrap_or_default()
            .to_owned();
        if !status.is_success() {
            let body = response.text().await.map_err(StreamableHttpError::Client)?;
            // Preserve non-auth JSON-RPC errors exactly as the SDK does.
            if let Ok(message @ ServerJsonRpcMessage::Error(_)) = serde_json::from_str(&body) {
                return Ok(StreamableHttpPostResponse::Json(message, session));
            }
            return Err(StreamableHttpError::UnexpectedServerResponse(
                format!("HTTP {status}: {body}").into(),
            ));
        }
        if response.content_length() == Some(0)
            && !matches!(message, ClientJsonRpcMessage::Request(_))
        {
            return Ok(StreamableHttpPostResponse::Accepted);
        }
        if content_type.starts_with("text/event-stream") {
            return Ok(StreamableHttpPostResponse::Sse(
                SseStream::from_bytes_stream(response.bytes_stream()).boxed(),
                session,
            ));
        }
        if content_type.starts_with("application/json") {
            return Ok(match response.json().await {
                Ok(message) => StreamableHttpPostResponse::Json(message, session),
                Err(_) => StreamableHttpPostResponse::Accepted,
            });
        }
        Err(StreamableHttpError::UnexpectedContentType(Some(
            content_type,
        )))
    }
}

fn is_unauthorized(err: &HttpError) -> bool {
    matches!(err, StreamableHttpError::AuthRequired(_))
        || matches!(err, StreamableHttpError::Client(err) if err.status() == Some(reqwest_mcp::StatusCode::UNAUTHORIZED))
}

fn refresh_error(err: AuthError) -> AuthError {
    match err {
        AuthError::TokenRefreshFailed(_) | AuthError::AuthorizationRequired => {
            AuthError::AuthorizationRequired
        }
        err => err,
    }
}

impl StreamableHttpClient for RefreshingHttpClient {
    type Error = reqwest_mcp::Error;

    async fn post_message(
        &self,
        uri: Arc<str>,
        message: ClientJsonRpcMessage,
        session_id: Option<Arc<str>>,
        _auth_header: Option<String>,
        headers: HashMap<HeaderName, HeaderValue>,
    ) -> Result<StreamableHttpPostResponse, HttpError> {
        self.request(|token| self.post(&uri, &message, session_id.as_deref(), &headers, token))
            .await
    }

    async fn get_stream(
        &self,
        uri: Arc<str>,
        session_id: Arc<str>,
        last_event_id: Option<String>,
        _auth_header: Option<String>,
        headers: HashMap<HeaderName, HeaderValue>,
    ) -> Result<BoxStream<'static, Result<Sse, SseError>>, HttpError> {
        self.request(|token| {
            self.client.http_client.get_stream(
                uri.clone(),
                session_id.clone(),
                last_event_id.clone(),
                Some(token),
                headers.clone(),
            )
        })
        .await
    }

    async fn delete_session(
        &self,
        uri: Arc<str>,
        session_id: Arc<str>,
        _auth_header: Option<String>,
        headers: HashMap<HeaderName, HeaderValue>,
    ) -> Result<(), HttpError> {
        self.request(|token| {
            self.client.http_client.delete_session(
                uri.clone(),
                session_id.clone(),
                Some(token),
                headers.clone(),
            )
        })
        .await
    }
}
