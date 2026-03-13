mod api;
mod bitcoin_rpc;
mod bumper;
mod config;
mod error;
mod lightning;
mod types;

use std::sync::Arc;
use std::time::Duration;

use clap::Parser;

use crate::bitcoin_rpc::BitcoinRpc;
use crate::bumper::FeeBumper;
use crate::config::Config;
use crate::lightning::LndClient;

#[derive(Parser)]
#[command(
    name = "feebumper",
    version,
    about = "Anchor fee-bumping service payable via Lightning"
)]
struct Args {
    /// Path to the TOML configuration file.
    #[arg(short, long, default_value = "config.toml")]
    config: String,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "feebumper=info,tower_http=info".into()),
        )
        .init();

    let args = Args::parse();
    let config = Config::load(&args.config)?;

    let bitcoin = BitcoinRpc::new(&config.bitcoin)?;
    let lightning = LndClient::new(&config.lightning)?;

    let bumper = Arc::new(FeeBumper::new(config.clone(), bitcoin, lightning));

    // Background task: poll LND for settled invoices every 5 seconds.
    let poll_bumper = Arc::clone(&bumper);
    tokio::spawn(async move {
        loop {
            poll_bumper.process_pending_bumps().await;
            tokio::time::sleep(Duration::from_secs(5)).await;
        }
    });

    let listen_addr = &bumper.config.service.listen_addr;
    tracing::info!("starting feebumper on {listen_addr}");

    let listener = tokio::net::TcpListener::bind(listen_addr).await?;
    axum::serve(listener, api::router(Arc::clone(&bumper))).await?;

    Ok(())
}
