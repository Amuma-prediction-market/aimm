//! Replays the real Polymarket histories as transactions on a local NEAR node (near-workspaces' sandbox, the real NEAR
//! runtime) and checks every number against the native run of `core/examples/replay.rs`:
//!   - each market is created and opened exactly as the native run opened it (depth, fund, prices);
//!   - every purchase is sent through the test stablecoin with `max_cost` equal to the cost and fee the native run
//!     computed, and every sale with `min_proceeds` equal to its proceeds, so the chain being off by one unit against
//!     the trader fails the trade; each event's cost or refund and fee must equal the native run's exactly, an event
//!     counts only from a receipt that went through, and after every trade the trader's token balance must have moved
//!     by exactly that;
//!   - every hour of market time, every price and both ends of every information interval must equal the native run's;
//!   - at the end the resolver reports the winner, and every redemption, the operator's surplus and the fees must be
//!     exact, in the event and in the balance of whoever is paid; the contract must hold only the native run's rounding
//!     dust, and the operator takes the storage balance back.
//! Six streams run in parallel: AIMM (ℓ = 0.3 L), AMM (ℓ = L, same money), AMM (ℓ = L, AIMM's depth), for each city.
//! About 11,800 transactions, about ninety minutes. The questions close AIMM_CLOSE_HOURS (default 6) after the start,
//! which must be longer than the replay takes; the clock is then moved past the close.
//!
//!     cargo build -p aimm-contract -p test-token --target wasm32-unknown-unknown --release
//!     cargo run -p aimm-sandbox -- ../aimm-data nyc london
//! Run the driver in the dev profile: the workspace's release profile is the wasm's, and LTO on this binary takes days.
use anyhow::{anyhow, bail, Result};
use near_workspaces::network::Sandbox;
use near_workspaces::result::ExecutionFinalResult;
use near_workspaces::{Account, Contract, Worker};
use near_workspaces::types::{Gas, NearToken};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Arc;

const MARKETS: [&str; 3] = ["AIMM l=0.3L", "AMM l=L same money", "AMM l=L AIMM depth"];

struct Env {
    worker: Worker<Sandbox>,
    aimm: Contract,
    ft: Contract,
    resolver: Account,
    close_time: u64,
}

/// The data of the contract's `name` event. A failed receipt keeps its logs, so only receipts that went through count.
fn event(res: &ExecutionFinalResult, name: &str) -> Option<Value> {
    res.outcomes().iter().filter(|o| !o.is_failure()).flat_map(|o| &o.logs).filter_map(|l| l.strip_prefix("EVENT_JSON:"))
        .filter_map(|l| serde_json::from_str::<Value>(l).ok()).find(|v| v["event"] == name).map(|v| v["data"][0].clone())
}

fn u(v: &Value) -> u128 {
    v.as_str().and_then(|s| s.parse().ok()).unwrap_or_else(|| panic!("not a U128: {v}"))
}

/// What `who` holds of the test stablecoin.
async fn cash(env: &Env, who: &Account) -> Result<u128> {
    Ok(u(&env.ft.view("ft_balance_of").args_json(json!({"account_id": who.id()})).await?.json()?))
}

/// The native run's rows for one market: time -> (price, lower, upper) per answer.
fn snapshots(csv: &str, market: &str) -> HashMap<u64, Vec<(f64, f64, f64)>> {
    let mut out: HashMap<u64, Vec<(f64, f64, f64)>> = HashMap::new();
    for line in csv.lines().skip(1) {
        let f: Vec<&str> = line.split(',').collect();
        if f[1] != market {
            continue;
        }
        out.entry(f[0].parse().unwrap()).or_default().push((f[4].parse().unwrap(), f[5].parse().unwrap(), f[6].parse().unwrap()));
    }
    out
}

struct Report {
    city: String,
    k: usize,
    operator: Account,
    traders: [Account; 2],
    name: String,
    question: u64,
    txs: usize,
    checked_snapshots: usize,
    first_tx: String,
    last_tx: String,
}

async fn stream(env: Arc<Env>, city: String, k: usize, ops: String, csv: String) -> Result<Report> {
    let name = format!("{city} / {}", MARKETS[k]);
    let native_market = ["AIMM", "AMM", "AMM full"][k];
    let snaps = snapshots(&csv, native_market);
    let mut times: Vec<u64> = snaps.keys().copied().collect();
    times.sort();
    let hourly: Vec<u64> = times.iter().copied().step_by(60).chain(times.last().copied()).collect();
    let operator = env.worker.dev_create_account().await?;
    let traders = [env.worker.dev_create_account().await?, env.worker.dev_create_account().await?]; // real traders, informed trader
    for a in [&operator, &traders[0], &traders[1]] {
        a.call(env.ft.id(), "storage_deposit").args_json(json!({})).deposit(NearToken::from_millinear(10)).transact().await?.into_result()?;
        a.call(env.ft.id(), "faucet").args_json(json!({"amount": "1000000000000000"})).transact().await?.into_result()?;
    }
    let lines: Vec<Vec<&str>> = ops.lines().map(|l| l.split_whitespace().collect()).collect();
    let me = k.to_string();
    let open = lines.iter().find(|f| f[0] == "open" && f[1] == me).ok_or(anyhow!("no open line"))?;
    let n = open.len() - 5;
    let answers: Vec<String> = (0..n).map(|i| format!("range {i}")).collect();
    let res = operator.call(env.aimm.id(), "create_question")
        .args_json(json!({"args": {"collateral": env.ft.id(), "title": format!("{name} (replay)"), "answers": answers,
            "resolver": env.resolver.id(), "fallback_resolver": env.resolver.id(), "close_time": env.close_time.to_string(),
            "resolve_after": env.close_time.to_string(), "fallback_after": env.close_time.to_string(), "fee_rate": open[4]}}))
        .deposit(NearToken::from_millinear(500)).max_gas().transact().await?;
    let qid: u64 = res.clone().into_result()?.json()?;
    let msg = json!({"action": "open", "question_id": qid, "depth": open[2], "prices": open[5..]}).to_string();
    let res = operator.call(env.ft.id(), "ft_transfer_call")
        .args_json(json!({"receiver_id": env.aimm.id(), "amount": open[3], "msg": msg}))
        .deposit(NearToken::from_yoctonear(1)).max_gas().transact().await?;
    event(&res, "open").ok_or_else(|| anyhow!("{name}: open failed: {:?}", res.logs()))?;
    let mut purse = [cash(&env, &traders[0]).await?, cash(&env, &traders[1]).await?];
    let mut txs = 2;
    let mut first_tx = String::new();
    let mut last_tx = String::new();
    let mut checked = 0;
    let mut next_check = 0;
    let check = |q: Value, t: u64| -> Result<()> {
        let want = &snaps[&t];
        for (i, (p, lo, hi)) in want.iter().enumerate() {
            let got = (u(&q["prices"][i]) as f64 / 1e18, u(&q["intervals"][i][0]) as f64 / 1e18, u(&q["intervals"][i][1]) as f64 / 1e18);
            if got != (*p, *lo, *hi) {
                bail!("{name}: at {t}, answer {i}: chain {got:?}, native {:?}", (p, lo, hi));
            }
        }
        Ok(())
    };
    // operation lines: <market> trade|arb|trade_sell|arb_sell <time> <range> <yes> <tickets> <cost or refund> <fee>
    for f in lines.iter().filter(|f| f[0] == me) {
        let (kind, t): (&str, u64) = (f[1], f[2].parse()?);
        let real = kind.starts_with("trade");
        // before an operation stamped after the next hourly check, compare the chain with the native snapshot
        while next_check < hourly.len() && (t > hourly[next_check] || (real && t == hourly[next_check])) {
            let q: Value = env.aimm.view("get_question").args_json(json!({"question_id": qid})).await?.json()?;
            check(q, hourly[next_check])?;
            checked += 1;
            next_check += 1;
        }
        let who = if real { 0 } else { 1 };
        let actor = &traders[who];
        let (range, yes, tickets, cost, fee) = (f[3], f[4] == "1", f[5], f[6], f[7]);
        let (amount, cut) = (cost.parse::<u128>()?, fee.parse::<u128>()?);
        let sale = kind.ends_with("_sell");
        let res = if sale {
            let res = actor.call(env.aimm.id(), "sell")
                .args_json(json!({"question_id": qid, "range": range.parse::<u8>()?, "yes": yes, "tickets": tickets, "min_proceeds": (amount - cut).to_string()}))
                .deposit(NearToken::from_yoctonear(1)).gas(Gas::from_tgas(100)).transact().await?;
            let ev = event(&res, "sell").ok_or_else(|| anyhow!("{name}: sale at {t} failed on chain: {:?}", res.logs()))?;
            if ev["refund"].as_str() != Some(cost) || ev["fee"].as_str() != Some(fee) {
                bail!("{name}: sale at {t}: chain refund {} fee {}, native {cost} {fee}", ev["refund"], ev["fee"]);
            }
            res
        } else {
            let total = (amount + cut).to_string();
            let msg = json!({"action": "buy", "question_id": qid, "range": range.parse::<u8>()?, "yes": yes, "tickets": tickets, "max_cost": total}).to_string();
            let res = actor.call(env.ft.id(), "ft_transfer_call")
                .args_json(json!({"receiver_id": env.aimm.id(), "amount": total, "msg": msg}))
                .deposit(NearToken::from_yoctonear(1)).gas(Gas::from_tgas(100)).transact().await?;
            let ev = event(&res, "buy").ok_or_else(|| anyhow!("{name}: purchase at {t} failed on chain: {:?}", res.logs()))?;
            if ev["cost"].as_str() != Some(cost) || ev["fee"].as_str() != Some(fee) {
                bail!("{name}: purchase at {t}: chain cost {} fee {}, native {cost} {fee}", ev["cost"], ev["fee"]);
            }
            res
        };
        let want = if sale { purse[who] + (amount - cut) } else { purse[who] - (amount + cut) };
        purse[who] = cash(&env, actor).await?;
        if purse[who] != want {
            bail!("{name}: at {t} the trader holds {}, not {want}", purse[who]);
        }
        let hash = format!("{}", res.outcome().transaction_hash);
        if first_tx.is_empty() {
            first_tx = hash.clone();
        }
        last_tx = hash;
        txs += 1;
        if txs % 200 == 0 {
            println!("{name}: {txs} transactions, market time {t}");
        }
    }
    while next_check < hourly.len() {
        let q: Value = env.aimm.view("get_question").args_json(json!({"question_id": qid})).await?.json()?;
        check(q, hourly[next_check])?;
        checked += 1;
        next_check += 1;
    }
    Ok(Report { city, k, operator, traders, name, question: qid, txs, checked_snapshots: checked, first_tx, last_tx })
}

/// Reports the winner, redeems both traders and claims the operator's surplus and fees, each checked against the
/// native run's `settle` line: settle <market> <winner> <payout> <redeem real> <redeem informed> <surplus> <fees> <dust>.
/// Then the operator takes the question's storage balance back.
async fn settle(env: &Env, r: &Report, ops: &str) -> Result<String> {
    let f: Vec<&str> = ops.lines().map(|l| l.split_whitespace().collect::<Vec<_>>())
        .find(|f| f[0] == "settle" && f[1] == r.k.to_string()).ok_or(anyhow!("no settle line"))?;
    let (winner, payout, real, informed, surplus, fees) = (f[2].parse::<u8>()?, f[3], f[4], f[5], f[6], f[7]);
    let res = env.resolver.call(env.aimm.id(), "resolve").args_json(json!({"question_id": r.question, "winner": winner}))
        .deposit(NearToken::from_yoctonear(1)).max_gas().transact().await?;
    let ev = event(&res, "resolve").ok_or_else(|| anyhow!("{}: resolve failed: {:?}", r.name, res.clone().into_result().err()))?;
    if ev["payout"].as_str() != Some(payout) || ev["surplus"].as_str() != Some(surplus) {
        bail!("{}: resolve: chain payout {} surplus {}, native {payout} {surplus}", r.name, ev["payout"], ev["surplus"]);
    }
    let mut got = Vec::new();
    for (a, want) in [(&r.traders[0], real), (&r.traders[1], informed)] {
        if want == "0" { got.push("0".to_string()); continue; }
        let had = cash(env, a).await?;
        let res = a.call(env.aimm.id(), "redeem").args_json(json!({"question_id": r.question})).deposit(NearToken::from_yoctonear(1)).max_gas().transact().await?;
        let ev = event(&res, "redeem").ok_or_else(|| anyhow!("{}: redeem failed", r.name))?;
        if ev["amount"].as_str() != Some(want) || cash(env, a).await? != had + want.parse::<u128>()? {
            bail!("{}: redeem: chain {}, native {want}, the holder has {}", r.name, ev["amount"], cash(env, a).await?);
        }
        got.push(want.to_string());
    }
    let had = cash(env, &r.operator).await?;
    let res = r.operator.call(env.aimm.id(), "claim_surplus").args_json(json!({"question_id": r.question})).deposit(NearToken::from_yoctonear(1)).max_gas().transact().await?;
    let ev = event(&res, "claim_surplus").ok_or_else(|| anyhow!("{}: claim_surplus failed", r.name))?;
    if ev["amount"].as_str() != Some(surplus) {
        bail!("{}: surplus: chain {}, native {surplus}", r.name, ev["amount"]);
    }
    let res = r.operator.call(env.aimm.id(), "claim_fees").args_json(json!({"question_id": r.question})).deposit(NearToken::from_yoctonear(1)).max_gas().transact().await?;
    let ev = event(&res, "claim_fees").ok_or_else(|| anyhow!("{}: claim_fees failed", r.name))?;
    if ev["amount"].as_str() != Some(fees) {
        bail!("{}: fees: chain {}, native {fees}", r.name, ev["amount"]);
    }
    if cash(env, &r.operator).await? != had + surplus.parse::<u128>()? + fees.parse::<u128>()? {
        bail!("{}: the operator was not paid {surplus} and {fees}", r.name);
    }
    r.operator.call(env.aimm.id(), "withdraw_storage").args_json(json!({"question_id": r.question})).deposit(NearToken::from_yoctonear(1)).max_gas().transact().await?.into_result()?;
    Ok(format!("winner {winner}, payout {}, redeemed {} and {}, operator surplus {surplus}, fees {fees}", payout.parse::<f64>()? / 1e18, got[0], got[1]))
}

#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let data = args.get(1).cloned().unwrap_or_else(|| "../aimm-data".into());
    let cities: Vec<String> = if args.len() > 2 { args[2..].to_vec() } else { vec!["nyc".into(), "london".into()] };
    let root = env!("CARGO_MANIFEST_DIR");
    let aimm_wasm = std::fs::read(format!("{root}/../target/wasm32-unknown-unknown/release/aimm_contract.wasm"))?;
    let ft_wasm = std::fs::read(format!("{root}/../target/wasm32-unknown-unknown/release/test_token.wasm"))?;
    let worker = near_workspaces::sandbox().await?;
    let aimm = worker.dev_deploy(&aimm_wasm).await?;
    aimm.call("new").max_gas().transact().await?.into_result()?;
    let ft = worker.dev_deploy(&ft_wasm).await?;
    ft.call("new").max_gas().transact().await?.into_result()?;
    let resolver = worker.dev_create_account().await?;
    resolver.call(ft.id(), "storage_deposit").args_json(json!({"account_id": aimm.id()})).deposit(NearToken::from_millinear(100)).transact().await?.into_result()?;
    let now = worker.view_block().await?.timestamp();
    let hours: u64 = std::env::var("AIMM_CLOSE_HOURS").ok().map(|h| h.parse()).transpose()?.unwrap_or(6);
    let close_time = now + hours * 3_600_000_000_000;
    println!("sandbox up: AIMM at {}, test stablecoin at {}", aimm.id(), ft.id());
    let env = Arc::new(Env { worker, aimm, ft, resolver, close_time });
    let mut tasks = Vec::new();
    for city in &cities {
        let ops = std::fs::read_to_string(format!("{data}/{city}.ops"))?;
        let csv = std::fs::read_to_string(format!("{data}/{city}.csv"))?;
        for k in 0..3 {
            tasks.push(tokio::spawn(stream(env.clone(), city.clone(), k, ops.clone(), csv.clone())));
        }
    }
    let mut ok = true;
    let mut done = Vec::new();
    let mut dust: u128 = 0;
    for t in tasks {
        match t.await? {
            Ok(r) => {
                println!("{:<36} question {:>2}: {:>5} transactions, every cost exact; {:>2} hourly snapshots equal to the native run; first tx {}, last tx {}",
                    r.name, r.question, r.txs, r.checked_snapshots, r.first_tx, r.last_tx);
                done.push(r);
            }
            Err(e) => { ok = false; println!("FAILED: {e}"); }
        }
    }
    // move the chain's clock past the close, then settle every question
    loop {
        let now = env.worker.view_block().await?.timestamp();
        if now > env.close_time {
            println!("clock moved past the close: block time now {now}");
            break;
        }
        env.worker.fast_forward((env.close_time - now) / 1_000_000_000 + 100).await?;
    }
    for r in &done {
        let ops = std::fs::read_to_string(format!("{data}/{}.ops", r.city))?;
        let line: Vec<&str> = ops.lines().find(|l| l.starts_with(&format!("settle {} ", r.k))).ok_or(anyhow!("no settle line"))?.split_whitespace().collect();
        dust += line[8].parse::<u128>()?;
        match settle(&env, r, &ops).await {
            Ok(s) => println!("{:<36} settled exactly as the native run: {s}", r.name),
            Err(e) => { ok = false; println!("FAILED: {e}"); }
        }
    }
    let left: Value = env.ft.view("ft_balance_of").args_json(json!({"account_id": env.aimm.id()})).await?.json()?;
    println!("the contract holds {} units of the collateral after everyone was paid; the native run left {dust} as rounding dust", left.as_str().unwrap_or("?"));
    if !ok || done.len() != cities.len() * 3 || left.as_str() != Some(dust.to_string().as_str()) {
        bail!("the chain disagreed with the native run");
    }
    println!("all checks passed");
    Ok(())
}
