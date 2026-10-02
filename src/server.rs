use crate::{Delivery, InputError, SigningKey, transform};
use axum::{
    Json, Router,
    body::Bytes,
    extract::{DefaultBodyLimit, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use reqwest::Url;
use std::{
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tracing::Instrument;

pub struct App {
    key: SigningKey,
    target: Option<DiscordTarget>,
    client: reqwest::Client,
    budget: Duration,
    slots: tokio::sync::Semaphore,
}
impl App {
    pub fn new(key: SigningKey, target: Option<DiscordTarget>) -> Result<Self, &'static str> {
        let client = reqwest::Client::builder()
            .https_only(true)
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .connect_timeout(Duration::from_secs(3))
            .timeout(Duration::from_secs(8))
            .build()
            .map_err(|_| "HTTP client initialization failed")?;
        Ok(Self {
            key,
            target,
            client,
            budget: Duration::from_secs(25),
            slots: tokio::sync::Semaphore::new(32),
        })
    }
    pub fn router(self) -> Router {
        let app = Arc::new(self);
        Router::new()
            .route("/healthz", get(|| async { "ok" }))
            .route("/transform", post(transform_http))
            .route("/webhook", post(webhook_http))
            .layer(DefaultBodyLimit::max(256 * 1024))
            .layer(axum::middleware::from_fn_with_state(app.clone(), bounded))
            .with_state(app)
    }
}
async fn bounded(
    State(app): State<Arc<App>>,
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> Response {
    let route = match request.uri().path() {
        "/healthz" => "health",
        "/transform" => "transform",
        "/webhook" => "webhook",
        _ => "other",
    };
    let span = if route == "health" {
        tracing::debug_span!("request", route)
    } else {
        tracing::info_span!("request", route)
    };
    async {
        let start = std::time::Instant::now();
        let (response, outcome) = if let Ok(_permit) = app.slots.try_acquire() {
            match tokio::time::timeout(app.budget, next.run(request)).await {
                Ok(response) => {
                    let outcome = match response.status() {
                        StatusCode::UNAUTHORIZED => "unauthorized",
                        StatusCode::PAYLOAD_TOO_LARGE => "limit",
                        StatusCode::BAD_REQUEST => "invalid",
                        StatusCode::BAD_GATEWAY => "upstream_failure",
                        StatusCode::SERVICE_UNAVAILABLE => "unavailable",
                        StatusCode::NOT_FOUND => "not_found",
                        StatusCode::METHOD_NOT_ALLOWED => "method_not_allowed",
                        _ => "complete",
                    };
                    (response, outcome)
                }
                Err(_) => (StatusCode::SERVICE_UNAVAILABLE.into_response(), "timeout"),
            }
        } else {
            (StatusCode::SERVICE_UNAVAILABLE.into_response(), "overload")
        };
        let status = response.status().as_u16();
        let elapsed_ms = start.elapsed().as_millis() as u64;
        if route == "health" {
            tracing::debug!(event = "request", route, status, elapsed_ms, outcome);
        } else {
            tracing::info!(event = "request", route, status, elapsed_ms, outcome);
        }
        response
    }
    .instrument(span)
    .await
}
fn authenticate(app: &App, headers: &HeaderMap, body: &[u8]) -> Result<(), StatusCode> {
    let unauthorized = StatusCode::UNAUTHORIZED;
    let header = |name: &str| {
        let mut values = headers.get_all(name).iter();
        let value = values
            .next()
            .ok_or(unauthorized)?
            .to_str()
            .map_err(|_| unauthorized)?;
        if values.next().is_some() {
            return Err(unauthorized);
        }
        Ok(value)
    };
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| unauthorized)?
        .as_secs();
    app.key
        .verify(body, header("X-Timestamp")?, header("X-Signature")?, now)
        .map_err(|_| unauthorized)
}
fn parse(app: &App, headers: &HeaderMap, body: &[u8]) -> Result<Delivery, StatusCode> {
    authenticate(app, headers, body)?;
    transform(body).map_err(|error| match error {
        InputError::Invalid => StatusCode::BAD_REQUEST,
        InputError::TooMany => StatusCode::PAYLOAD_TOO_LARGE,
    })
}
async fn transform_http(State(app): State<Arc<App>>, headers: HeaderMap, body: Bytes) -> Response {
    match parse(&app, &headers, &body) {
        Ok(Delivery::Single(payload)) => Json(payload).into_response(),
        Ok(Delivery::Batch(payloads)) => Json(payloads).into_response(),
        Ok(Delivery::Ignored) => StatusCode::NO_CONTENT.into_response(),
        Err(status) => status.into_response(),
    }
}
enum ForwardReason {
    Accepted,
    UpstreamStatus,
    Transport,
}
impl ForwardReason {
    fn label(&self) -> &'static str {
        match self {
            Self::Accepted => "accepted",
            Self::UpstreamStatus => "upstream_status",
            Self::Transport => "transport",
        }
    }
}
async fn webhook_http(State(app): State<Arc<App>>, headers: HeaderMap, body: Bytes) -> Response {
    let payloads = match parse(&app, &headers, &body) {
        Ok(Delivery::Single(payload)) => vec![payload],
        Ok(Delivery::Batch(payloads)) => payloads,
        Ok(Delivery::Ignored) => return StatusCode::NO_CONTENT.into_response(),
        Err(status) => return status.into_response(),
    };
    let Some(target) = &app.target else {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    };
    let total = payloads.len();
    for (index, payload) in payloads.into_iter().enumerate() {
        match app
            .client
            .post(target.0.clone())
            .json(&payload)
            .send()
            .await
        {
            Ok(response) => {
                let status = response.status().as_u16();
                if response.status().is_success() {
                    tracing::info!(
                        event = "forward",
                        status,
                        total,
                        completed = index + 1,
                        reason = ForwardReason::Accepted.label()
                    );
                } else {
                    tracing::warn!(
                        event = "forward",
                        status,
                        total,
                        completed = index,
                        reason = ForwardReason::UpstreamStatus.label()
                    );
                    return StatusCode::BAD_GATEWAY.into_response();
                }
            }
            Err(_) => {
                tracing::warn!(
                    event = "forward",
                    total,
                    completed = index,
                    reason = ForwardReason::Transport.label()
                );
                return StatusCode::BAD_GATEWAY.into_response();
            }
        }
    }
    StatusCode::NO_CONTENT.into_response()
}

pub struct DiscordTarget(Url);
impl DiscordTarget {
    pub fn parse(value: &str) -> Result<Self, &'static str> {
        let invalid = "invalid DISCORD_WEBHOOK_URL";
        let mut url = Url::parse(value).map_err(|_| invalid)?;
        let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
        let parts: Vec<_> = url.path().split('/').collect();
        if url.scheme() != "https"
            || url.host_str() != Some("discord.com")
            || !url.username().is_empty()
            || url.password().is_some()
            || url.port_or_known_default() != Some(443)
            || url.fragment().is_some()
            || parts.len() != 5
            || parts[1] != "api"
            || parts[2] != "webhooks"
            || !digits(parts[3])
            || parts[4].is_empty()
            || !parts[4]
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
        {
            return Err(invalid);
        }
        let mut seen_thread = false;
        for (key, value) in url.query_pairs() {
            if key != "thread_id" || seen_thread || !digits(&value) {
                return Err(invalid);
            }
            seen_thread = true;
        }
        url.query_pairs_mut().append_pair("wait", "true");
        Ok(Self(url))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tracing::instrument::WithSubscriber;
    #[tokio::test]
    async fn accepts_exact_body_boundary_and_rejects_chunked_overflow() {
        let relay = serve(
            App::new(SigningKey::new("test-key").unwrap(), None)
                .unwrap()
                .router(),
        )
        .await;
        let mut body = SMS.to_owned();
        body.push_str(&" ".repeat(256 * 1024 - body.len()));
        let response = signed(
            &reqwest::Client::new(),
            format!("{}/transform", relay.base),
            &body,
        )
        .send()
        .await
        .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let address = relay.base.strip_prefix("http://").unwrap().to_owned();
        let response=tokio::task::spawn_blocking(move || {
            use std::io::{Read,Write};
            let mut stream=std::net::TcpStream::connect(&address).unwrap();
            stream.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
            let data="x".repeat(256*1024+1);
            write!(stream,"POST /transform HTTP/1.1\r\nHost: {address}\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n{:x}\r\n{}\r\n0\r\n\r\n",data.len(),data).unwrap();
            let mut response=String::new(); stream.read_to_string(&mut response).unwrap(); response
        }).await.unwrap();
        assert!(response.starts_with("HTTP/1.1 413"), "{response}");
    }
    #[tokio::test]
    async fn repeated_valid_requests_can_duplicate_without_deduplication() {
        let hits = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let h = hits.clone();
        let discord = serve(Router::new().route(
            "/",
            post(move || {
                let h = h.clone();
                async move {
                    h.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    StatusCode::OK
                }
            }),
        ))
        .await;
        let relay = serve(mock_app(&discord.base).router()).await;
        let client = reqwest::Client::new();
        for _ in 0..2 {
            assert_eq!(
                signed(&client, format!("{}/webhook", relay.base), SMS)
                    .send()
                    .await
                    .unwrap()
                    .status(),
                StatusCode::NO_CONTENT
            );
        }
        assert_eq!(hits.load(std::sync::atomic::Ordering::SeqCst), 2);
    }
    // Regression coverage of failure branches exercised by the forwarding tracer bullet.
    #[tokio::test]
    async fn upstream_failures_never_acknowledged_or_leaked() {
        for status in [
            StatusCode::BAD_REQUEST,
            StatusCode::TOO_MANY_REQUESTS,
            StatusCode::INTERNAL_SERVER_ERROR,
            StatusCode::TEMPORARY_REDIRECT,
        ] {
            let hits = Arc::new(std::sync::atomic::AtomicUsize::new(0));
            let h = hits.clone();
            let discord = serve(Router::new().route(
                "/",
                post(move || {
                    let h = h.clone();
                    async move {
                        h.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                        (
                            status,
                            [("location", "http://127.0.0.1:1/secret-token")],
                            "private SMS and signing key",
                        )
                    }
                }),
            ))
            .await;
            let relay = serve(mock_app(&discord.base).router()).await;
            let response = signed(
                &reqwest::Client::new(),
                format!("{}/webhook", relay.base),
                SMS,
            )
            .send()
            .await
            .unwrap();
            assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
            assert!(response.text().await.unwrap().is_empty());
            assert_eq!(hits.load(std::sync::atomic::Ordering::SeqCst), 1);
        }
    }
    #[tokio::test]
    async fn timeout_or_connection_failure_is_retryable() {
        let discord = serve(Router::new().route(
            "/",
            post(|| async {
                tokio::time::sleep(Duration::from_millis(150)).await;
                StatusCode::OK
            }),
        ))
        .await;
        let mut app = mock_app(&discord.base);
        app.client = reqwest::Client::builder()
            .no_proxy()
            .timeout(Duration::from_millis(30))
            .build()
            .unwrap();
        let relay = serve(app.router()).await;
        let response = signed(
            &reqwest::Client::new(),
            format!("{}/webhook", relay.base),
            SMS,
        )
        .send()
        .await
        .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
        drop(discord);
        let relay = serve(mock_app("http://127.0.0.1:0").router()).await;
        let response = signed(
            &reqwest::Client::new(),
            format!("{}/webhook", relay.base),
            SMS,
        )
        .send()
        .await
        .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
    }
    #[tokio::test]
    async fn partial_batch_stops_after_first_failure() {
        let hits = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let h = hits.clone();
        let discord = serve(Router::new().route(
            "/",
            post(move || {
                let h = h.clone();
                async move {
                    if h.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0 {
                        StatusCode::OK
                    } else {
                        StatusCode::TOO_MANY_REQUESTS
                    }
                }
            }),
        ))
        .await;
        let relay = serve(mock_app(&discord.base).router()).await;
        let sms: serde_json::Value = serde_json::from_str(SMS).unwrap();
        let batch=serde_json::json!({"event":"sms:batch:received","payload":{"messages":vec![sms["payload"].clone();3]}}).to_string();
        let response = signed(
            &reqwest::Client::new(),
            format!("{}/webhook", relay.base),
            &batch,
        )
        .send()
        .await
        .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
        assert_eq!(hits.load(std::sync::atomic::Ordering::SeqCst), 2);
    }
    #[tokio::test]
    async fn authenticated_batches_and_ignored_events_over_http() {
        let relay = serve(
            App::new(SigningKey::new("test-key").unwrap(), None)
                .unwrap()
                .router(),
        )
        .await;
        let client = reqwest::Client::new();
        let sms: serde_json::Value = serde_json::from_str(SMS).unwrap();
        for (n, status) in [
            (0, StatusCode::OK),
            (100, StatusCode::OK),
            (101, StatusCode::PAYLOAD_TOO_LARGE),
        ] {
            let batch=serde_json::json!({"event":"sms:batch:received","payload":{"messages":vec![sms["payload"].clone();n]}}).to_string();
            let response = signed(&client, format!("{}/transform", relay.base), &batch)
                .send()
                .await
                .unwrap();
            assert_eq!(response.status(), status);
            if status == StatusCode::OK {
                let json: serde_json::Value = response.json().await.unwrap();
                assert_eq!(json.as_array().unwrap().len(), n);
            }
        }
        for route in ["transform", "webhook"] {
            let url = format!("{}/{route}", relay.base);
            assert_eq!(
                signed(&client, url.clone(), r#"{"event":"sms:sent"}"#)
                    .send()
                    .await
                    .unwrap()
                    .status(),
                StatusCode::NO_CONTENT
            );
            assert_eq!(
                signed(&client, url.clone(), "not json")
                    .send()
                    .await
                    .unwrap()
                    .status(),
                StatusCode::BAD_REQUEST
            );
            assert_eq!(
                signed(
                    &client,
                    url.clone(),
                    r#"{"event":"sms:received","payload":{"message":null}}"#
                )
                .send()
                .await
                .unwrap()
                .status(),
                StatusCode::BAD_REQUEST
            );
        }
        assert_eq!(
            signed(&client, format!("{}/webhook", relay.base), SMS)
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::SERVICE_UNAVAILABLE
        );
    }
    #[tokio::test]
    async fn bad_or_ambiguous_auth_never_reaches_discord() {
        let hits = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let h = hits.clone();
        let discord = serve(Router::new().route(
            "/",
            post(move || {
                let h = h.clone();
                async move {
                    h.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    StatusCode::OK
                }
            }),
        ))
        .await;
        let relay = serve(mock_app(&discord.base).router()).await;
        let client = reqwest::Client::new();
        for route in ["transform", "webhook"] {
            let url = format!("{}/{route}", relay.base);
            let requests = [
                client.post(url.clone()).body(SMS),
                client
                    .post(url.clone())
                    .header("X-Timestamp", "1")
                    .header("X-Signature", "00".repeat(32))
                    .body(SMS),
                signed(&client, url.clone(), SMS).body("tampered body"),
                signed(&client, url.clone(), SMS).header("X-Timestamp", "duplicate"),
                signed(&client, url.clone(), SMS).header("X-Signature", "duplicate"),
            ];
            for request in requests {
                let response = request.send().await.unwrap();
                assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
                assert!(response.text().await.unwrap().is_empty());
            }
        }
        assert_eq!(hits.load(std::sync::atomic::Ordering::SeqCst), 0);
    }
    #[tokio::test]
    async fn rejects_overload_without_queuing() {
        let (tx, mut rx) = tokio::sync::mpsc::channel(1);
        let discord = serve(Router::new().route(
            "/",
            post(move || {
                let tx = tx.clone();
                async move {
                    tx.send(()).await.unwrap();
                    tokio::time::sleep(Duration::from_millis(150)).await;
                    StatusCode::OK
                }
            }),
        ))
        .await;
        let mut app = mock_app(&discord.base);
        app.slots = tokio::sync::Semaphore::new(1);
        let relay = serve(app.router()).await;
        let client = reqwest::Client::new();
        let first = signed(&client, format!("{}/webhook", relay.base), SMS);
        let first = tokio::spawn(async move { first.send().await.unwrap() });
        tokio::time::timeout(Duration::from_secs(2), rx.recv())
            .await
            .unwrap()
            .unwrap();
        let second = signed(&client, format!("{}/webhook", relay.base), SMS)
            .send()
            .await
            .unwrap();
        assert_eq!(second.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(first.await.unwrap().status(), StatusCode::NO_CONTENT);
    }

    #[tokio::test]
    async fn shared_batch_deadline_cancels_instead_of_per_message_budget() {
        let discord = serve(Router::new().route(
            "/",
            post(|| async {
                tokio::time::sleep(Duration::from_millis(100)).await;
                StatusCode::OK
            }),
        ))
        .await;
        let mut app = mock_app(&discord.base);
        app.budget = Duration::from_millis(150);
        let relay = serve(app.router()).await;
        let sms: serde_json::Value = serde_json::from_str(SMS).unwrap();
        let batch=serde_json::json!({"event":"sms:batch:received","payload":{"messages":vec![sms["payload"].clone();5]}}).to_string();
        let start = tokio::time::Instant::now();
        let response = signed(
            &reqwest::Client::new(),
            format!("{}/webhook", relay.base),
            &batch,
        )
        .send()
        .await
        .unwrap();
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert!(start.elapsed() < Duration::from_millis(450));
    }

    #[tokio::test]
    async fn caps_raw_body_before_transform() {
        let relay = serve(
            App::new(SigningKey::new("test-key").unwrap(), None)
                .unwrap()
                .router(),
        )
        .await;
        let body = " ".repeat(256 * 1024 + 1);
        for route in ["transform", "webhook"] {
            let response = signed(
                &reqwest::Client::new(),
                format!("{}/{route}", relay.base),
                &body,
            )
            .send()
            .await
            .unwrap();
            assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
        }
    }

    #[derive(Clone, Default)]
    struct LogCapture(Arc<std::sync::Mutex<Vec<u8>>>);
    impl std::io::Write for LogCapture {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn forwarding_logs_safe_counts_and_upstream_status() {
        for status in [StatusCode::OK, StatusCode::TOO_MANY_REQUESTS] {
            let capture = LogCapture::default();
            let sink = capture.clone();
            let subscriber = crate::logging::subscriber(Some("trace"), move || sink.clone());
            async {
                let discord = serve(Router::new().route(
                    "/",
                    post(move || async move { (status, "UPSTREAM_BODY_SECRET") }),
                ))
                .await;
                let client = reqwest::Client::new();
                let sms = SMS
                    .replace("hello", "BODY_SECRET")
                    .replace("+123", "SENDER_SECRET");
                let request = signed(
                    &client,
                    "http://localhost/webhook?QUERY_SECRET".into(),
                    &sms,
                )
                .build()
                .unwrap();
                let response = webhook_http(
                    State(Arc::new(mock_app(&discord.base))),
                    request.headers().clone(),
                    Bytes::from(sms),
                )
                .await;
                assert_eq!(
                    response.status(),
                    if status.is_success() {
                        StatusCode::NO_CONTENT
                    } else {
                        StatusCode::BAD_GATEWAY
                    }
                );
            }
            .with_subscriber(subscriber)
            .await;
            let logs = String::from_utf8(capture.0.lock().unwrap().clone()).unwrap();
            assert!(logs.contains("event=\"forward\""), "{logs}");
            assert!(
                logs.contains(&format!("status={}", status.as_u16())),
                "{logs}"
            );
            assert!(logs.contains("total=1"), "{logs}");
            assert!(
                logs.contains(if status.is_success() {
                    "completed=1"
                } else {
                    "completed=0"
                }),
                "{logs}"
            );
            assert!(
                logs.contains(if status.is_success() {
                    "reason=\"accepted\""
                } else {
                    "reason=\"upstream_status\""
                }),
                "{logs}"
            );
            assert!(!logs.contains("SECRET"), "{logs}");
            assert!(!logs.contains("http://"), "{logs}");
            assert!(!logs.contains("test-key"), "{logs}");
        }
    }

    #[tokio::test]
    async fn forwarding_logs_transport_failure_without_raw_error() {
        let capture = LogCapture::default();
        let sink = capture.clone();
        let subscriber = crate::logging::subscriber(Some("trace"), move || sink.clone());
        async {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let base = format!("http://{}/TOKEN_SECRET", listener.local_addr().unwrap());
            drop(listener);
            let client = reqwest::Client::new();
            let request = signed(&client, "http://localhost/webhook".into(), SMS)
                .build()
                .unwrap();
            let response = webhook_http(
                State(Arc::new(mock_app(&base))),
                request.headers().clone(),
                Bytes::from_static(SMS.as_bytes()),
            )
            .await;
            assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
        }
        .with_subscriber(subscriber)
        .await;
        let logs = String::from_utf8(capture.0.lock().unwrap().clone()).unwrap();
        assert!(logs.contains("reason=\"transport\""), "{logs}");
        assert!(logs.contains("completed=0"), "{logs}");
        assert!(!logs.contains("SECRET"), "{logs}");
        assert!(!logs.contains("http://"), "{logs}");
    }

    #[tokio::test]
    async fn forwards_to_local_discord_with_wait_and_safe_payload() {
        let (tx, mut rx) = tokio::sync::mpsc::channel(1);
        let discord = serve(Router::new().route(
            "/",
            post(
                move |uri: axum::http::Uri, Json(payload): Json<serde_json::Value>| {
                    let tx = tx.clone();
                    async move {
                        tx.send((uri, payload)).await.unwrap();
                        (StatusCode::OK, "{\"id\":\"1\"}")
                    }
                },
            ),
        ))
        .await;
        let relay = serve(mock_app(&discord.base).router()).await;
        let client = reqwest::Client::new();
        let response = signed(&client, format!("{}/webhook", relay.base), SMS)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
        let (uri, payload) = rx.recv().await.unwrap();
        assert_eq!(uri.query(), Some("wait=true"));
        assert!(
            payload["content"]
                .as_str()
                .unwrap()
                .contains("📱 **New SMS from +123**")
        );
        assert_eq!(payload["allowed_mentions"]["parse"], serde_json::json!([]));
    }

    fn mock_app(base: &str) -> App {
        let mut url = Url::parse(base).unwrap();
        url.query_pairs_mut().append_pair("wait", "true");
        let mut app = App::new(
            SigningKey::new("test-key").unwrap(),
            Some(DiscordTarget(url)),
        )
        .unwrap();
        app.client = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .timeout(Duration::from_secs(1))
            .build()
            .unwrap();
        app
    }
    const SMS: &str = r#"{"event":"sms:received","deviceId":"device","id":"delivery","webhookId":"hook","scheme":"https","payload":{"messageId":"message","phoneNumber":"legacy","sender":"+123","message":"hello","recipient":null,"simNumber":1,"receivedAt":"now"}}"#;
    struct TestServer {
        base: String,
        task: tokio::task::JoinHandle<()>,
    }
    impl Drop for TestServer {
        fn drop(&mut self) {
            self.task.abort();
        }
    }
    async fn serve(router: Router) -> TestServer {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let task = tokio::spawn(
            async move {
                axum::serve(listener, router).await.unwrap();
            }
            .with_current_subscriber(),
        );
        TestServer { base, task }
    }
    fn signed(client: &reqwest::Client, url: String, body: &str) -> reqwest::RequestBuilder {
        use hmac::{Hmac, Mac};
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs()
            .to_string();
        let mut mac = Hmac::<sha2::Sha256>::new_from_slice(b"test-key").unwrap();
        mac.update(body.as_bytes());
        mac.update(timestamp.as_bytes());
        client
            .post(url)
            .header("X-Timestamp", timestamp)
            .header("X-Signature", hex::encode(mac.finalize().into_bytes()))
            .body(body.to_owned())
    }
    #[tokio::test]
    async fn authenticated_transform_over_http() {
        let server = serve(
            App::new(SigningKey::new("test-key").unwrap(), None)
                .unwrap()
                .router(),
        )
        .await;
        let client = reqwest::Client::new();
        assert_eq!(
            client
                .get(format!("{}/healthz", server.base))
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::OK
        );
        let response = signed(&client, format!("{}/transform", server.base), SMS)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let json: serde_json::Value = response.json().await.unwrap();
        assert!(json["content"].as_str().unwrap().contains("hello"));
        assert_eq!(json["allowed_mentions"]["parse"], serde_json::json!([]));
        for route in ["transform", "webhook"] {
            let response = client
                .post(format!("{}/{route}", server.base))
                .body(SMS)
                .send()
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        }
    }

    #[test]
    fn restricts_discord_target_and_forces_wait() {
        let target =
            DiscordTarget::parse("https://discord.com/api/webhooks/123/token_X-y?thread_id=456")
                .unwrap();
        assert_eq!(
            target.0.as_str(),
            "https://discord.com/api/webhooks/123/token_X-y?thread_id=456&wait=true"
        );
        for bad in [
            "http://discord.com/api/webhooks/1/x",
            "https://evil.com/api/webhooks/1/x",
            "https://discord.com.evil.com/api/webhooks/1/x",
            "https://user:pass@discord.com/api/webhooks/1/x",
            "https://discord.com:444/api/webhooks/1/x",
            "https://discord.com/api/webhooks/x/y",
            "https://discord.com/api/webhooks/1/",
            "https://discord.com/api/webhooks/1/x/extra",
            "https://discord.com/api/webhooks/1/x#fragment",
            "https://discord.com/api/webhooks/1/x?wait=false",
            "https://discord.com/api/webhooks/1/x?thread_id=x",
            "https://discord.com/api/webhooks/1/x?thread_id=1&thread_id=2",
        ] {
            assert!(DiscordTarget::parse(bad).is_err(), "{bad}");
        }
    }
}
