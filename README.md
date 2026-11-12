# AIMM: Automated Information Market Maker

Information is not free. Capital is required to get good information in prediction markets. Operators have to choose between the two evils of low pricing accuracy vs capital inefficiency.
Parimutuel pools fail to provide reliable information and force informed traders to wait until the last moments to place their bet to avoid revealing information.
AMM and orderbooks can provide accurate signals but require heavy capital - more so for illiquid specialty markets.
This is the fundamental trade off between capital requirement and prediction accuracy: AMM/orderbook and parimutuel are at two ends of the spectrum.
Existing prediction markets are not optimal for specialty longtail markets such as wind speeds around a wind farm. Such information often triggers millions in profits or losses for utilities but no existing prediction market can accurately and efficiently serve it.
We propose a new mechanism: Automated Information Market Maker (AIMM). Which does not seek for a point estimation but to form a prediction range which contains true probabilities. The range length represents the accuracy of predictions.

Whitepaper: [`whitepaper/aimm.pdf`](whitepaper/aimm.pdf).

## Repository layout

| Component | Description |
|---|---|
| `core/` | The AIMM engine in fixed-point Rust: curve, pot, payout and information interval |
| `core/tests/` | Reference-model, property and limit tests |
| `contract/` | The NEAR smart contract, serving every question from one account |
| `scripts/` | Testnet deployment |

## Run it

Requires Rust stable and the `wasm32-unknown-unknown` target.

```bash
cargo test
```

```bash
cargo build -p aimm-contract --target wasm32-unknown-unknown --release
```

## Built with

| | |
|---|---|
| Contract | [`near-sdk`](https://crates.io/crates/near-sdk) 5 |
| Testnet script | [near-cli-rs](https://github.com/near/near-cli-rs) sends the transactions |
| AI-assisted code | Written with [Claude Code](https://claude.com/claude-code) (Anthropic) under the team's direction; commits it worked on carry its co-author line |

## Hackathon

Everything in this repository was built for the NEAR Hackathon, inside its window of 19 October to 14 November 2026:
the first commit is from 3 November.

## Licence

No licence is granted. All rights reserved.
