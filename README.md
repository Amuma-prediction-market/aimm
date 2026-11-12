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
| `test-token/` | A NEP-141 test stablecoin for testnet |
| `sandbox/` | End-to-end checks on a local NEAR node |
| `demo/`, `core/examples/` | Replays of real market data, natively and on testnet |
| `scripts/` | Testnet deployment |

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

The replays need Polymarket's trade data, which stays outside this repository, in `../aimm-data`. The scripts in
`demo/` fetch it, prepare it and run the replays natively, on a local node and on testnet; each says how to run it at
its top. All of it takes about two hours, most of it the on-chain run.

## Built with

| | |
|---|---|
| Contract and test token | [`near-sdk`](https://crates.io/crates/near-sdk) 5, [`near-contract-standards`](https://crates.io/crates/near-contract-standards) 5 |
| Local-node checks | [`near-workspaces`](https://crates.io/crates/near-workspaces) 0.24, `tokio`, `serde_json`, `anyhow` |
| Replay and testnet scripts | Python's standard library; [near-cli-rs](https://github.com/near/near-cli-rs) sends the testnet transactions |
| AI-assisted code | Written with [Claude Code](https://claude.com/claude-code) (Anthropic) under the team's direction; commits it worked on carry its co-author line |

## Hackathon

Everything in this repository was built for the NEAR Hackathon, inside its window of 19 October to 14 November 2026:
the first commit is from 3 November.

## Licence

No licence is granted. All rights reserved.
