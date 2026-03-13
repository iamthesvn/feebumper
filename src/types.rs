use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Internal state
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BumpStatus {
    AwaitingPayment,
    Paid,
    Broadcasting,
    Broadcast,
    Failed,
    Expired,
}

#[derive(Debug, Clone)]
pub struct BumpState {
    pub id: Uuid,
    pub parent_txid: String,
    pub anchor_vout: u32,
    pub target_blocks: u32,

    #[allow(dead_code)]
    pub parent_fee_sats: u64,
    #[allow(dead_code)]
    pub parent_vsize: u64,
    pub miner_fee_sats: u64,
    pub service_fee_sats: u64,
    pub total_fee_sats: u64,
    pub target_fee_rate: f64,

    pub invoice: String,
    pub r_hash_hex: String,

    pub status: BumpStatus,
    pub child_txid: Option<String>,
    pub error_message: Option<String>,

    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
}

// ---------------------------------------------------------------------------
// API request / response types
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct EstimateRequest {
    pub parent_txid: String,
    pub anchor_vout: u32,
    pub target_blocks: u32,
}

#[derive(Debug, Serialize)]
pub struct EstimateResponse {
    pub parent_txid: String,
    pub anchor_vout: u32,
    pub target_blocks: u32,
    pub parent_fee_sats: u64,
    pub parent_vsize: u64,
    pub miner_fee_sats: u64,
    pub service_fee_sats: u64,
    pub total_fee_sats: u64,
    pub target_fee_rate: f64,
    pub estimated_child_vsize: u64,
}

#[derive(Debug, Deserialize)]
pub struct BumpCreateRequest {
    pub parent_txid: String,
    pub anchor_vout: u32,
    pub target_blocks: u32,
}

#[derive(Debug, Serialize)]
pub struct BumpCreateResponse {
    pub bump_id: Uuid,
    pub invoice: String,
    pub total_fee_sats: u64,
    pub expires_at: DateTime<Utc>,
}

#[derive(Debug, Serialize)]
pub struct BumpStatusResponse {
    pub bump_id: Uuid,
    pub status: BumpStatus,
    pub parent_txid: String,
    pub anchor_vout: u32,
    pub target_blocks: u32,
    pub miner_fee_sats: u64,
    pub service_fee_sats: u64,
    pub total_fee_sats: u64,
    pub target_fee_rate: f64,
    pub invoice: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub child_txid: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
}

impl From<&BumpState> for BumpStatusResponse {
    fn from(b: &BumpState) -> Self {
        Self {
            bump_id: b.id,
            status: b.status.clone(),
            parent_txid: b.parent_txid.clone(),
            anchor_vout: b.anchor_vout,
            target_blocks: b.target_blocks,
            miner_fee_sats: b.miner_fee_sats,
            service_fee_sats: b.service_fee_sats,
            total_fee_sats: b.total_fee_sats,
            target_fee_rate: b.target_fee_rate,
            invoice: b.invoice.clone(),
            child_txid: b.child_txid.clone(),
            error_message: b.error_message.clone(),
            created_at: b.created_at,
            expires_at: b.expires_at,
        }
    }
}
