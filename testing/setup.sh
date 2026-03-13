#!/usr/bin/env bash
#
# Sets up the regtest environment after docker-compose is running.
#
# What this does:
#   1. Creates a bitcoind wallet for mining and one for the feebumper service
#   2. Mines 110 blocks (so coinbase rewards are spendable)
#   3. Funds both LND nodes on-chain
#   4. Funds the feebumper service wallet
#   5. Opens a channel from lnd-user → lnd-service (so the user can pay invoices)
#   6. Extracts lnd-service credentials to ./credentials/
#   7. Writes a ready-to-use feebumper config.toml
#
set -euo pipefail

BCLI="docker exec fb-bitcoind bitcoin-cli -regtest -rpcuser=rpcuser -rpcpassword=rpcpass"
LNCLI_SVC="docker exec fb-lnd-service lncli --network=regtest --rpcserver=localhost:10009"
LNCLI_USR="docker exec fb-lnd-user lncli --network=regtest --rpcserver=localhost:10009"

wait_for_sync() {
    echo "waiting for LND nodes to sync to chain..."
    for i in $(seq 1 30); do
        SVC_SYNCED=$($LNCLI_SVC getinfo 2>/dev/null | jq -r '.synced_to_chain' || echo "false")
        USR_SYNCED=$($LNCLI_USR getinfo 2>/dev/null | jq -r '.synced_to_chain' || echo "false")
        if [ "$SVC_SYNCED" = "true" ] && [ "$USR_SYNCED" = "true" ]; then
            echo "both LND nodes synced."
            return 0
        fi
        sleep 2
    done
    echo "ERROR: LND nodes did not sync in time" >&2
    exit 1
}

echo "=== 1. Create bitcoind wallets ==="
$BCLI createwallet "miner"     2>/dev/null || true
$BCLI createwallet "feebumper" 2>/dev/null || true

echo "=== 2. Mine 110 blocks ==="
MINER_ADDR=$($BCLI -rpcwallet=miner getnewaddress)
$BCLI generatetoaddress 110 "$MINER_ADDR" > /dev/null
echo "mined 110 blocks to $MINER_ADDR"

wait_for_sync

echo "=== 3. Fund LND nodes ==="
SVC_ADDR=$($LNCLI_SVC newaddress p2wkh | jq -r '.address')
USR_ADDR=$($LNCLI_USR newaddress p2wkh | jq -r '.address')

$BCLI -rpcwallet=miner sendtoaddress "$SVC_ADDR" 5.0
$BCLI -rpcwallet=miner sendtoaddress "$USR_ADDR" 5.0

echo "=== 4. Fund feebumper service wallet ==="
FB_ADDR=$($BCLI -rpcwallet=feebumper getnewaddress)
$BCLI -rpcwallet=miner sendtoaddress "$FB_ADDR" 10.0

echo "=== 5. Mine to confirm funding txs ==="
$BCLI generatetoaddress 6 "$MINER_ADDR" > /dev/null
sleep 3
wait_for_sync

echo "=== 6. Open channel: lnd-user → lnd-service ==="
SVC_PUBKEY=$($LNCLI_SVC getinfo | jq -r '.identity_pubkey')
SVC_HOST="fb-lnd-service:9735"
$LNCLI_USR connect "$SVC_PUBKEY@$SVC_HOST" 2>/dev/null || true
$LNCLI_USR openchannel "$SVC_PUBKEY" 1000000

echo "=== 7. Mine to confirm channel ==="
$BCLI generatetoaddress 6 "$MINER_ADDR" > /dev/null
sleep 5
wait_for_sync

echo "=== 8. Extract lnd-service credentials ==="
mkdir -p credentials
docker cp fb-lnd-service:/root/.lnd/tls.cert        credentials/tls.cert
docker cp fb-lnd-service:/root/.lnd/data/chain/bitcoin/regtest/admin.macaroon credentials/admin.macaroon
echo "credentials saved to ./credentials/"

echo "=== 9. Write feebumper config ==="
CRED_DIR="$(cd credentials && pwd)"
cat > config.toml <<EOF
[bitcoin]
rpc_url  = "http://127.0.0.1:18443"
rpc_user = "rpcuser"
rpc_pass = "rpcpass"
network  = "regtest"
wallet   = "feebumper"

[lightning]
lnd_rest_url        = "https://127.0.0.1:8080"
macaroon_path       = "${CRED_DIR}/admin.macaroon"
tls_cert_path       = "${CRED_DIR}/tls.cert"
accept_invalid_certs = true

[service]
service_fee_sats    = 500
min_target_blocks   = 1
max_target_blocks   = 144
listen_addr         = "127.0.0.1:3000"
invoice_expiry_secs = 3600
EOF
echo "config.toml written — ready to run feebumper."

echo ""
echo "--- setup complete ---"
echo "  bitcoind wallets:  miner, feebumper"
echo "  lnd-service funds: $($LNCLI_SVC walletbalance | jq -r '.confirmed_balance') sat"
echo "  lnd-user funds:    $($LNCLI_USR walletbalance | jq -r '.confirmed_balance') sat"
echo "  channel:           $($LNCLI_USR listchannels | jq '.channels | length') open"
