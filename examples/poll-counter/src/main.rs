//! Serves the counter on port 9080.

use poll_counter::Counter;
use restate_ext::shutdown_signal;
use restate_sdk::{endpoint::Endpoint, filter::ReplayAwareFilter, http_server::HttpServer};
use tokio::net::TcpListener;
use tracing_subscriber::{EnvFilter, Layer, layer::SubscriberExt, util::SubscriberInitExt};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::fmt::layer()
                .with_filter(
                    EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
                )
                .with_filter(ReplayAwareFilter),
        )
        .init();

    let listener = TcpListener::bind("0.0.0.0:9080").await?;
    let endpoint = Endpoint::builder().bind(Counter).build();

    HttpServer::new(endpoint)
        .serve_with_cancel(listener, shutdown_signal())
        .await;

    Ok(())
}
