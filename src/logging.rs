//! Stderr observability; never pass request or credential data to events.
use tracing::{Event, Metadata, Subscriber, subscriber::Interest};
use tracing_log::NormalizeEvent;
use tracing_subscriber::{EnvFilter, Layer, fmt::MakeWriter, layer::Context, prelude::*};

struct AppTargets;
fn app_target(target: &str) -> bool {
    target == "sms_discord_relay" || target.starts_with("sms_discord_relay::")
}
impl<S: Subscriber> Layer<S> for AppTargets {
    fn register_callsite(&self, _: &'static Metadata<'static>) -> Interest {
        // LogTracer checks original target metadata before dispatching a synthetic
        // `log` callsite event. Never cache a decision for that shared callsite.
        Interest::sometimes()
    }
    fn enabled(&self, metadata: &Metadata<'_>, _: Context<'_, S>) -> bool {
        app_target(metadata.target())
    }
    fn event_enabled(&self, event: &Event<'_>, _: Context<'_, S>) -> bool {
        let normalized = event.normalized_metadata();
        app_target(
            normalized
                .as_ref()
                .unwrap_or_else(|| event.metadata())
                .target(),
        )
    }
}

pub fn subscriber<W>(directives: Option<&str>, writer: W) -> impl Subscriber + Send + Sync
where
    W: for<'a> MakeWriter<'a> + Send + Sync + 'static,
{
    let filter = EnvFilter::builder()
        .with_regex(false)
        .with_default_directive(tracing::Level::INFO.into())
        .parse(directives.unwrap_or("info"))
        .unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::registry()
        .with(filter)
        .with(AppTargets)
        .with(
            tracing_subscriber::fmt::layer()
                .with_ansi(false)
                .without_time()
                .with_writer(writer),
        )
}

pub fn init() {
    tracing_log::LogTracer::init().expect("log bridge initialization failed");
    let directives = std::env::var("RUST_LOG").ok();
    tracing::subscriber::set_global_default(subscriber(directives.as_deref(), std::io::stderr))
        .expect("subscriber initialization failed");
}
