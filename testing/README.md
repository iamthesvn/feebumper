# Testing feebumper end-to-end (no real money)

Everything runs on **regtest** — a private Bitcoin chain where you mine
blocks at will and coins are free.

## What you need

| Component | Role |
|---|---|
| **bitcoind** (regtest) | Bitcoin full node — hosts the mempool, mines blocks |
| **lnd-service** | Lightning node for feebumper (receives invoice payments) |
| **lnd-user** | Lightning node simulating a paying customer |
| **feebumper** | The service under test |

## Quick start (Docker)

```bash
cd testing/

# 1. Start the infrastructure
docker compose up -d

# Wait ~15 seconds for LND nodes to initialize and sync.
sleep 15

# 2. Run the one-time setup (wallets, funding, channel)
chmod +x setup.sh test-bump.sh
./setup.sh

# 3. Start feebumper (from the repo root, in another terminal)
cd ..
cargo run -- --config testing/config.toml

# 4. Run the end-to-end test
cd testing/
./test-bump.sh
```

### What setup.sh does

1. Creates two bitcoind wallets: `miner` (for mining) and `feebumper`
   (service UTXO pool).
2. Mines 110 blocks so coinbase rewards are spendable.
3. Funds both LND nodes with 5 BTC each (on-chain).
4. Funds the `feebumper` wallet with 10 BTC.
5. Opens a 1M-sat channel from `lnd-user` → `lnd-service`.
6. Extracts lnd-service's TLS cert and admin macaroon into
   `./credentials/`.
7. Writes a ready-to-use `config.toml` for feebumper.

### What test-bump.sh does

1. Creates a raw transaction with a **P2A anchor output** (240 sats)
   and a very low fee (~1 sat/vB) so it's stuck in the mempool.
2. Calls `POST /api/v1/estimate` to get the fee breakdown.
3. Calls `POST /api/v1/bumps` to create a bump request (returns a
   Lightning invoice).
4. Pays the invoice from `lnd-user`.
5. Polls `GET /api/v1/bumps/{id}` until feebumper detects the payment,
   constructs the CPFP child, and broadcasts it.
6. Mines a block and verifies both the parent and child transactions
   confirmed.

## Alternative: Polar (GUI)

[Polar](https://lightningpolar.com/) gives you a regtest Lightning
network with a graphical UI.  Steps:

1. Create a new network with 2 LND nodes and a bitcoind backend.
2. Start the network and mine some blocks.
3. Open a channel between the nodes.
4. Point feebumper's `config.toml` at Polar's bitcoind RPC and
   one of the LND REST endpoints (Polar shows credentials in the
   node info panel).
5. Use the other LND node to pay invoices.

## Alternative: nigiri

[nigiri](https://github.com/vulpemventures/nigiri) is a CLI tool that
spins up a regtest environment in one command:

```bash
nigiri start --ln
```

Then configure feebumper to point at `localhost:18443` (bitcoind) and
`localhost:8080` (LND).

## Manual regtest (no Docker)

If you have `bitcoind` and `lnd` installed locally:

```bash
# Terminal 1: bitcoind
bitcoind -regtest -server -txindex \
  -rpcuser=rpcuser -rpcpassword=rpcpass \
  -zmqpubrawblock=tcp://127.0.0.1:28332 \
  -zmqpubrawtx=tcp://127.0.0.1:28333 \
  -fallbackfee=0.00001

# Terminal 2: lnd-service (feebumper's node)
lnd --noseedbackup \
    --bitcoin.active --bitcoin.regtest --bitcoin.node=bitcoind \
    --bitcoind.rpchost=127.0.0.1:18443 \
    --bitcoind.rpcuser=rpcuser --bitcoind.rpcpass=rpcpass \
    --bitcoind.zmqpubrawblock=tcp://127.0.0.1:28332 \
    --bitcoind.zmqpubrawtx=tcp://127.0.0.1:28333 \
    --restlisten=127.0.0.1:8080 \
    --rpclisten=127.0.0.1:10009 \
    --listen=127.0.0.1:9735 \
    --lnddir=./lnd-service-data

# Terminal 3: lnd-user
lnd --noseedbackup \
    --bitcoin.active --bitcoin.regtest --bitcoin.node=bitcoind \
    --bitcoind.rpchost=127.0.0.1:18443 \
    --bitcoind.rpcuser=rpcuser --bitcoind.rpcpass=rpcpass \
    --bitcoind.zmqpubrawblock=tcp://127.0.0.1:28332 \
    --bitcoind.zmqpubrawtx=tcp://127.0.0.1:28333 \
    --restlisten=127.0.0.1:8081 \
    --rpclisten=127.0.0.1:10010 \
    --listen=127.0.0.1:9736 \
    --lnddir=./lnd-user-data
```

Then follow the same setup steps (mine, fund, open channel) using
`bitcoin-cli -regtest` and `lncli --network=regtest`.

## Teardown

```bash
docker compose down -v   # stops everything and removes volumes
```

## Troubleshooting

| Problem | Fix |
|---|---|
| `LND nodes did not sync` | Increase the sleep in setup.sh, or check `docker logs fb-lnd-service` |
| `transaction not found in mempool` | Make sure you didn't mine a block between creating the parent tx and calling feebumper |
| `insufficient service funds` | The `feebumper` wallet needs confirmed UTXOs — run `bitcoin-cli -regtest -rpcwallet=miner sendtoaddress <fb_addr> 1.0` and mine |
| `fee bump not needed` | The parent tx already pays enough. Lower the fee by adjusting `FEE_BTC` in `test-bump.sh` |
| `LND invoice errors` | Verify the macaroon path in `config.toml` matches the extracted file in `./credentials/` |
