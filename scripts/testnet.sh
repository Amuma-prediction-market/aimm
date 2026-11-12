#!/usr/bin/env bash
# Deploys AIMM to NEAR testnet, opens one question and makes one purchase, then prints the commands for the rest of the
# question's life, with near-cli-rs (https://github.com/near/near-cli-rs).
#
#   AIMM=aimm.yourname.testnet OPERATOR=yourname.testnet TRADER=friend.testnet RESOLVER=oracle.yourname.testnet \
#     FT=usdc.fakes.testnet scripts/testnet.sh
#
# AIMM must be an account you control (a sub-account works), OPERATOR and TRADER must hold the NEP-141 token FT,
# RESOLVER must be someone other than OPERATOR, and every key must be in your keychain. Run it from the repository's
# root. The question closes 10 minutes after it is created.
set -euo pipefail
: "${AIMM:?set AIMM}" "${OPERATOR:?set OPERATOR}" "${TRADER:?set TRADER}" "${RESOLVER:?set RESOLVER}"
[ "$RESOLVER" != "$OPERATOR" ] || { echo "the resolver must not be the operator: it could send the pot to itself" >&2; exit 1; }
FT=${FT:-usdc.fakes.testnet}
NET="network-config testnet"
WASM=target/wasm32-unknown-unknown/release/aimm_contract.wasm

# near-sdk 5.30 imports host functions that need protocol 88
PROTOCOL=$(curl -s -X POST https://rpc.testnet.near.org -H 'content-type: application/json' \
  -d '{"jsonrpc":"2.0","id":"x","method":"status","params":[]}' | grep -Eo '"protocol_version": *[0-9]+' | grep -Eo '[0-9]+' || true)
[ "${PROTOCOL:-0}" -ge 88 ] || { echo "testnet protocol is ${PROTOCOL:-unknown}; this wasm needs 88 or later" >&2; exit 1; }

# the remap keeps your home folder out of the panic messages in the wasm, so this hash differs from the wasm the local
# tests build without it
RUSTFLAGS="--cfg near --remap-path-prefix=$HOME=~" cargo build -p aimm-contract --target wasm32-unknown-unknown --release
near contract deploy "$AIMM" use-file "$WASM" with-init-call new json-args '{}' prepaid-gas '100 Tgas' attached-deposit '0 NEAR' $NET sign-with-keychain send

# registering twice is harmless: the token refunds the deposit
for who in "$AIMM" "$OPERATOR" "$TRADER"; do
  near contract call-function as-transaction "$FT" storage_deposit json-args "{\"account_id\":\"$who\"}" \
    prepaid-gas '30 Tgas' attached-deposit '0.01 NEAR' sign-as "$OPERATOR" $NET sign-with-keychain send
done

NOW=$(($(date +%s) * 1000000000)); CLOSE=$((NOW + 600000000000))
near contract call-function as-transaction "$AIMM" create_question json-args "{\"args\":{\"collateral\":\"$FT\",\"title\":\"Which answer will happen? (demo)\",\"answers\":[\"A\",\"B\",\"C\",\"D\",\"E\"],\"resolver\":\"$RESOLVER\",\"fallback_resolver\":\"$RESOLVER\",\"close_time\":\"$CLOSE\",\"resolve_after\":\"$CLOSE\",\"fallback_after\":\"$CLOSE\",\"fee_rate\":\"20000000000000000\"}}" \
  prepaid-gas '100 Tgas' attached-deposit '0.1 NEAR' sign-as "$OPERATOR" $NET sign-with-keychain send
ID=$(near contract call-function as-read-only "$AIMM" question_count json-args '{}' $NET now 2>/dev/null | grep -Eo '[0-9]+' | tail -1)
echo "question $ID"

# fund 50, depth 71.3: fund / L = 0.2 (6-decimal token)
OPEN="{\\\"action\\\":\\\"open\\\",\\\"question_id\\\":$ID,\\\"depth\\\":\\\"71300000\\\",\\\"prices\\\":[\\\"380000000000000000\\\",\\\"220000000000000000\\\",\\\"180000000000000000\\\",\\\"190000000000000000\\\",\\\"30000000000000000\\\"]}"
near contract call-function as-transaction "$FT" ft_transfer_call json-args "{\"receiver_id\":\"$AIMM\",\"amount\":\"50000000\",\"msg\":\"$OPEN\"}" \
  prepaid-gas '100 Tgas' attached-deposit '1 yoctoNEAR' sign-as "$OPERATOR" $NET sign-with-keychain send

near contract call-function as-read-only "$AIMM" quote_buy json-args "{\"question_id\":$ID,\"range\":3,\"yes\":true,\"tickets\":\"100000000\"}" $NET now
BUY="{\\\"action\\\":\\\"buy\\\",\\\"question_id\\\":$ID,\\\"range\\\":3,\\\"yes\\\":true,\\\"tickets\\\":\\\"100000000\\\",\\\"max_cost\\\":\\\"40000000\\\"}"
near contract call-function as-transaction "$FT" ft_transfer_call json-args "{\"receiver_id\":\"$AIMM\",\"amount\":\"40000000\",\"msg\":\"$BUY\"}" \
  prepaid-gas '100 Tgas' attached-deposit '1 yoctoNEAR' sign-as "$TRADER" $NET sign-with-keychain send
near contract call-function as-read-only "$AIMM" get_question json-args "{\"question_id\":$ID}" $NET now

echo "Now wait until the close ($CLOSE), then:"
echo "  near contract call-function as-transaction $AIMM resolve json-args '{\"question_id\":$ID,\"winner\":3}' prepaid-gas '30 Tgas' attached-deposit '1 yoctoNEAR' sign-as $RESOLVER $NET sign-with-keychain send"
echo "  near contract call-function as-transaction $AIMM redeem json-args '{\"question_id\":$ID}' prepaid-gas '50 Tgas' attached-deposit '1 yoctoNEAR' sign-as $TRADER $NET sign-with-keychain send"
echo "  near contract call-function as-transaction $AIMM claim_surplus json-args '{\"question_id\":$ID}' prepaid-gas '50 Tgas' attached-deposit '1 yoctoNEAR' sign-as $OPERATOR $NET sign-with-keychain send"
echo "  near contract call-function as-transaction $AIMM claim_fees json-args '{\"question_id\":$ID}' prepaid-gas '50 Tgas' attached-deposit '1 yoctoNEAR' sign-as $OPERATOR $NET sign-with-keychain send"
echo "  near contract call-function as-transaction $AIMM withdraw_storage json-args '{\"question_id\":$ID}' prepaid-gas '30 Tgas' attached-deposit '1 yoctoNEAR' sign-as $OPERATOR $NET sign-with-keychain send"
