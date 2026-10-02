use std::{
    io::{self, Write},
    sync::{Arc, Mutex},
};
#[derive(Clone, Default)]
struct Capture(Arc<Mutex<Vec<u8>>>);
impl Write for Capture {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
impl Capture {
    fn text(&self) -> String {
        String::from_utf8(self.0.lock().unwrap().clone()).unwrap()
    }
}

// This integration-test process owns its global log bridge; no env mutation.
#[test]
fn app_allowlist_intersects_env_filter_for_native_and_bridged_events() {
    tracing_log::LogTracer::init().unwrap();
    for directive in [
        None,
        Some("trace"),
        Some("reqwest=trace,sms_discord_relay=trace"),
        Some("sms_discord_relay=trace"),
        Some("info"),
    ] {
        let output = Capture::default();
        let sink = output.clone();
        let subscriber = sms_discord_relay::logging::subscriber(directive, move || sink.clone());
        tracing::subscriber::with_default(subscriber, || {
            tracing::info!(target: "sms_discord_relay", event="native_safe");
            tracing::info!(target: "sms_discord_relay::bridge", event="child_safe");
            log::info!(target: "sms_discord_relay::bridge", "bridge_safe");
            log::error!(target: "reqwest", "DEPENDENCY_LOG_SECRET");
            log::error!(target: "hyper::client", "WIRE_LOG_SECRET");
            tracing::trace!(target: "hyper", token="WIRE_TRACE_SECRET");
            tracing::error!(target: "reqwest", token="DEPENDENCY_TRACE_SECRET");
            tracing::error!(target: "sms_discord_relay_evil", token="PREFIX_SECRET");
            tracing::error!(target: "log", token="SYNTHETIC_LOG_SECRET");
        });
        let logs = output.text();
        assert!(logs.contains("native_safe"), "{directive:?}: {logs}");
        assert!(logs.contains("child_safe"), "{directive:?}: {logs}");
        assert!(logs.contains("bridge_safe"), "{directive:?}: {logs}");
        assert!(!logs.contains("SECRET"), "{directive:?}: {logs}");
    }
    for directive in [
        "off",
        "reqwest=trace",
        "reqwest=trace,sms_discord_relay=off",
    ] {
        let output = Capture::default();
        let sink = output.clone();
        tracing::subscriber::with_default(
            sms_discord_relay::logging::subscriber(Some(directive), move || sink.clone()),
            || {
                tracing::error!(target: "sms_discord_relay", event="disabled");
                log::error!(target: "sms_discord_relay", "disabled_bridge");
                log::error!(target: "reqwest", "disabled_dependency");
            },
        );
        assert!(output.text().is_empty(), "{}", output.text());
    }
}
