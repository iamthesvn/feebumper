use std::str::FromStr;
use std::sync::Arc;

use bitcoin::{ScriptBuf, Transaction, Txid};
use bitcoincore_rpc::json::{self, AddressType};
use bitcoincore_rpc::{Auth, Client, RpcApi};

use crate::config::BitcoinConfig;
use crate::error::Error;

/// Thin async wrapper around the synchronous `bitcoincore-rpc` client.
/// Every RPC call is dispatched onto the blocking threadpool via
/// `tokio::task::spawn_blocking`.
pub struct BitcoinRpc {
    client: Arc<Client>,
}

// The underlying jsonrpc transport is Send + Sync.
unsafe impl Send for BitcoinRpc {}
unsafe impl Sync for BitcoinRpc {}

impl BitcoinRpc {
    pub fn new(cfg: &BitcoinConfig) -> Result<Self, Error> {
        let url = match &cfg.wallet {
            Some(w) => format!("{}/wallet/{}", cfg.rpc_url, w),
            None => cfg.rpc_url.clone(),
        };
        let client = Client::new(
            &url,
            Auth::UserPass(cfg.rpc_user.clone(), cfg.rpc_pass.clone()),
        )
        .map_err(|e| Error::BitcoinRpc(e.to_string()))?;
        Ok(Self {
            client: Arc::new(client),
        })
    }

    pub async fn get_mempool_entry(
        &self,
        txid: Txid,
    ) -> Result<json::GetMempoolEntryResult, Error> {
        let c = self.client.clone();
        tokio::task::spawn_blocking(move || c.get_mempool_entry(&txid))
            .await
            .map_err(|e| Error::Internal(e.to_string()))?
            .map_err(|e| {
                if e.to_string().contains("not found")
                    || e.to_string().contains("Transaction not in mempool")
                {
                    Error::TxNotInMempool(txid.to_string())
                } else {
                    Error::BitcoinRpc(e.to_string())
                }
            })
    }

    pub async fn get_raw_transaction(&self, txid: Txid) -> Result<Transaction, Error> {
        let c = self.client.clone();
        tokio::task::spawn_blocking(move || c.get_raw_transaction(&txid, None))
            .await
            .map_err(|e| Error::Internal(e.to_string()))?
            .map_err(|e| Error::BitcoinRpc(e.to_string()))
    }

    /// Returns the estimated fee rate in **sat/vB** for the given
    /// confirmation target.
    pub async fn estimate_fee_rate(&self, target_blocks: u16) -> Result<f64, Error> {
        let c = self.client.clone();
        let est = tokio::task::spawn_blocking(move || c.estimate_smart_fee(target_blocks, None))
            .await
            .map_err(|e| Error::Internal(e.to_string()))?
            .map_err(|e| Error::BitcoinRpc(e.to_string()))?;

        let rate_btc_per_kvb = est
            .fee_rate
            .ok_or_else(|| Error::BitcoinRpc("fee estimation unavailable".into()))?;

        // BTC/kvB → sat/vB
        let sat_per_vb = rate_btc_per_kvb.to_sat() as f64 / 1000.0;
        Ok(sat_per_vb.max(1.0))
    }

    pub async fn list_unspent(&self) -> Result<Vec<json::ListUnspentResultEntry>, Error> {
        let c = self.client.clone();
        tokio::task::spawn_blocking(move || c.list_unspent(Some(1), None, None, None, None))
            .await
            .map_err(|e| Error::Internal(e.to_string()))?
            .map_err(|e| Error::BitcoinRpc(e.to_string()))
    }

    pub async fn get_new_address(&self) -> Result<ScriptBuf, Error> {
        let c = self.client.clone();
        let addr =
            tokio::task::spawn_blocking(move || c.get_new_address(None, Some(AddressType::Bech32)))
                .await
                .map_err(|e| Error::Internal(e.to_string()))?
                .map_err(|e| Error::BitcoinRpc(e.to_string()))?;
        Ok(addr.assume_checked().script_pubkey())
    }

    /// Sign the service-wallet inputs of `raw_tx`, optionally passing
    /// `prev_txouts` so the signer knows about external inputs (e.g. the
    /// anchor output).  Returns the signed transaction bytes.
    pub async fn sign_raw_transaction(
        &self,
        raw_tx: Vec<u8>,
        prev_txouts: Option<Vec<json::SignRawTransactionInput>>,
    ) -> Result<Vec<u8>, Error> {
        let c = self.client.clone();
        let tx_hex = hex::encode(&raw_tx);
        let signed = tokio::task::spawn_blocking(move || {
            c.sign_raw_transaction_with_wallet(tx_hex, prev_txouts.as_deref(), None)
        })
        .await
        .map_err(|e| Error::Internal(e.to_string()))?
        .map_err(|e| Error::BitcoinRpc(e.to_string()))?;

        Ok(signed.hex)
    }

    pub async fn send_raw_transaction(&self, tx_bytes: &[u8]) -> Result<Txid, Error> {
        let bytes = tx_bytes.to_vec();
        let c = self.client.clone();
        tokio::task::spawn_blocking(move || {
            let tx_hex = hex::encode(&bytes);
            c.send_raw_transaction(&*tx_hex)
        })
        .await
        .map_err(|e| Error::Internal(e.to_string()))?
        .map_err(|e| Error::BitcoinRpc(e.to_string()))
    }

    pub fn parse_txid(s: &str) -> Result<Txid, Error> {
        Txid::from_str(s).map_err(|e| Error::InvalidRequest(format!("bad txid: {e}")))
    }
}
