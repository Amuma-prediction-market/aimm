# AIMM: Automated Information Market Maker

**Live demo: [amuma-cv4.pages.dev](https://amuma-cv4.pages.dev)**

Information is not free. Capital is required to get good information in prediction markets. Operators have to choose between the two evils of low pricing accuracy vs capital inefficiency.
Parimutuel pools fail to provide reliable information and force informed traders to wait until the last moments to place their bet to avoid revealing information.
AMM and orderbooks can provide accurate signals but require heavy capital - more so for illiquid specialty markets.
This is the fundamental trade off between capital requirement and prediction accuracy: AMM/orderbook and parimutuel are at two ends of the spectrum.
Existing prediction markets are not optimal for specialty longtail markets such as wind speeds around a wind farm. Such information often triggers millions in profits or losses for utilities but no existing prediction market can accurately and efficiently serve it.
We propose a new mechanism: Automated Information Market Maker (AIMM). Which does not seek for a point estimation but to form a prediction range around the price: a trader whose belief lies outside it expects to gain by trading (whitepaper, Proposition 3), so its length measures how far the price can be trusted.

Whitepaper: [`whitepaper/aimm.pdf`](whitepaper/aimm.pdf).

## Live

The contract runs on NEAR testnet as
[`market.aimm-weather.testnet`](https://testnet.nearblocks.io/address/market.aimm-weather.testnet). The site is
[amuma-cv4.pages.dev](https://amuma-cv4.pages.dev), and its code is in
[Amuma-prediction-market/app](https://github.com/Amuma-prediction-market/app).

## Repository layout

| Component | Description |
|---|---|
| `SPEC.md` | The exact rules of the engine and the contract |
| `core/` | The AIMM engine in fixed-point Rust: curve, pot, payout and prediction range |
| `core/tests/` | Reference-model, property and limit tests |
| `contract/` | The NEAR smart contract, serving every question from one account |
| `scripts/` | Testnet deployment |

## Run it

Requires Rust stable and the `wasm32-unknown-unknown` target.

```bash
cargo test
```

```bash
RUSTFLAGS="--cfg near --remap-path-prefix=$HOME=~" cargo build -p aimm-contract --target wasm32-unknown-unknown --release
```

RUSTFLAGS from the environment replaces the rustflags of `.cargo/config.toml`, so the command repeats its `--cfg near`.
The remap keeps the builder's home folder out of the panic messages in the wasm. `scripts/testnet.sh` builds the same
way.

## Built with

| | |
|---|---|
| Contract | [`near-sdk`](https://crates.io/crates/near-sdk) 5 |
| Testnet script | [near-cli-rs](https://github.com/near/near-cli-rs) sends the transactions |
| AI-assisted code | Written with [Claude Code](https://claude.com/claude-code) (Anthropic) under the team's direction; commits it worked on carry its co-author line |

## Hackathon

This repository is our entry to the NEAR Hackathon, whose development window is 1 to 14 November 2026. The AIMM
mechanism and its whitepaper came before it. The Rust engine, the NEAR contract, the deploy script and the site were
written during the hackathon, from 3 November 2026.

## Licence

Business Source License 1.1; see [`LICENSE`](LICENSE). Anyone may read, copy, modify and test the code, but production
use needs a commercial licence from the Amuma Team. By 14 November 2030 at the latest the code becomes available under
the GNU General Public License v2.0 or later.
