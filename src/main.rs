use sms_discord_relay::{
    SigningKey,
    server::{App, DiscordTarget},
};
use std::{net::SocketAddr, process::ExitCode};

fn configuration() -> Result<(App, SocketAddr), &'static str> {
    let key = std::env::var("SMS_GATEWAY_SIGNING_KEY")
        .map_err(|_| "SMS_GATEWAY_SIGNING_KEY is required")?;
    let key = SigningKey::new(&key).map_err(|_| "SMS_GATEWAY_SIGNING_KEY must be nonempty")?;
    let target = match std::env::var("DISCORD_WEBHOOK_URL") {
        Ok(url) => Some(DiscordTarget::parse(&url)?),
        Err(std::env::VarError::NotPresent) => None,
        Err(_) => return Err("invalid DISCORD_WEBHOOK_URL"),
    };
    let bind = match std::env::var("BIND_ADDR") {
        Ok(bind) => bind,
        Err(std::env::VarError::NotPresent) => "0.0.0.0:8080".to_owned(),
        Err(_) => return Err("invalid BIND_ADDR"),
    };
    let bind = bind.parse().map_err(|_| "invalid BIND_ADDR")?;
    Ok((App::new(key, target)?, bind))
}

async fn run() -> Result<(), &'static str> {
    let (app, bind) = configuration()?;
    let listener = tokio::net::TcpListener::bind(bind)
        .await
        .map_err(|_| "HTTP listener bind failed")?;
    let listen = listener
        .local_addr()
        .map_err(|_| "HTTP listener address failed")?;
    let mode = if std::env::var_os("DISCORD_WEBHOOK_URL").is_some() {
        "forward"
    } else {
        "transform"
    };
    tracing::info!(event = "startup", %listen, mode);
    axum::serve(listener, app.router())
        .with_graceful_shutdown(shutdown())
        .await
        .map_err(|_| "HTTP server failed")
}
async fn shutdown() {
    #[cfg(unix)]
    {
        if let Ok(mut terminate) =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        {
            tokio::select! { _ = tokio::signal::ctrl_c() => {}, _ = terminate.recv() => {} }
            tracing::info!(event = "shutdown");
            return;
        }
    }
    let _ = tokio::signal::ctrl_c().await;
    tracing::info!(event = "shutdown");
}
#[tokio::main]
async fn main() -> ExitCode {
    sms_discord_relay::logging::init();
    match run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            tracing::error!(event = "startup_failure", reason = message);
            ExitCode::FAILURE
        }
    }
}
