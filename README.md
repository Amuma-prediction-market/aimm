# AIMM on NEAR

**AIMM, the Adaptive Information Market Maker, as a NEAR smart contract in Rust.** A prediction market that backs its
curve with part of the money an automated market maker needs, and publishes how far each price can be trusted.

Whitepaper: [`docs/aimm.pdf`](docs/aimm.pdf). Exact rules: [`SPEC.md`](SPEC.md). This is a port of the Solidity
contracts in `weather-cast/AIMM-automated-information-market-maker`.

## The idea in four lines

1. **Pricing.** One curve prices every answer of a question, so the prices add up to 1 and a trade on any answer moves them all. It is Hanson's curve with a fixed depth, whose worst-case loss L is the *virtual* liquidity.
2. **Capital.** The operator backs it with real money, a fund ℓ. An AMM must put up all of L. AIMM lets ℓ be less.
3. **Payout.** Every winning token is paid min(1, pot ÷ winning tokens). This follows from four axioms (whitepaper, Proposition 1, stated there for two answers): a token is paid only from money the market holds, a YES and a NO together are paid at most 1, every winning token is paid the same whenever it was bought, and tokens are paid as much as those rules allow. A sale is capped at the seller's share of the pot, so no sale lowers the payout on any answer the sold token pays on. The pot is shared, so a sale can lower the payout on other answers.
4. **Information.** The payout turns each price into an *information interval*, published on chain for every answer. With ℓ at or above L the pot always covers every winner and the interval is a single point, the AMM's price. With ℓ below L it widens only while the pot falls short of an answer's winning tokens. The operator chooses ℓ; trading sets the width. With two answers, a risk-neutral trader buys YES exactly when their belief is above the upper end and NO exactly when it is below the lower end (Proposition 3). With three or more answers, below the lower end is enough to buy NO but not necessary (`SPEC.md`, section 3).

## What a replay of real trades shows

Two real Polymarket temperature questions, the last ten hours of each, with $1,000 of real money and no fee. AIMM has
L = $3,333. An informed trader who believes Polymarket's prices keeps each market in line, so every market follows
those prices by construction; the replay measures what that costs, not whether the market is accurate.

| | NYC, 22 Feb 2025 | London, 17 Mar 2025 |
|---|---|---|
| Real trades kept | 157, $7,706 | 309, $8,283 |
| Fills left out, priced outside 0.5% to 99.5% | 80 of 237 | 92 of 401 |
| Slippage, AIMM ℓ = 0.3 L | $149.70, 1.94% | $142.90, 1.73% |
| Slippage, AMM ℓ = L, same $1,000 | $462.92, 6.01% | $436.41, 5.27% |
| Slippage, AMM ℓ = L at AIMM's depth, $3,333 | $149.76 | $142.90 |
| Widest interval on any answer | 9.96 points | 10.75 points |
| Mean interval width | 0.037 points | 0.058 points |
| Lowest payout on any answer | 0.900 | 0.862 |
| Sales, and sales the cap bound | 790, 6 | 1,076, 11 |
| Operator's result, AIMM | −$209.08 | −$366.52 |
| Operator's result, AMM, same $1,000 | −$66.75 | −$148.34 |
| Operator's result, AMM at AIMM's depth | −$222.50 | −$494.46 |

AIMM gives the slippage of a $3,333 market for $1,000. The operator pays for that: it loses more than the AMM with the
same money, by $142.33 in NYC and $218.18 in London, which is less than the slippage saved, $313.22 and $293.51. Every
winner was paid 1.0000 at settlement, so these two runs never tested a short pot at the end. With a 2% fee the operator's result is +$125.07 and −$67.91 for
AIMM, and +$203.17 and +$118.25 for the AMM with the same money.

Polymarket's prices are floored at 1% and normalised before use, because their raw prices add up to between 0.71 and
1.55. Both accounts sell what they hold before they buy, and the fills left out are the ones a curve floored at 1%
cannot mirror. Keeping them makes one or two huge trades dominate and takes the slippage ratio from about 3 to about
1.3.

## What has been checked

| Check | Result |
|---|---|
| `cargo test` | 415 tests pass: 145 on the arithmetic, 270 on the contract |
| `cargo run -p aimm-sandbox --bin scenarios` | 15 claims pass on a local NEAR node, among them gas at 16 answers, a pot that settles short, a failed payout, and the storage attacks |
| The replay above as transactions, no fee | 11,818 transactions, all six questions settled exactly, the contract left holding 0 |
| The same with a 2% fee | 5,725 transactions, all six questions settled exactly, the contract left holding 0 |

In both replays every cost, refund and fee matched the native run to the unit, the trader's token balance was checked after every trade, and every price and interval was compared with the native run each hour of market time. All of this ran on a local node at protocol 88. Nothing has run on NEAR testnet or mainnet yet.

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

## Differences from the Solidity version

- One contract holds every question, instead of one market contract per question plus a shared token contract.
- Tokens live in the contract, per account. `mt_transfer` and `mt_balance_of` borrow their names from NEP-245, but this is not a NEP-245 token, so wallets and indexers will not show them.
- Limits are 2^100 tokens per trade and per answer, against 2^128 − 1, because the book is held in i128.
- A failed payout is kept for its owner instead of reverting the whole transaction.
- Each question pays for its own storage in NEAR, which has no counterpart in Solidity, and a transfer pays for the row it adds.
- A purchase goes to its sender and a payment to the seller, holder or operator, where Solidity takes a receiver. `mt_transfer` moves tokens to someone else.
- `redeem` can be called by anyone for any holder and pays only the holder. It also drops the holder's losing tokens, so the whole row goes.
- A sale whose rounding would raise the curve's cost is refused, which closes a riskless rounding profit that the Solidity clamp may share.
- Quotes refuse what the trade would refuse for the market as a whole, which Solidity's do not. Answer labels must be distinct and not empty.
- `resolve` and `halt` need 1 yoctoNEAR, so a function-call key cannot call them.
- A second `claim_surplus` is refused where Solidity returns 0, and there is no `state()` view: `get_question` and `book` give the same numbers.

## Licence

No licence is granted. All rights reserved.
