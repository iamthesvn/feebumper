#!/usr/bin/env bash
#
# End-to-end test of the feebumper service on regtest.
#
# Prerequisites:
#   1. docker-compose up -d
#   2. ./setup.sh completed successfully
#   3. feebumper is running:  cargo run -- --config testing/config.toml
#
# What this does:
#   1. Creates a low-fee transaction with a P2A anchor output
#   2. Calls the feebumper /estimate endpoint
#   3. Calls the feebumper /bumps endpoint (gets a Lightning invoice)
#   4. Pays the invoice from lnd-user
#   5. Waits for the CPFP child to be broadcast
#   6. Mines a block and verifies confirmation
#
set -euo pipefail

BCLI="docker exec fb-bitcoind bitcoin-cli -regtest -rpcuser=rpcuser -rpcpassword=rpcpass"
LNCLI_SVC="docker exec fb-lnd-service lncli --network=regtest --rpcserver=localhost:10009"
LNCLI_USR="docker exec fb-lnd-user lncli --network=regtest --rpcserver=localhost:10009"
FB="http://localhost:3000"

echo "============================================="
echo " feebumper end-to-end regtest test"
echo "============================================="
echo ""

# ---------------------------------------------------------------
# Step 1: Create a low-fee tx with a P2A anchor output
# ---------------------------------------------------------------
echo "--- Step 1: Create stuck transaction with P2A anchor ---"

# P2A scriptPubKey: OP_1 PUSHBYTES_2 0x4e73 → hex 51024e73
P2A_SCRIPT="51024e73"

# Get a spendable UTXO from the miner wallet
UTXO_JSON=$($BCLI -rpcwallet=miner listunspent 1 9999 '[]' true '{"minimumAmount": 1}' | jq '.[0]')
if [ "$UTXO_JSON" = "null" ]; then
    echo "ERROR: no spendable UTXO found in miner wallet" >&2
    exit 1
fi

UTXO_TXID=$(echo "$UTXO_JSON" | jq -r '.txid')
UTXO_VOUT=$(echo "$UTXO_JSON" | jq -r '.vout')
UTXO_AMOUNT=$(echo "$UTXO_JSON" | jq -r '.amount')
echo "  using UTXO: ${UTXO_TXID}:${UTXO_VOUT} (${UTXO_AMOUNT} BTC)"

CHANGE_ADDR=$($BCLI -rpcwallet=miner getnewaddress)

# Build the tx: send 0.00000240 BTC (240 sat) to P2A, rest to change.
# Set a very low fee (~1 sat/vB) so the tx "sticks" in the mempool.
# We'll subtract a tiny fee (0.00000200 BTC = 200 sat ≈ 1 sat/vB for ~200 vB tx).
ANCHOR_BTC="0.00000240"
FEE_BTC="0.00000200"
CHANGE_BTC=$(echo "$UTXO_AMOUNT - $ANCHOR_BTC - $FEE_BTC" | bc)

# createrawtransaction: send change to the address, and we'll add the
# P2A anchor output by patching the raw hex.
RAW=$($BCLI -rpcwallet=miner createrawtransaction \
    "[{\"txid\":\"${UTXO_TXID}\",\"vout\":${UTXO_VOUT}}]" \
    "[{\"${CHANGE_ADDR}\":${CHANGE_BTC}}]")

# Decode to find the output section, then build a new tx that also
# includes the P2A anchor output.  The simplest way: use fundrawtransaction
# to set inputs, then patch in the anchor.
#
# Alternatively, we can use python to splice the P2A output into the hex.
# For simplicity, let's use bitcoin-cli's JSON and re-create:

RAW_WITH_ANCHOR=$($BCLI -rpcwallet=miner createrawtransaction \
    "[{\"txid\":\"${UTXO_TXID}\",\"vout\":${UTXO_VOUT},\"sequence\":4294967293}]" \
    "{\"${CHANGE_ADDR}\":${CHANGE_BTC}}" \
    0 true)

# Unfortunately createrawtransaction doesn't support raw scriptPubKey
# outputs directly.  We'll insert the P2A output by decoding, patching,
# and re-encoding with a small python script.
PARENT_HEX=$(python3 -c "
import struct, sys, json

raw_hex = '${RAW_WITH_ANCHOR}'
raw = bytes.fromhex(raw_hex)

# Parse version (4 bytes)
version = raw[:4]
rest = raw[4:]

# Parse input count (varint - assume 1 byte for test)
n_inputs = rest[0]
pos = 1

# Skip inputs (each: 32 txid + 4 vout + varint scriptSig + 4 sequence)
for _ in range(n_inputs):
    pos += 36  # txid + vout
    script_len = rest[pos]; pos += 1
    pos += script_len + 4  # scriptSig + sequence

# Parse output count
n_outputs = rest[pos]; pos += 1
outputs_start = pos

# Skip existing outputs
for _ in range(n_outputs):
    pos += 8  # value
    script_len = rest[pos]; pos += 1
    pos += script_len

locktime = rest[pos:pos+4]

# Build P2A output: value = 240 sats (little-endian u64) + scriptPubKey
p2a_value = struct.pack('<Q', 240)
p2a_script = bytes.fromhex('51024e73')
p2a_output = p2a_value + bytes([len(p2a_script)]) + p2a_script

# Reassemble: version + inputs + (n_outputs+1) + existing outputs + p2a + locktime
new_n_outputs = n_outputs + 1
assembled = (
    version
    + bytes([n_inputs]) + rest[1:outputs_start]          # inputs
    + bytes([new_n_outputs])                              # new output count
    + rest[outputs_start:pos]                             # existing outputs
    + p2a_output                                          # P2A anchor
    + locktime
)
print(assembled.hex())
")

echo "  raw tx with P2A anchor built (${#PARENT_HEX} hex chars)"

# Sign the transaction
SIGNED_JSON=$($BCLI -rpcwallet=miner signrawtransactionwithwallet "$PARENT_HEX")
SIGNED_HEX=$(echo "$SIGNED_JSON" | jq -r '.hex')
COMPLETE=$(echo "$SIGNED_JSON" | jq -r '.complete')

if [ "$COMPLETE" != "true" ]; then
    echo "ERROR: signing incomplete" >&2
    echo "$SIGNED_JSON" >&2
    exit 1
fi

# Broadcast (don't mine — we want it stuck in the mempool)
PARENT_TXID=$($BCLI -rpcwallet=miner sendrawtransaction "$SIGNED_HEX")
echo "  parent tx broadcast: $PARENT_TXID"

# Figure out which vout is the P2A anchor
DECODED=$($BCLI decoderawtransaction "$SIGNED_HEX")
ANCHOR_VOUT=$(echo "$DECODED" | jq '[.vout[] | select(.scriptPubKey.hex == "51024e73")] | .[0].n')
echo "  anchor output at vout $ANCHOR_VOUT"
echo ""

# ---------------------------------------------------------------
# Step 2: Estimate the fee bump
# ---------------------------------------------------------------
echo "--- Step 2: Estimate fee bump ---"
ESTIMATE=$(curl -s -X POST "$FB/api/v1/estimate" \
    -H "Content-Type: application/json" \
    -d "{\"parent_txid\":\"${PARENT_TXID}\",\"anchor_vout\":${ANCHOR_VOUT},\"target_blocks\":2}")

echo "$ESTIMATE" | jq .

TOTAL_FEE=$(echo "$ESTIMATE" | jq '.total_fee_sats')
if [ "$TOTAL_FEE" = "null" ]; then
    echo "ERROR: estimate failed" >&2
    echo "$ESTIMATE" >&2
    exit 1
fi
echo ""

# ---------------------------------------------------------------
# Step 3: Create the bump request (generates LN invoice)
# ---------------------------------------------------------------
echo "--- Step 3: Create bump request ---"
BUMP=$(curl -s -X POST "$FB/api/v1/bumps" \
    -H "Content-Type: application/json" \
    -d "{\"parent_txid\":\"${PARENT_TXID}\",\"anchor_vout\":${ANCHOR_VOUT},\"target_blocks\":2}")

echo "$BUMP" | jq .

BUMP_ID=$(echo "$BUMP" | jq -r '.bump_id')
INVOICE=$(echo "$BUMP" | jq -r '.invoice')

if [ "$INVOICE" = "null" ] || [ -z "$INVOICE" ]; then
    echo "ERROR: no invoice returned" >&2
    echo "$BUMP" >&2
    exit 1
fi
echo ""

# ---------------------------------------------------------------
# Step 4: Pay the invoice from lnd-user
# ---------------------------------------------------------------
echo "--- Step 4: Pay Lightning invoice ---"
PAY_RESULT=$($LNCLI_USR payinvoice --force "$INVOICE" 2>&1) || true
echo "$PAY_RESULT" | tail -3
echo ""

# ---------------------------------------------------------------
# Step 5: Wait for feebumper to detect payment and broadcast
# ---------------------------------------------------------------
echo "--- Step 5: Wait for CPFP broadcast ---"
for i in $(seq 1 12); do
    sleep 5
    STATUS=$(curl -s "$FB/api/v1/bumps/$BUMP_ID")
    BUMP_STATUS=$(echo "$STATUS" | jq -r '.status')
    echo "  poll $i: status=$BUMP_STATUS"

    if [ "$BUMP_STATUS" = "broadcast" ]; then
        CHILD_TXID=$(echo "$STATUS" | jq -r '.child_txid')
        echo ""
        echo "  CPFP child broadcast: $CHILD_TXID"
        break
    elif [ "$BUMP_STATUS" = "failed" ]; then
        echo "  ERROR: bump failed"
        echo "$STATUS" | jq .
        exit 1
    fi
done

if [ "$BUMP_STATUS" != "broadcast" ]; then
    echo "ERROR: timed out waiting for broadcast" >&2
    exit 1
fi
echo ""

# ---------------------------------------------------------------
# Step 6: Mine a block and verify both txs confirmed
# ---------------------------------------------------------------
echo "--- Step 6: Mine a block and verify ---"
MINER_ADDR=$($BCLI -rpcwallet=miner getnewaddress)
$BCLI generatetoaddress 1 "$MINER_ADDR" > /dev/null

sleep 2

PARENT_CONFS=$($BCLI getrawtransaction "$PARENT_TXID" true | jq '.confirmations // 0')
CHILD_CONFS=$($BCLI getrawtransaction "$CHILD_TXID" true | jq '.confirmations // 0')

echo "  parent tx confirmations: $PARENT_CONFS"
echo "  child  tx confirmations: $CHILD_CONFS"
echo ""

if [ "$PARENT_CONFS" -ge 1 ] && [ "$CHILD_CONFS" -ge 1 ]; then
    echo "============================================="
    echo " TEST PASSED: fee bump confirmed on-chain"
    echo "============================================="
else
    echo "============================================="
    echo " TEST FAILED: transactions not confirmed"
    echo "============================================="
    exit 1
fi
