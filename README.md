# AMUMA - A New Prediction Market Mechanism: Automated Information Market Maker (AIMM)

**Live demo: [amuma-cv4.pages.dev](https://amuma-cv4.pages.dev)**

Current prediction market architectures - AMMs, orderbooks, and parimutuel pools - force a strict trade-off between capital efficiency and pricing accuracy:

* Orderbooks and AMMs deliver precise point-estimate signals (singleton range) but demand deep liquidity, making them prohibitively expensive for illiquid, niche markets.
* Parimutuel pools minimize capital requirements but offer virtually no informational accuracy (uninformative range of [0,1]), as informed traders withhold capital until the final moments to avoid leaking alpha.

This structural dilemma leaves long-tail, specialized markets underserved. High-stakes niche information - such as micro-climate wind forecasts that shift millions in utility profits - remains unviable on existing platforms.

AIMM resolves this tension by replacing rigid point estimates with dynamic probability range. Market operators tune capital efficiency against signal resolution: wider ranges require less liquidity, while tighter ranges yield higher precision.

By slashing the capital threshold needed to bootstrap specialty markets, AIMM unlocks a vast array of niche real-world prediction markets for signal discovery, enterprise hedging, consumer decisions, and autonomous agents.

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
