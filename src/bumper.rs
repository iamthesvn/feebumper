use std::collections::HashMap;
use std::sync::RwLock;

use bitcoin::absolute::LockTime;
use bitcoin::consensus::encode;
use bitcoin::transaction::Version;
use bitcoin::{Amount, OutPoint, ScriptBuf, Sequence, Transaction, TxIn, TxOut, Witness};
use bitcoincore_rpc::json::SignRawTransactionInput;
use chrono::Utc;
use uuid::Uuid;

use crate::bitcoin_rpc::BitcoinRpc;
use crate::config::Config;
use crate::error::Error;
use crate::lightning::{InvoiceState, LndClient};
use crate::types::*;

/// Conservative vsize estimate for the CPFP child transaction.
///
/// Layout: 1 P2A input (empty witness) + 1 P2WPKH input + 1 P2WPKH output.
///
/// Weight breakdown:
///   non-witness: nVersion(4) + in_count(1) + 2*input(41) + out_count(1)
///                + output(31) + nLockTime(4) = 123 bytes  → ×4 = 492
///   witness:     marker(1) + flag(1) + P2A witness(1)
///                + P2WPKH witness(1+1+72+1+33) = 111 bytes → ×1 = 111
///   total weight = 603,  vsize = ceil(603/4) = 151
///
/// We round up to 155 for safety.
const ESTIMATED_CHILD_VSIZE: u64 = 155;

pub struct FeeBumper {
    pub config: Config,
    bitcoin: BitcoinRpc,
    lightning: LndClient,
    bumps: RwLock<HashMap<Uuid, BumpState>>,
}

/// Intermediate result from the fee analysis step shared by both the
/// estimate and bump-creation paths.
struct FeeAnalysis {
    parent_fee_sats: u64,
    parent_vsize: u64,
    target_fee_rate: f64,
    miner_fee_sats: u64,
    service_fee_sats: u64,
    total_fee_sats: u64,
}

impl FeeBumper {
    pub fn new(config: Config, bitcoin: BitcoinRpc, lightning: LndClient) -> Self {
        Self {
            config,
            bitcoin,
            lightning,
            bumps: RwLock::new(HashMap::new()),
        }
    }

    // ------------------------------------------------------------------
    // Public API
    // ------------------------------------------------------------------

    pub async fn estimate(&self, req: &EstimateRequest) -> Result<EstimateResponse, Error> {
        self.validate_request(req.target_blocks)?;
        let analysis = self
            .analyse_fees(&req.parent_txid, req.anchor_vout, req.target_blocks)
            .await?;
        Ok(EstimateResponse {
            parent_txid: req.parent_txid.clone(),
            anchor_vout: req.anchor_vout,
            target_blocks: req.target_blocks,
            parent_fee_sats: analysis.parent_fee_sats,
            parent_vsize: analysis.parent_vsize,
            miner_fee_sats: analysis.miner_fee_sats,
            service_fee_sats: analysis.service_fee_sats,
            total_fee_sats: analysis.total_fee_sats,
            target_fee_rate: analysis.target_fee_rate,
            estimated_child_vsize: ESTIMATED_CHILD_VSIZE,
        })
    }

    pub async fn create_bump(&self, req: &BumpCreateRequest) -> Result<BumpCreateResponse, Error> {
        self.validate_request(req.target_blocks)?;
        let analysis = self
            .analyse_fees(&req.parent_txid, req.anchor_vout, req.target_blocks)
            .await?;

        let expiry_secs = self.config.service.invoice_expiry_secs.unwrap_or(3600);
        let memo = format!("Fee bump for {}", &req.parent_txid[..16]);

        let (invoice, r_hash_hex) = self
            .lightning
            .create_invoice(analysis.total_fee_sats, &memo, expiry_secs)
            .await?;

        let now = Utc::now();
        let expires_at = now + chrono::Duration::seconds(expiry_secs as i64);
        let id = Uuid::new_v4();

        let state = BumpState {
            id,
            parent_txid: req.parent_txid.clone(),
            anchor_vout: req.anchor_vout,
            target_blocks: req.target_blocks,
            parent_fee_sats: analysis.parent_fee_sats,
            parent_vsize: analysis.parent_vsize,
            miner_fee_sats: analysis.miner_fee_sats,
            service_fee_sats: analysis.service_fee_sats,
            total_fee_sats: analysis.total_fee_sats,
            target_fee_rate: analysis.target_fee_rate,
            invoice: invoice.clone(),
            r_hash_hex,
            status: BumpStatus::AwaitingPayment,
            child_txid: None,
            error_message: None,
            created_at: now,
            expires_at,
        };

        self.bumps.write().unwrap().insert(id, state);

        Ok(BumpCreateResponse {
            bump_id: id,
            invoice,
            total_fee_sats: analysis.total_fee_sats,
            expires_at,
        })
    }

    pub fn get_bump(&self, id: Uuid) -> Result<BumpStatusResponse, Error> {
        let bumps = self.bumps.read().unwrap();
        let b = bumps.get(&id).ok_or(Error::BumpNotFound(id))?;
        Ok(BumpStatusResponse::from(b))
    }

    /// Called periodically by the background task. Checks every
    /// awaiting-payment bump, and when the invoice is settled, constructs
    /// and broadcasts the CPFP child.
    pub async fn process_pending_bumps(&self) {
        let pending: Vec<(Uuid, String)> = {
            let bumps = self.bumps.read().unwrap();
            bumps
                .values()
                .filter(|b| b.status == BumpStatus::AwaitingPayment)
                .map(|b| (b.id, b.r_hash_hex.clone()))
                .collect()
        };

        for (id, r_hash) in pending {
            match self.lightning.lookup_invoice(&r_hash).await {
                Ok(InvoiceState::Settled) => {
                    tracing::info!(%id, "invoice settled — broadcasting CPFP");
                    self.set_status(id, BumpStatus::Paid);
                    self.execute_bump(id).await;
                }
                Ok(InvoiceState::Canceled) => {
                    tracing::info!(%id, "invoice canceled — marking expired");
                    self.set_status(id, BumpStatus::Expired);
                }
                Ok(_) => {} // still open / accepted — nothing to do
                Err(e) => {
                    tracing::warn!(%id, "invoice lookup failed: {e}");
                }
            }
        }

        // Expire stale bumps that passed their deadline.
        let now = Utc::now();
        let mut bumps = self.bumps.write().unwrap();
        for b in bumps.values_mut() {
            if b.status == BumpStatus::AwaitingPayment && b.expires_at < now {
                b.status = BumpStatus::Expired;
            }
        }
    }

    // ------------------------------------------------------------------
    // Internal helpers
    // ------------------------------------------------------------------

    fn validate_request(&self, target_blocks: u32) -> Result<(), Error> {
        let min = self.config.service.min_target_blocks;
        let max = self.config.service.max_target_blocks;
        if target_blocks < min || target_blocks > max {
            return Err(Error::InvalidRequest(format!(
                "target_blocks must be between {min} and {max}"
            )));
        }
        Ok(())
    }

    async fn analyse_fees(
        &self,
        parent_txid_str: &str,
        anchor_vout: u32,
        target_blocks: u32,
    ) -> Result<FeeAnalysis, Error> {
        let parent_txid = BitcoinRpc::parse_txid(parent_txid_str)?;

        // Confirm the parent is in the mempool and fetch its fee / vsize.
        let entry = self.bitcoin.get_mempool_entry(parent_txid).await?;

        // Validate the anchor output exists on the parent.
        let parent_tx = self.bitcoin.get_raw_transaction(parent_txid).await?;
        if anchor_vout as usize >= parent_tx.output.len() {
            return Err(Error::InvalidAnchorVout(anchor_vout));
        }

        let parent_fee_sats = entry.fees.base.to_sat();
        let parent_vsize = entry.vsize;

        let target_fee_rate = self.bitcoin.estimate_fee_rate(target_blocks as u16).await?;

        let package_vsize = parent_vsize + ESTIMATED_CHILD_VSIZE;
        let needed_package_fee = (target_fee_rate * package_vsize as f64).ceil() as u64;

        let miner_fee_sats = needed_package_fee.saturating_sub(parent_fee_sats);
        if miner_fee_sats == 0 {
            return Err(Error::BumpNotNeeded);
        }

        // The child must at least pay 1 sat/vB for itself to pass relay.
        let miner_fee_sats = miner_fee_sats.max(ESTIMATED_CHILD_VSIZE);

        let service_fee_sats = self.config.service.service_fee_sats;
        let total_fee_sats = miner_fee_sats + service_fee_sats;

        Ok(FeeAnalysis {
            parent_fee_sats,
            parent_vsize,
            target_fee_rate,
            miner_fee_sats,
            service_fee_sats,
            total_fee_sats,
        })
    }

    async fn execute_bump(&self, id: Uuid) {
        let bump_snapshot = {
            let bumps = self.bumps.read().unwrap();
            bumps.get(&id).cloned()
        };
        let Some(bump) = bump_snapshot else { return };

        self.set_status(id, BumpStatus::Broadcasting);

        match self.construct_and_broadcast(&bump).await {
            Ok(child_txid) => {
                let mut bumps = self.bumps.write().unwrap();
                if let Some(b) = bumps.get_mut(&id) {
                    b.status = BumpStatus::Broadcast;
                    b.child_txid = Some(child_txid.to_string());
                }
                tracing::info!(%id, %child_txid, "CPFP child broadcast");
            }
            Err(e) => {
                let mut bumps = self.bumps.write().unwrap();
                if let Some(b) = bumps.get_mut(&id) {
                    b.status = BumpStatus::Failed;
                    b.error_message = Some(e.to_string());
                }
                tracing::error!(%id, "CPFP broadcast failed: {e}");
            }
        }
    }

    /// Build the CPFP child transaction, sign it, and broadcast.
    async fn construct_and_broadcast(&self, bump: &BumpState) -> Result<bitcoin::Txid, Error> {
        let parent_txid = BitcoinRpc::parse_txid(&bump.parent_txid)?;
        let parent_tx = self.bitcoin.get_raw_transaction(parent_txid).await?;

        let anchor_output = parent_tx
            .output
            .get(bump.anchor_vout as usize)
            .ok_or(Error::InvalidAnchorVout(bump.anchor_vout))?;
        let anchor_value = anchor_output.value;
        let anchor_script = anchor_output.script_pubkey.clone();

        // Pick a service UTXO that can cover the miner fee.
        let utxos = self.bitcoin.list_unspent().await?;
        let dust_threshold = Amount::from_sat(546);
        let needed = Amount::from_sat(bump.miner_fee_sats);

        let utxo = utxos
            .iter()
            .filter(|u| u.spendable)
            .find(|u| {
                let total_input = u.amount + anchor_value;
                total_input >= needed + dust_threshold
            })
            .ok_or(Error::InsufficientFunds)?;

        let change_script = self.bitcoin.get_new_address().await?;
        let change_amount = (utxo.amount + anchor_value)
            .checked_sub(Amount::from_sat(bump.miner_fee_sats))
            .ok_or(Error::InsufficientFunds)?;

        let child_tx = Transaction {
            version: Version::TWO,
            lock_time: LockTime::ZERO,
            input: vec![
                TxIn {
                    previous_output: OutPoint::new(parent_txid, bump.anchor_vout),
                    script_sig: ScriptBuf::new(),
                    sequence: Sequence::ENABLE_RBF_NO_LOCKTIME,
                    witness: Witness::default(),
                },
                TxIn {
                    previous_output: OutPoint::new(utxo.txid, utxo.vout),
                    script_sig: ScriptBuf::new(),
                    sequence: Sequence::ENABLE_RBF_NO_LOCKTIME,
                    witness: Witness::default(),
                },
            ],
            output: vec![TxOut {
                value: change_amount,
                script_pubkey: change_script,
            }],
        };

        let raw = encode::serialize(&child_tx);

        // Tell the signer about the anchor input so that taproot sighash
        // (which commits to ALL input amounts) can be computed if the
        // service UTXO happens to be P2TR.
        let prev = vec![SignRawTransactionInput {
            txid: parent_txid,
            vout: bump.anchor_vout,
            script_pub_key: anchor_script,
            redeem_script: None,
            amount: Some(anchor_value),
        }];

        let signed_bytes = self.bitcoin.sign_raw_transaction(raw, Some(prev)).await?;

        // Ensure the anchor input still carries an empty witness (P2A
        // spending rule).  The wallet signer leaves unknown inputs alone,
        // but we enforce it explicitly.
        let mut signed_tx: Transaction = encode::deserialize(&signed_bytes)
            .map_err(|e| Error::Internal(format!("deserialize signed tx: {e}")))?;
        signed_tx.input[0].witness = Witness::default();

        let final_bytes = encode::serialize(&signed_tx);
        let child_txid = self.bitcoin.send_raw_transaction(&final_bytes).await?;

        Ok(child_txid)
    }

    fn set_status(&self, id: Uuid, status: BumpStatus) {
        let mut bumps = self.bumps.write().unwrap();
        if let Some(b) = bumps.get_mut(&id) {
            b.status = status;
        }
    }
}
