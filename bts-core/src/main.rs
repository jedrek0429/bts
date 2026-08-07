use std::net::{IpAddr, Ipv4Addr, SocketAddr};

use anyhow::Context;
use bts_core::{
    server::{CoreConfiguration, CoreServer},
    terminals::configured_state_path,
};
use tracing::{error, info};
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    initialise_logging();
    let bind_address = std::env::var("BTS_CORE_BIND")
        .map(|value| {
            value
                .parse()
                .context("BTS_CORE_BIND is not a valid socket address")
        })
        .unwrap_or_else(|_| Ok(SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 3100)))?;
    let listener = tokio::net::TcpListener::bind(bind_address)
        .await
        .with_context(|| format!("failed to bind to {bind_address}"))?;
    CoreServer::new(CoreConfiguration::production(configured_state_path()))?
        .serve(listener, None, shutdown_signal())
        .await
}

fn initialise_logging() {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("bts_core=info,tower_http=info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .compact()
        .init();
}

async fn shutdown_signal() {
    match tokio::signal::ctrl_c().await {
        Ok(()) => info!("shutdown signal received"),
        Err(error) => error!(%error, "failed to listen for shutdown signal"),
    }
}
