use serde::Deserialize;

use crate::config::LightningConfig;
use crate::error::Error;

#[derive(Debug, Clone, PartialEq)]
pub enum InvoiceState {
    Open,
    Settled,
    Canceled,
    Accepted,
    Unknown(String),
}

pub struct LndClient {
    client: reqwest::Client,
    base_url: String,
    macaroon_hex: String,
}

// --- LND JSON shapes -------------------------------------------------------

#[derive(Debug, Deserialize)]
struct AddInvoiceResponse {
    /// base64-encoded payment hash
    r_hash: String,
    payment_request: String,
}

#[derive(Debug, Deserialize)]
struct LookupInvoiceResponse {
    state: String,
}

// ---------------------------------------------------------------------------

impl LndClient {
    pub fn new(cfg: &LightningConfig) -> Result<Self, Error> {
        let macaroon_bytes =
            std::fs::read(&cfg.macaroon_path).map_err(|e| Error::Lightning(e.to_string()))?;
        let macaroon_hex = hex::encode(&macaroon_bytes);

        let mut builder = reqwest::Client::builder();

        if let Some(ref cert_path) = cfg.tls_cert_path {
            let pem = std::fs::read(cert_path).map_err(|e| Error::Lightning(e.to_string()))?;
            let cert = reqwest::Certificate::from_pem(&pem)
                .map_err(|e| Error::Lightning(e.to_string()))?;
            builder = builder.add_root_certificate(cert);
        }

        if cfg.accept_invalid_certs.unwrap_or(false) {
            builder = builder.danger_accept_invalid_certs(true);
        }

        let client = builder
            .build()
            .map_err(|e| Error::Lightning(e.to_string()))?;

        Ok(Self {
            client,
            base_url: cfg.lnd_rest_url.trim_end_matches('/').to_string(),
            macaroon_hex,
        })
    }

    /// Create a new Lightning invoice.
    /// Returns `(bolt11_invoice, r_hash_hex)`.
    pub async fn create_invoice(
        &self,
        value_sats: u64,
        memo: &str,
        expiry_secs: u64,
    ) -> Result<(String, String), Error> {
        let body = serde_json::json!({
            "value": value_sats.to_string(),
            "memo": memo,
            "expiry": expiry_secs.to_string(),
        });

        let resp = self
            .client
            .post(format!("{}/v1/invoices", self.base_url))
            .header("Grpc-Metadata-macaroon", &self.macaroon_hex)
            .json(&body)
            .send()
            .await
            .map_err(|e| Error::Lightning(e.to_string()))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(Error::Lightning(format!("LND returned {status}: {text}")));
        }

        let inv: AddInvoiceResponse = resp
            .json()
            .await
            .map_err(|e| Error::Lightning(e.to_string()))?;

        // r_hash comes back as standard base64 — decode to bytes, then hex-encode
        // so we have a consistent representation for lookups.
        let r_hash_bytes = base64_decode(&inv.r_hash)?;
        let r_hash_hex = hex::encode(&r_hash_bytes);

        Ok((inv.payment_request, r_hash_hex))
    }

    /// Look up an invoice by its hex-encoded payment hash.
    pub async fn lookup_invoice(&self, r_hash_hex: &str) -> Result<InvoiceState, Error> {
        let resp = self
            .client
            .get(format!("{}/v1/invoice/{}", self.base_url, r_hash_hex))
            .header("Grpc-Metadata-macaroon", &self.macaroon_hex)
            .send()
            .await
            .map_err(|e| Error::Lightning(e.to_string()))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(Error::Lightning(format!("LND returned {status}: {text}")));
        }

        let info: LookupInvoiceResponse = resp
            .json()
            .await
            .map_err(|e| Error::Lightning(e.to_string()))?;

        Ok(match info.state.as_str() {
            "OPEN" => InvoiceState::Open,
            "SETTLED" => InvoiceState::Settled,
            "CANCELED" => InvoiceState::Canceled,
            "ACCEPTED" => InvoiceState::Accepted,
            other => InvoiceState::Unknown(other.to_string()),
        })
    }
}

fn base64_decode(s: &str) -> Result<Vec<u8>, Error> {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD
        .decode(s)
        .or_else(|_| base64::engine::general_purpose::URL_SAFE.decode(s))
        .map_err(|e| Error::Lightning(format!("base64 decode: {e}")))
}
