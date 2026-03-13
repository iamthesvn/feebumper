use serde::Deserialize;
use std::path::Path;

#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    pub bitcoin: BitcoinConfig,
    pub lightning: LightningConfig,
    pub service: ServiceConfig,
}

#[derive(Debug, Clone, Deserialize)]
pub struct BitcoinConfig {
    pub rpc_url: String,
    pub rpc_user: String,
    pub rpc_pass: String,
    #[allow(dead_code)]
    pub network: String,
    pub wallet: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct LightningConfig {
    pub lnd_rest_url: String,
    pub macaroon_path: String,
    pub tls_cert_path: Option<String>,
    pub accept_invalid_certs: Option<bool>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ServiceConfig {
    pub service_fee_sats: u64,
    pub min_target_blocks: u32,
    pub max_target_blocks: u32,
    pub listen_addr: String,
    pub invoice_expiry_secs: Option<u64>,
}

impl Config {
    pub fn load(path: impl AsRef<Path>) -> Result<Self, Box<dyn std::error::Error>> {
        let content = std::fs::read_to_string(path)?;
        let config: Config = toml::from_str(&content)?;
        Ok(config)
    }
}
