use std::{
    io::{Read, Write},
    net::TcpListener,
    process::{Command, Output, Stdio},
    thread,
    time::{Duration, Instant},
};

fn probe(bind: &str) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_sms-discord-relay"));
    command
        .env_clear()
        .env("BIND_ADDR", bind)
        .env("RUST_LOG", "trace")
        .env("DISCORD_WEBHOOK_URL", "invalid-secret-webhook")
        .arg("--healthcheck");
    bounded_output(&mut command)
}

fn bounded_output(command: &mut Command) -> Output {
    let mut child = command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let start = Instant::now();
    while child.try_wait().unwrap().is_none() {
        if start.elapsed() >= Duration::from_secs(3) {
            child.kill().unwrap();
            let _ = child.wait();
            panic!("probe exceeded container's 3s timeout");
        }
        thread::sleep(Duration::from_millis(10));
    }
    let output = child.wait_with_output().unwrap();
    assert!(output.stdout.is_empty(), "stdout: {:?}", output.stdout);
    assert!(output.stderr.is_empty(), "stderr: {:?}", output.stderr);
    output
}

fn responder(response: &'static str) -> (String, thread::JoinHandle<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    responder_on(listener, response)
}

fn responder_on(
    listener: TcpListener,
    response: &'static str,
) -> (String, thread::JoinHandle<String>) {
    let address = listener.local_addr().unwrap().to_string();
    listener.set_nonblocking(true).unwrap();
    let handle = thread::spawn(move || {
        let start = Instant::now();
        let mut stream = loop {
            match listener.accept() {
                Ok((stream, _)) => break stream,
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    if start.elapsed() >= Duration::from_millis(300) {
                        return String::new();
                    }
                    thread::sleep(Duration::from_millis(10));
                }
                Err(e) => panic!("{e}"),
            }
        };
        stream
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        let mut request = Vec::new();
        while !request.ends_with(b"\r\n\r\n") {
            let mut byte = [0];
            assert_eq!(stream.read(&mut byte).unwrap(), 1);
            request.push(byte[0]);
        }
        stream.write_all(response.as_bytes()).unwrap();
        String::from_utf8(request).unwrap()
    });
    (address, handle)
}

#[test]
fn dropped_connection_is_not_retried() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap().to_string();
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = [0; 4096];
        assert!(stream.read(&mut request).unwrap() > 0);
        drop(stream);
        listener.set_nonblocking(true).unwrap();
        let start = Instant::now();
        while start.elapsed() < Duration::from_millis(300) {
            if let Ok((mut stream, _)) = listener.accept() {
                assert!(stream.read(&mut request).unwrap() > 0);
                stream
                    .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                    .unwrap();
                return 2;
            }
            thread::sleep(Duration::from_millis(10));
        }
        1
    });
    let output = probe(&address);
    assert_eq!(server.join().unwrap(), 1);
    assert_eq!(output.status.code(), Some(1));
}

#[test]
fn closed_listener_fails_quietly() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap().to_string();
    drop(listener);
    assert_eq!(probe(&address).status.code(), Some(1));
}

#[test]
fn invalid_probe_configuration_fails_quietly() {
    for bind in [
        "",
        "bad-secret",
        "127.0.0.1:0",
        "[::1]:0",
        "http://127.0.0.1:8080",
        "127.0.0.1:8080/path",
    ] {
        assert_eq!(probe(bind).status.code(), Some(1));
    }
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt;
        let mut command = Command::new(env!("CARGO_BIN_EXE_sms-discord-relay"));
        command
            .env_clear()
            .env("BIND_ADDR", std::ffi::OsString::from_vec(vec![0xff]))
            .arg("--healthcheck");
        assert_eq!(bounded_output(&mut command).status.code(), Some(1));
        let (address, server) =
            responder("HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
        let mut command = Command::new(env!("CARGO_BIN_EXE_sms-discord-relay"));
        command
            .env_clear()
            .env("BIND_ADDR", address)
            .env(
                "SMS_GATEWAY_SIGNING_KEY",
                std::ffi::OsString::from_vec(vec![0xff]),
            )
            .env(
                "DISCORD_WEBHOOK_URL",
                std::ffi::OsString::from_vec(vec![0xff]),
            )
            .env("RUST_LOG", "trace")
            .arg("--healthcheck");
        assert_eq!(bounded_output(&mut command).status.code(), Some(0));
        assert!(server.join().unwrap().starts_with("GET /healthz "));
    }
}

#[test]
fn ipv6_explicit_and_wildcard_connect_to_loopback() {
    for wildcard in [false, true] {
        let listener = TcpListener::bind("[::1]:0").unwrap();
        let (address, server) = responder_on(
            listener,
            "HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
        );
        let bind = if wildcard {
            address.replace("[::1]", "[::]")
        } else {
            address
        };
        assert_eq!(probe(&bind).status.code(), Some(0));
        assert!(server.join().unwrap().starts_with("GET /healthz "));
    }
}

#[test]
fn ipv4_wildcard_connects_to_loopback() {
    let (address, server) =
        responder("HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
    assert_eq!(
        probe(&address.replace("127.0.0.1", "0.0.0.0"))
            .status
            .code(),
        Some(0)
    );
    assert!(server.join().unwrap().starts_with("GET /healthz "));
}

#[test]
fn unsupported_arguments_fail_quietly_without_starting_server() {
    for args in [
        vec!["--unknown"],
        vec!["--healthcheck", "extra"],
        vec!["--healthcheck", "--healthcheck"],
        vec!["--allow-http"],
    ] {
        let mut command = Command::new(env!("CARGO_BIN_EXE_sms-discord-relay"));
        command.env_clear().env("RUST_LOG", "trace").args(args);
        assert_eq!(bounded_output(&mut command).status.code(), Some(1));
    }
}

#[test]
fn hostnames_are_rejected_before_network_access() {
    let (address, server) =
        responder("HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
    let output = probe(&address.replace("127.0.0.1", "localhost"));
    let request = server.join().unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(request.is_empty());
}

#[test]
fn stalled_headers_fail_within_two_second_budget() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap().to_string();
    let server = thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        thread::sleep(Duration::from_millis(2600));
        drop(stream);
    });
    let start = Instant::now();
    let output = probe(&address);
    let elapsed = start.elapsed();
    server.join().unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(elapsed >= Duration::from_millis(1800), "{elapsed:?}");
    assert!(elapsed < Duration::from_millis(2400), "{elapsed:?}");
}

#[test]
fn environment_proxy_cannot_report_false_health() {
    let proxy = TcpListener::bind("127.0.0.1:0").unwrap();
    proxy.set_nonblocking(true).unwrap();
    let target = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = target.local_addr().unwrap();
    drop(target);
    let mut command = Command::new(env!("CARGO_BIN_EXE_sms-discord-relay"));
    command
        .env_clear()
        .env("BIND_ADDR", address.to_string())
        .env(
            "HTTP_PROXY",
            format!("http://{}", proxy.local_addr().unwrap()),
        )
        .env(
            "http_proxy",
            format!("http://{}", proxy.local_addr().unwrap()),
        )
        .arg("--healthcheck");
    let server = thread::spawn(move || {
        let start = Instant::now();
        while start.elapsed() < Duration::from_millis(300) {
            if let Ok((mut stream, _)) = proxy.accept() {
                let mut request = [0; 4096];
                assert!(stream.read(&mut request).unwrap() > 0);
                stream
                    .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                    .unwrap();
                return true;
            }
            thread::sleep(Duration::from_millis(10));
        }
        false
    });
    let output = bounded_output(&mut command);
    let contacted_proxy = server.join().unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(!contacted_proxy);
}

#[test]
fn redirects_are_not_followed() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap().to_string();
    let location = format!("http://{address}/healthz");
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = [0; 4096];
        assert!(stream.read(&mut request).unwrap() > 0);
        write!(stream, "HTTP/1.1 302 Found\r\nLocation: {location}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").unwrap();
        drop(stream);
        listener.set_nonblocking(true).unwrap();
        let start = Instant::now();
        while start.elapsed() < Duration::from_millis(300) {
            if let Ok((mut stream, _)) = listener.accept() {
                assert!(stream.read(&mut request).unwrap() > 0);
                stream
                    .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                    .unwrap();
                return 2;
            }
            thread::sleep(Duration::from_millis(10));
        }
        1
    });
    let output = probe(&address);
    let requests = server.join().unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(requests, 1);
}

#[test]
fn only_http_200_is_healthy() {
    for response in [
        "HTTP/1.1 204 No Content\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
        "HTTP/1.1 500 Internal Server Error\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
    ] {
        let (address, server) = responder(response);
        assert_eq!(probe(&address).status.code(), Some(1));
        server.join().unwrap();
    }
}

#[test]
fn healthcheck_succeeds_without_credentials_or_logging() {
    let (address, server) =
        responder("HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
    assert_eq!(probe(&address).status.code(), Some(0));
    let request = server.join().unwrap();
    assert!(
        request.starts_with("GET /healthz HTTP/1.1\r\n"),
        "{request}"
    );
    assert!(!request.contains("Authorization"));
}
