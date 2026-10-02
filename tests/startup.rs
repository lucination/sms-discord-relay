use std::process::Command;

#[tokio::test]
async fn executable_serves_signed_transform_on_real_listener() {
    struct Child(std::process::Child);
    impl Drop for Child {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    drop(listener);
    let mut child = Child(
        Command::new(env!("CARGO_BIN_EXE_sms-discord-relay"))
            .env("SMS_GATEWAY_SIGNING_KEY", "test-key")
            .env_remove("DISCORD_WEBHOOK_URL")
            .env("BIND_ADDR", address.to_string())
            .spawn()
            .unwrap(),
    );
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(1))
        .build()
        .unwrap();
    let start = tokio::time::Instant::now();
    loop {
        if client
            .get(format!("http://{address}/healthz"))
            .send()
            .await
            .is_ok()
        {
            break;
        }
        assert!(
            child.0.try_wait().unwrap().is_none(),
            "server exited before listening"
        );
        assert!(
            start.elapsed() < std::time::Duration::from_secs(3),
            "server did not start"
        );
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    use hmac::{Hmac, Mac};
    let body = r#"{"event":"sms:received","payload":{"sender":"x","message":"actual executable","receivedAt":"now"}}"#;
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
        .to_string();
    let mut mac = Hmac::<sha2::Sha256>::new_from_slice(b"test-key").unwrap();
    mac.update(body.as_bytes());
    mac.update(timestamp.as_bytes());
    let response = client
        .post(format!("http://{address}/transform"))
        .header("X-Timestamp", timestamp)
        .header("X-Signature", hex::encode(mac.finalize().into_bytes()))
        .body(body)
        .send()
        .await
        .unwrap();
    assert!(response.status().is_success());
    let json: serde_json::Value = response.json().await.unwrap();
    assert!(
        json["content"]
            .as_str()
            .unwrap()
            .contains("actual executable")
    );
    #[cfg(unix)]
    {
        assert!(
            Command::new("kill")
                .args(["-TERM", &child.0.id().to_string()])
                .status()
                .unwrap()
                .success()
        );
        let start = tokio::time::Instant::now();
        loop {
            if let Some(status) = child.0.try_wait().unwrap() {
                assert!(status.success());
                break;
            }
            assert!(start.elapsed() < std::time::Duration::from_secs(3));
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    }
}

#[test]
fn rejects_missing_empty_or_invalid_configuration_without_exposing_values() {
    let cases = [
        (None, None, None),
        (Some(""), None, None),
        (
            Some("secret-signing-key"),
            Some("http://evil.test/private-token"),
            None,
        ),
        (Some("secret-signing-key"), Some(""), None),
        (Some("secret-signing-key"), None, Some("bad-address")),
    ];
    for (key, url, bind) in cases {
        let mut command = Command::new(env!("CARGO_BIN_EXE_sms-discord-relay"));
        command
            .env_remove("SMS_GATEWAY_SIGNING_KEY")
            .env_remove("DISCORD_WEBHOOK_URL")
            .env_remove("BIND_ADDR");
        if let Some(key) = key {
            command.env("SMS_GATEWAY_SIGNING_KEY", key);
        }
        if let Some(url) = url {
            command.env("DISCORD_WEBHOOK_URL", url);
        }
        if let Some(bind) = bind {
            command.env("BIND_ADDR", bind);
        }
        let output = command.output().unwrap();
        assert!(!output.status.success());
        let stderr = String::from_utf8(output.stderr).unwrap();
        assert!(!stderr.contains("secret-signing-key"));
        assert!(!stderr.contains("private-token"));
        assert!(!stderr.contains("evil.test"));
        assert!(output.stdout.is_empty());
    }
}
