# AIMM: Automated Information Market Maker

Information is not free. Capital is required to get good information in prediction markets. Operators have to choose between the two evils of low pricing accuracy vs capital inefficiency.
Parimutuel pools fail to provide reliable information and force informed traders to wait until the last moments to place their bet to avoid revealing information.
AMM and orderbooks can provide accurate signals but require heavy capital - more so for illiquid specialty markets.
This is the fundamental trade off between capital requirement and prediction accuracy: AMM/orderbook and parimutuel are at two ends of the spectrum.
Existing prediction markets are not optimal for specialty longtail markets such as wind speeds around a wind farm. Such information often triggers millions in profits or losses for utilities but no existing prediction market can accurately and efficiently serve it.
We propose a new mechanism: Automated Information Market Maker (AIMM). Which does not seek for a point estimation but to form a prediction range which contains true probabilities. The range length represents the accuracy of predictions.

Whitepaper: [`whitepaper/aimm.pdf`](whitepaper/aimm.pdf).

## What is here

| Path | What it is |
|---|---|
| `core/` | `aimm-core`: the curve, pot, payout, sell cap and information interval in fixed-point Rust, with no floating point |
| `core/tests/` | the reference model's 360 trades (3 to 16 answers, buys and sells, every number within one part in 1e9 or 1e9 wei), the guarantees of `SPEC.md` on random trading, and exact-value and limit tests |
| `core/examples/replay.rs`, `demo/` | fetch the two Polymarket questions, replay them on AIMM and two AMMs, write a report page. The data stays outside this repository, in `../aimm-data` |
| `contract/` | `aimm-contract`: one NEAR contract for every question, NEP-141 collateral in through `ft_on_transfer`, payouts out through `ft_transfer`, tokens held per account |
| `test-token/` | a NEP-141 test stablecoin, 6 decimals, with an open faucet. For demos only |
| `sandbox/` | `main.rs` replays the histories as transactions on a local NEAR node; `bin/scenarios.rs` checks on the same runtime what the replay does not reach |
| `demo/testnet_replay.py` | replays a question on NEAR testnet through near-cli, checking every transaction against the native run |
| `scripts/testnet.sh` | deploys to NEAR testnet, opens one question and makes one buy, then prints the commands for after the close |

## Run it

Requires Rust stable and the `wasm32-unknown-unknown` target.

```bash
cargo test
```

```bash
cargo build -p aimm-contract -p test-token --target wasm32-unknown-unknown --release
```

The local NEAR node comes with the sandbox crate, which downloads it on the first build, so `cargo test` leaves that
crate out. Run it in the dev profile: the workspace's release profile is the wasm's. The checks take a few minutes:

```bash
cargo run -p aimm-sandbox --bin scenarios
```

The replay needs the data. These take about two hours in all, most of it the on-chain run:

```bash
python3 demo/fetch_polymarket.py
```

```bash
python3 demo/prepare.py nyc && python3 demo/prepare.py london
```

```bash
for c in nyc london; do cargo run -p aimm-core --example replay -- ../aimm-data/$c.replay ../aimm-data/$c; done
```

```bash
python3 demo/report.py nyc london
```

```bash
cargo run -p aimm-sandbox -- ../aimm-data nyc london
```

## Use it

Requires [near-cli-rs](https://github.com/near/near-cli-rs) and about 4 NEAR on the contract's account. `scripts/testnet.sh` does these steps for a demo question. All amounts of tokens and money are in the collateral's smallest unit; prices and payouts are in WAD (1e18 = 1); times are in nanoseconds and must be in the future.

```bash
near contract call-function as-transaction $AIMM create_question json-args '{"args":{"collateral":"usdc.fakes.testnet","title":"Highest temperature in NYC on December 1, 2026","answers":["49°F or below","50-51°F","52-53°F","54-55°F","56°F or higher"],"resolver":"oracle.testnet","fallback_resolver":"council.testnet","close_time":"1798732740000000000","resolve_after":"1799251200000000000","fallback_after":"1799856000000000000","fee_rate":"20000000000000000"}}' prepaid-gas '100 Tgas' attached-deposit '0.5 NEAR' sign-as $OPERATOR network-config testnet sign-with-keychain send
```

The attached NEAR pays for the question's row and its curve, under 0.01 NEAR here; the rest is its storage balance, about 0.002 NEAR for each holder who buys. Open it by sending the fund with the depth and the opening prices:

```bash
near contract call-function as-transaction usdc.fakes.testnet ft_transfer_call json-args '{"receiver_id":"'$AIMM'","amount":"5000000000","msg":"{\"action\":\"open\",\"question_id\":1,\"depth\":\"7130000000\",\"prices\":[\"380000000000000000\",\"220000000000000000\",\"180000000000000000\",\"190000000000000000\",\"30000000000000000\"]}"}' prepaid-gas '100 Tgas' attached-deposit '1 yoctoNEAR' sign-as $OPERATOR network-config testnet sign-with-keychain send
```

Quote, then buy 100 YES on 54-55°F, sending at most 25 USDC; the unused part comes back:

```bash
near contract call-function as-read-only $AIMM quote_buy json-args '{"question_id":1,"range":3,"yes":true,"tickets":"100000000"}' network-config testnet now
```

```bash
near contract call-function as-transaction usdc.fakes.testnet ft_transfer_call json-args '{"receiver_id":"'$AIMM'","amount":"25000000","msg":"{\"action\":\"buy\",\"question_id\":1,\"range\":3,\"yes\":true,\"tickets\":\"100000000\",\"max_cost\":\"25000000\"}"}' prepaid-gas '100 Tgas' attached-deposit '1 yoctoNEAR' sign-as $TRADER network-config testnet sign-with-keychain send
```

Read prices, information intervals and payouts:

```bash
near contract call-function as-read-only $AIMM get_question json-args '{"question_id":1}' network-config testnet now
```

The contract account must be registered with the collateral token (`storage_deposit`). A payout goes to the seller, the holder or the operator, and if it cannot be delivered it is kept for them, and anyone can send it on with `withdraw_pending` once they are registered. `sell` needs about 25 Tgas prepaid. `mt_transfer` to someone who holds nothing of the question pays for their row, up to 0.0064 NEAR: attach 0.01 NEAR and the rest comes back.

Before buying, look at who the resolvers are. They decide the result and a report is final (`SPEC.md`, section 6).

## Licence

No licence is granted. All rights reserved.
