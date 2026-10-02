use std::{
    io::Read,
    process::{Command, Stdio},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
struct Server {
    child: std::process::Child,
    base: String,
}
impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
impl Server {
    async fn start(filter: Option<&str>) -> Self {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        drop(listener);
        let mut command = Command::new(env!("CARGO_BIN_EXE_sms-discord-relay"));
        command
            .env("BIND_ADDR", address.to_string())
            .env("SMS_GATEWAY_SIGNING_KEY", "CONFIG_KEY_SECRET")
            .env_remove("DISCORD_WEBHOOK_URL")
            .env_remove("RUST_LOG")
            .stderr(Stdio::piped())
            .stdout(Stdio::piped());
        if let Some(filter) = filter {
            command.env("RUST_LOG", filter);
        }
        let mut server = Self {
            child: command.spawn().unwrap(),
            base: format!("http://{address}"),
        };
        let client = client();
        let start = tokio::time::Instant::now();
        loop {
            if client
                .get(format!("{}/healthz", server.base))
                .send()
                .await
                .is_ok()
            {
                break;
            }
            assert!(server.child.try_wait().unwrap().is_none());
            assert!(start.elapsed() < Duration::from_secs(3));
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        server
    }
    fn logs(mut self) -> String {
        self.child.kill().unwrap();
        self.child.wait().unwrap();
        let mut stdout = String::new();
        self.child
            .stdout
            .take()
            .unwrap()
            .read_to_string(&mut stdout)
            .unwrap();
        assert!(stdout.is_empty());
        let mut logs = String::new();
        self.child
            .stderr
            .take()
            .unwrap()
            .read_to_string(&mut logs)
            .unwrap();
        logs
    }
}
fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(2))
        .build()
        .unwrap()
}
fn signed(client: &reqwest::Client, url: String, body: &str) -> reqwest::RequestBuilder {
    use hmac::{Hmac, Mac};
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
        .to_string();
    let mut mac = Hmac::<sha2::Sha256>::new_from_slice(b"CONFIG_KEY_SECRET").unwrap();
    mac.update(body.as_bytes());
    mac.update(timestamp.as_bytes());
    client
        .post(url)
        .header("X-Timestamp", timestamp)
        .header("X-Signature", hex::encode(mac.finalize().into_bytes()))
        .body(body.to_owned())
}
const SMS: &str = r#"{"event":"sms:received","payload":{"sender":"SENDER_SECRET","recipient":"RECIPIENT_SECRET","message":"BODY_SECRET","receivedAt":"TIME_SECRET"}}"#;
#[tokio::test]
async fn executable_logs_safe_request_outcomes_without_health_info_spam() {
    for filter in [
        None,
        Some("trace"),
        Some("reqwest=trace,sms_discord_relay=trace"),
        Some("off"),
    ] {
        let server = Server::start(filter).await;
        let client = client();
        assert_eq!(
            signed(
                &client,
                format!("{}/transform?QUERY_SECRET", server.base),
                SMS
            )
            .send()
            .await
            .unwrap()
            .status(),
            200
        );
        assert_eq!(
            client
                .post(format!("{}/webhook?QUERY_SECRET", server.base))
                .header("X-Timestamp", "HEADER_TIME_SECRET")
                .header("X-Signature", "SIGNATURE_SECRET")
                .body(SMS)
                .send()
                .await
                .unwrap()
                .status(),
            401
        );
        assert_eq!(
            client
                .get(format!("{}/PATH_SECRET?QUERY_SECRET", server.base))
                .send()
                .await
                .unwrap()
                .status(),
            404
        );
        let logs = server.logs();
        if filter == Some("off") {
            assert!(logs.is_empty(), "{logs}");
            continue;
        }
        assert!(logs.contains("event=\"request\""), "{logs}");
        assert!(logs.contains("route=\"transform\""), "{logs}");
        assert!(logs.contains("status=200"), "{logs}");
        assert!(logs.contains("outcome=\"unauthorized\""), "{logs}");
        assert!(logs.contains("status=401"), "{logs}");
        assert!(logs.contains("route=\"other\""), "{logs}");
        assert!(logs.contains("elapsed_ms="), "{logs}");
        assert!(!logs.contains("SECRET"), "{logs}");
        assert!(!logs.contains('\u{1b}'));
        if filter.is_none() {
            assert!(!logs.contains("health"), "{logs}");
        } else {
            assert!(logs.contains("DEBUG"), "{logs}");
            assert!(logs.contains("route=\"health\""), "{logs}");
        }
    }
}
