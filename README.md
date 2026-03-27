**THIS IMPLEMENTATION IS A PROTOTYPE AND IS FULL OF BUGS. DO NOT USE IT IN PRODUCTION.**

---

# feebumper

A transaction anchor fee-bumping service, payable via Lightning Network.

---

## 1. Overview

### What is this project?

`feebumper` is a service that accepts a stuck Bitcoin transaction — one that has stalled in the mempool due to insufficient fees — and bumps its effective fee rate using **Child-Pays-For-Parent (CPFP)** via the transaction's **anchor output**. The service charges for this work over the Lightning Network.

### What problem does it solve?

Bitcoin's mempool is a competitive fee market. Transactions broadcast during periods of low fees can become stranded when network congestion rises, leaving funds unconfirmed for hours or days. While Replace-By-Fee (RBF) is one solution, it requires the original broadcaster to sign a new transaction. **CPFP via anchor outputs** offers an alternative: any party holding the anchor output's key can create a child transaction that incentivizes miners to include the stuck parent.

This service acts as a **CPFP-as-a-service** provider, performing the bump on the user's behalf in exchange for a Lightning payment.

### Why build this with Anchor Outputs and LN?

**Anchor outputs** (standardized in [BOLT #3](https://github.com/lightning/bolts/blob/master/03-transactions.md) and available more broadly via P2A / BIP 348) are small, pre-committed outputs on a transaction specifically designed to allow a third party to attach a child transaction for fee bumping. They remove the need for the original signer to re-sign anything.

**Lightning Network payments** are the natural payment rail here because:
- Payments settle instantly, enabling the service to act immediately without waiting for on-chain confirmation of the fee.
- They are trust-minimized: the service requires payment before broadcasting the CPFP child.
- The users most likely to need fee bumping (e.g., Lightning node operators dealing with time-sensitive commitment transactions) already have LN infrastructure.

### What is the benefit to the user?

- **No re-signing required.** The user does not need to produce a new signed transaction.
- **Speed.** A Lightning payment takes seconds; the CPFP child is broadcast immediately after.
- **Convenience.** Outsource the complexity of fee estimation, UTXO management, and transaction construction to the service.
- **Time-critical rescue.** Especially useful for Lightning commitment transactions that have a CSV lock — every block counts.

---

## 2. How It Works

### How does a typical transaction become "anchor-bumpable" via this service?

A transaction is eligible for CPFP bumping through this service if it meets both:

1. **It contains a P2A (Pay-to-Anchor) output** — a small output with a script that anyone can spend (witness v1, `OP_1 OP_PUSHBYTES_2 4e73`).
2. **It is unconfirmed and sitting in the mempool**, paying below the desired fee rate.

The service does not need the user's private keys or signing authority over the original transaction.

### What are the technical steps of the fee bump?

1. **User requests an estimate** via `POST /api/v1/estimate` with the stuck transaction's TXID, anchor output index, and desired confirmation target.
2. **Service validates** the TXID is in the mempool, the anchor output exists, and calculates the CPFP fee.
3. **User creates a bump** via `POST /api/v1/bumps` — the service generates a Lightning invoice for the total fee (miner fee + service fee).
4. **User pays the invoice** over Lightning.
5. **Service detects the payment** (polls LND every 5 seconds), constructs the CPFP child transaction spending the anchor output + a service UTXO, and broadcasts it.
6. **User polls for status** via `GET /api/v1/bumps/{id}` — once broadcast, the response includes the child TXID.

### What data does the service need from the user?

| Field | Description |
|---|---|
| `parent_txid` | TXID of the stuck transaction |
| `anchor_vout` | Output index of the anchor output |
| `target_blocks` | Desired confirmation target in blocks (e.g., `1`, `3`, `6`) |

The user does **not** need to provide private keys, raw transaction hex, or wallet credentials.

### How is the bump fee calculated?

```
total_fee = miner_fee + service_fee
```

Where:

- **`miner_fee`** is computed from `estimatesmartfee`:

  ```
  package_vsize    = parent_vsize + child_vsize (≈155 vB)
  needed_pkg_fee   = target_fee_rate × package_vsize
  miner_fee        = max(needed_pkg_fee − parent_fee, child_vsize)
  ```

  The child must pay enough to bring the entire package to the target rate, with a floor of 1 sat/vB for relay.

- **`service_fee`** is a flat fee configured by the operator (default: 1000 sats).

---

## 3. API Reference

### Health check

```
GET /api/v1/health → "ok"
```

### Estimate a fee bump

```
POST /api/v1/estimate
Content-Type: application/json

{
  "parent_txid": "abc123...",
  "anchor_vout": 0,
  "target_blocks": 3
}
```

Response:

```json
{
  "parent_txid": "abc123...",
  "anchor_vout": 0,
  "target_blocks": 3,
  "parent_fee_sats": 300,
  "parent_vsize": 200,
  "miner_fee_sats": 2100,
  "service_fee_sats": 1000,
  "total_fee_sats": 3100,
  "target_fee_rate": 10.5,
  "estimated_child_vsize": 155
}
```

### Create a bump request

```
POST /api/v1/bumps
Content-Type: application/json

{
  "parent_txid": "abc123...",
  "anchor_vout": 0,
  "target_blocks": 3
}
```

Response:

```json
{
  "bump_id": "550e8400-e29b-41d4-a716-446655440000",
  "invoice": "lnbc31000n1p...",
  "total_fee_sats": 3100,
  "expires_at": "2026-02-24T15:00:00Z"
}
```

### Check bump status

```
GET /api/v1/bumps/{bump_id}
```

Response:

```json
{
  "bump_id": "550e8400-e29b-41d4-a716-446655440000",
  "status": "broadcast",
  "parent_txid": "abc123...",
  "anchor_vout": 0,
  "target_blocks": 3,
  "miner_fee_sats": 2100,
  "service_fee_sats": 1000,
  "total_fee_sats": 3100,
  "target_fee_rate": 10.5,
  "invoice": "lnbc31000n1p...",
  "child_txid": "def456...",
  "created_at": "2026-02-24T14:00:00Z",
  "expires_at": "2026-02-24T15:00:00Z"
}
```

Possible `status` values: `awaiting_payment`, `paid`, `broadcasting`, `broadcast`, `failed`, `expired`.

---

## 4. Setup

### Prerequisites

- **Rust** (edition 2021+) — install via [rustup](https://rustup.rs)
- **Bitcoin Core** (v28+ recommended for P2A support) with RPC access and a loaded wallet
- **LND** Lightning node with the REST API enabled and an active channel
- Sufficient on-chain funds in the Bitcoin Core wallet to construct CPFP child transactions

### Installation

```bash
git clone https://github.com/iamthesvn/feebumper
cd feebumper
cargo build --release
```

The compiled binary will be at `target/release/feebumper`.

### Configuration

Copy the example config and edit it:

```bash
cp config.example.toml config.toml
```

```toml
[bitcoin]
rpc_url  = "http://127.0.0.1:8332"
rpc_user = "bitcoinrpc"
rpc_pass = "changeme"
network  = "regtest"
# wallet = "feebumper"      # optional: use a named wallet

[lightning]
lnd_rest_url = "https://127.0.0.1:8080"
macaroon_path = "/path/to/admin.macaroon"
# tls_cert_path = "/path/to/tls.cert"
accept_invalid_certs = true

[service]
service_fee_sats    = 1000
min_target_blocks   = 1
max_target_blocks   = 144
listen_addr         = "127.0.0.1:3000"
invoice_expiry_secs = 3600
```

### Running the service

```bash
# Default config path: ./config.toml
./target/release/feebumper

# Custom config
./target/release/feebumper --config /etc/feebumper/config.toml

# Verbose logging
RUST_LOG=feebumper=debug ./target/release/feebumper
```

---

## 5. Project Structure

```
src/
├── main.rs          Entry point, CLI, server startup, background poller
├── config.rs        TOML config structs + loader
├── error.rs         Error enum with axum IntoResponse
├── types.rs         API request/response types, internal BumpState
├── api.rs           Axum HTTP routes
├── bitcoin_rpc.rs   Async wrapper around bitcoincore-rpc
├── lightning.rs     LND REST API client (invoices)
└── bumper.rs        Core CPFP logic: fee analysis, tx construction, broadcast
```

---

## 6. Caveats

### Security disclaimer

> **This software is experimental and unaudited. Use at your own risk.**

- The service holds on-chain Bitcoin UTXOs to construct CPFP transactions. Compromise of the service wallet results in loss of those funds.
- The service's Lightning node holds channel liquidity. Secure your node's macaroon and TLS credentials.
- Payment is collected before the child transaction is broadcast. A crash between these two steps could result in a paid-but-unbroadcast bump. Bump state is held in-memory only — a restart loses all pending bumps.
- Always verify the invoice amount before paying.

### Limitations

- **P2A anchors only.** The current implementation assumes the anchor output is a P2A (Pay-to-Anchor) anyone-can-spend output. Non-standard or keyed anchor scripts are not supported.
- **Mempool presence required.** The parent transaction must be in the node's mempool. Evicted transactions need to be re-broadcast first.
- **Package relay.** Bitcoin Core enforces package size limits (25 transactions, 101 kvB). Large ancestor chains may block acceptance.
- **No persistence.** Bump state lives in-memory. Restart = lost state.
- **Single child.** If the fee estimate is wrong, a second bump is needed — the child is not automatically re-bumped.
- **LND only.** The Lightning integration targets LND's REST API. CLN / Eclair / LDK support is not yet implemented.
