//! Proves on the real NEAR runtime (near-workspaces' sandbox) what the replay cannot: gas at 16 answers, a short pot,
//! fees, a failed payout, every refusal, and the storage attacks. Fifteen claims, one PASS or FAIL line each, on numbers
//! read back from the chain. The contract's account gives away its spare NEAR first, so its free
//! NEAR is the questions' storage balances and a sliver more: any byte a question did not pay for would fail on chain.
//!
//!     cargo build --offline -p aimm-contract -p test-token --target wasm32-unknown-unknown --release
//!     cargo run --offline -p aimm-sandbox --bin scenarios
//! Run it in the dev profile: the workspace's release profile is the wasm's, and LTO on this binary takes days.
use anyhow::{anyhow, bail, ensure, Result};
use near_workspaces::network::Sandbox;
use near_workspaces::result::ExecutionFinalResult;
use near_workspaces::types::{Gas, KeyType, NearToken, SecretKey};
use near_workspaces::{AccessKey, Account, AccountId, Contract, Worker};
use serde_json::{json, Value};
use std::cell::Cell;
use std::time::{Duration, Instant};

const WAD: u128 = 1_000_000_000_000_000_000;
const ONE: u128 = 1_000_000; // one tUSD
const DEPTH: u128 = 1_000 * ONE;
const BYTE_COST: u128 = 10_000_000_000_000_000_000; // yoctoNEAR locked per byte of state
const MILLI: u128 = 1_000_000_000_000_000_000_000; // a thousandth of a NEAR
const TRADING: u64 = 180_000_000_000; // a question closes three minutes after it is created
const SPARE: u128 = MILLI; // what the contract's account keeps beyond its state, less the gas of giving the rest away

/// What a holder takes at 16 answers with a 64-letter name.
const HOLDER_ROW: u128 = (1 + 8 + 4 + 64 + 4 + 32 * 16 + 40) * BYTE_COST;

fn u(v: &Value) -> u128 {
    v.as_str().and_then(|s| s.parse().ok()).unwrap_or_else(|| panic!("not a U128: {v}"))
}

/// Why a transaction failed: the failure of each receipt that did.
fn why(res: &ExecutionFinalResult) -> String {
    let errors = res.failures().into_iter().filter_map(|o| o.clone().into_result().err());
    errors.map(|e| format!("{e:?}")).collect::<Vec<_>>().join("; ")
}

/// What the contract said when it panicked.
fn said(reason: &str) -> &str {
    reason.split("Smart contract panicked: ").nth(1).and_then(|s| s.split('"').next()).unwrap_or(reason)
}

fn refused(res: &ExecutionFinalResult, text: &str) -> Result<()> {
    let reason = why(res);
    ensure!(reason.contains(text), "expected a refusal with {text:?}, got {reason:?}");
    Ok(())
}

/// The data of the contract's `name` event, or why there is none.
/// A failed receipt keeps its logs, so only receipts that went through count.
fn event(res: &ExecutionFinalResult, name: &str) -> Result<Value> {
    res.outcomes().iter().filter(|o| !o.is_failure()).flat_map(|o| &o.logs).filter_map(|l| l.strip_prefix("EVENT_JSON:"))
        .filter_map(|l| serde_json::from_str::<Value>(l).ok()).find(|v| v["event"] == name).map(|v| v["data"][0].clone())
        .ok_or_else(|| anyhow!("no {name} event: {}", why(res)))
}

fn tgas(gas: u64) -> String {
    format!("{:.2} Tgas", gas as f64 / 1e12)
}

fn near(yocto: u128) -> String {
    format!("{:.6} NEAR", yocto as f64 / 1e24)
}

/// Equal opening prices that add up to exactly 1.
fn even(n: usize) -> Vec<u128> {
    let each = WAD / n as u128;
    let mut prices = vec![each; n];
    prices[0] += WAD - each * n as u128;
    prices
}

fn open_msg(question: u64, depth: u128, prices: &[u128]) -> String {
    let prices: Vec<String> = prices.iter().map(u128::to_string).collect();
    json!({"action": "open", "question_id": question, "depth": depth.to_string(), "prices": prices}).to_string()
}

fn buy_msg(question: u64, range: usize, yes: bool, tickets: u128, max_cost: u128) -> String {
    json!({"action": "buy", "question_id": question, "range": range, "yes": yes, "tickets": tickets.to_string(), "max_cost": max_cost.to_string()}).to_string()
}

/// A 64-letter account name no one has used.
fn fresh_name(question: u64, i: u128) -> AccountId {
    format!("{question:032x}{i:032x}").parse().expect("64 hex letters are an account name")
}

struct Chain {
    worker: Worker<Sandbox>,
    aimm: Contract,
    ft: Contract,
    ft_wasm: Vec<u8>,
    resolver: Account,
    fast_forwarding: Cell<Duration>,
}

struct Market {
    id: u64,
    ft: AccountId,
    operator: Account,
    close: u64,
}

impl Chain {
    async fn now(&self) -> Result<u64> {
        Ok(self.worker.view_block().await?.timestamp())
    }

    /// Moves the chain's clock past `t`.
    async fn past(&self, t: u64) -> Result<()> {
        let start = Instant::now();
        while self.now().await? <= t {
            self.worker.fast_forward(200).await?;
        }
        self.fast_forwarding.set(self.fast_forwarding.get() + start.elapsed());
        Ok(())
    }

    /// A new token the contract can hold, so its balance there is this claim's alone.
    async fn token(&self) -> Result<Contract> {
        let ft = self.worker.dev_deploy(&self.ft_wasm).await?;
        ft.call("new").max_gas().transact().await?.into_result()?;
        self.resolver.call(ft.id(), "storage_deposit").args_json(json!({"account_id": self.aimm.id()}))
            .deposit(NearToken::from_millinear(100)).transact().await?.into_result()?;
        Ok(ft)
    }

    /// A new account registered with the token and holding `money`.
    async fn holder(&self, ft: &AccountId, money: u128) -> Result<Account> {
        let a = self.worker.dev_create_account().await?;
        a.call(ft, "storage_deposit").args_json(json!({})).deposit(NearToken::from_millinear(10)).transact().await?.into_result()?;
        a.call(ft, "faucet").args_json(json!({"amount": money.to_string()})).transact().await?.into_result()?;
        Ok(a)
    }

    /// Creates a question that closes soon, and returns it with its close time.
    async fn create(&self, operator: &Account, collateral: &AccountId, n: usize, fee: u128, deposit: u128) -> Result<(u64, u64)> {
        let close = self.now().await? + TRADING;
        let res = self.create_call(operator, collateral, n, fee, deposit, close).await?;
        Ok((res.json()?, close))
    }

    async fn create_call(&self, operator: &Account, collateral: &AccountId, n: usize, fee: u128, deposit: u128, close: u64) -> Result<ExecutionFinalResult> {
        let answers: Vec<String> = (0..n).map(|i| format!("answer {i}")).collect();
        Ok(operator.call(self.aimm.id(), "create_question")
            .args_json(json!({"args": {"collateral": collateral, "title": "scenario", "answers": answers,
                "resolver": self.resolver.id(), "fallback_resolver": self.resolver.id(), "close_time": close.to_string(),
                "resolve_after": close.to_string(), "fallback_after": close.to_string(), "fee_rate": fee.to_string()}}))
            .deposit(NearToken::from_yoctonear(deposit)).max_gas().transact().await?)
    }

    async fn market_with(&self, ft: &AccountId, n: usize, fee: u128, fund: u128, deposit: u128) -> Result<Market> {
        let operator = self.holder(ft, fund).await?;
        let (id, close) = self.create(&operator, ft, n, fee, deposit).await?;
        let res = operator.call(ft, "ft_transfer_call")
            .args_json(json!({"receiver_id": self.aimm.id(), "amount": fund.to_string(), "msg": open_msg(id, DEPTH, &even(n))}))
            .deposit(NearToken::from_yoctonear(1)).max_gas().transact().await?;
        event(&res, "open")?;
        Ok(Market { id, ft: ft.clone(), operator, close })
    }

    /// A question opened by a new operator with `fund`, at equal prices, with half a NEAR for storage.
    async fn market(&self, ft: &AccountId, n: usize, fee: u128, fund: u128) -> Result<Market> {
        self.market_with(ft, n, fee, fund, 500 * MILLI).await
    }

    /// A purchase through the token, sending `amount`. A refusal shows in the result.
    async fn send(&self, m: &Market, who: &Account, range: usize, yes: bool, tickets: u128, amount: u128, max_cost: u128) -> Result<ExecutionFinalResult> {
        Ok(who.call(&m.ft, "ft_transfer_call")
            .args_json(json!({"receiver_id": self.aimm.id(), "amount": amount.to_string(), "msg": buy_msg(m.id, range, yes, tickets, max_cost)}))
            .deposit(NearToken::from_yoctonear(1)).gas(Gas::from_tgas(100)).transact().await?)
    }

    /// A purchase at the quoted price, which must go through.
    async fn buy_tx(&self, m: &Market, who: &Account, range: usize, yes: bool, tickets: u128) -> Result<ExecutionFinalResult> {
        let total = u(&self.quote(m, "quote_buy", range, yes, tickets).await?["total"]);
        let res = self.send(m, who, range, yes, tickets, total, total).await?;
        event(&res, "buy")?;
        Ok(res)
    }

    async fn buy(&self, m: &Market, who: &Account, range: usize, yes: bool, tickets: u128) -> Result<Value> {
        event(&self.buy_tx(m, who, range, yes, tickets).await?, "buy")
    }

    async fn sell(&self, m: &Market, who: &Account, range: usize, yes: bool, tickets: u128) -> Result<ExecutionFinalResult> {
        self.yocto(who, "sell", json!({"question_id": m.id, "range": range, "yes": yes, "tickets": tickets.to_string(), "min_proceeds": "0"})).await
    }

    /// Moves one answer's YES tokens, attaching `deposit`.
    async fn transfer(&self, m: u64, who: &Account, to: &AccountId, tickets: u128, deposit: u128) -> Result<ExecutionFinalResult> {
        Ok(who.call(self.aimm.id(), "mt_transfer").args_json(json!({"receiver_id": to, "token_id": format!("{m}:yes:0"), "amount": tickets.to_string()}))
            .deposit(NearToken::from_yoctonear(deposit)).gas(Gas::from_tgas(100)).transact().await?)
    }

    /// `who` plays the token of its own question: it tells the contract `sender` paid, and no money exists.
    async fn as_token_for(&self, who: &Account, sender: &AccountId, msg: String) -> Result<ExecutionFinalResult> {
        Ok(who.call(self.aimm.id(), "ft_on_transfer").args_json(json!({"sender_id": sender, "amount": "1000000", "msg": msg})).gas(Gas::from_tgas(100)).transact().await?)
    }

    async fn as_token(&self, who: &Account, msg: String) -> Result<ExecutionFinalResult> {
        self.as_token_for(who, who.id(), msg).await
    }

    /// Calls a method of the contract that takes 1 yoctoNEAR.
    async fn yocto(&self, who: &Account, method: &str, args: Value) -> Result<ExecutionFinalResult> {
        Ok(who.call(self.aimm.id(), method).args_json(args).deposit(NearToken::from_yoctonear(1)).gas(Gas::from_tgas(100)).transact().await?)
    }

    async fn resolve(&self, m: &Market, winner: u8) -> Result<ExecutionFinalResult> {
        self.yocto(&self.resolver, "resolve", json!({"question_id": m.id, "winner": winner})).await
    }

    async fn q(&self, id: u64) -> Result<Value> {
        Ok(self.aimm.view("get_question").args_json(json!({"question_id": id})).await?.json()?)
    }

    async fn storage(&self, id: u64) -> Result<u128> {
        Ok(u(&self.q(id).await?["storage"]))
    }

    async fn quote(&self, m: &Market, method: &str, range: usize, yes: bool, tickets: u128) -> Result<Value> {
        Ok(self.aimm.view(method).args_json(json!({"question_id": m.id, "range": range, "yes": yes, "tickets": tickets.to_string()})).await?.json()?)
    }

    async fn holdings(&self, id: u64, who: &AccountId) -> Result<Value> {
        Ok(self.aimm.view("holdings").args_json(json!({"question_id": id, "account_id": who})).await?.json()?)
    }

    async fn pending(&self, id: u64, who: &AccountId) -> Result<u128> {
        Ok(u(&self.aimm.view("pending_of").args_json(json!({"question_id": id, "account_id": who})).await?.json()?))
    }

    async fn balance(&self, ft: &AccountId, who: &AccountId) -> Result<u128> {
        Ok(u(&self.worker.view(ft, "ft_balance_of").args_json(json!({"account_id": who})).await?.json()?))
    }

    /// The contract's free NEAR, its balance less what its state locks, and the sum of every question's storage balance.
    async fn free_near(&self) -> Result<(u128, u128)> {
        let a = self.aimm.view_account().await?;
        let free = a.balance.as_yoctonear() - a.storage_usage as u128 * BYTE_COST;
        let count: u64 = self.aimm.view("question_count").await?.json()?;
        let mut live = 0;
        for id in 1..=count {
            live += self.storage(id).await?;
        }
        Ok((free, live))
    }
}

async fn sixteen_answers(c: &Chain) -> Result<String> {
    let m = c.market(c.ft.id(), 16, WAD / 50, 500 * ONE).await?;
    let t = c.holder(c.ft.id(), 100_000 * ONE).await?;
    ensure!(c.q(m.id).await?["prices"].as_array().map(Vec::len) == Some(16), "it did not open with 16 prices");
    let buy_yes = c.buy_tx(&m, &t, 0, true, 100 * ONE).await?.total_gas_burnt.as_gas();
    let mut buy_no = 0;
    for range in 1..16 {
        buy_no = buy_no.max(c.buy_tx(&m, &t, range, false, 10 * ONE).await?.total_gas_burnt.as_gas());
    }
    let sell_yes = c.sell(&m, &t, 0, true, 50 * ONE).await?;
    event(&sell_yes, "sell")?;
    let sell_no = c.sell(&m, &t, 1, false, 5 * ONE).await?;
    event(&sell_no, "sell")?;
    c.past(m.close).await?;
    event(&c.resolve(&m, 0).await?, "resolve")?;
    let redeem = c.yocto(&t, "redeem", json!({"question_id": m.id})).await?;
    event(&redeem, "redeem")?;
    Ok(format!("gas burnt in all: buy YES {}, buy NO {} at most, sell YES {}, sell NO {}, redeem on 16 positions {}",
        tgas(buy_yes), tgas(buy_no), tgas(sell_yes.total_gas_burnt.as_gas()), tgas(sell_no.total_gas_burnt.as_gas()), tgas(redeem.total_gas_burnt.as_gas())))
}

async fn short_pot(c: &Chain) -> Result<String> {
    let ft = c.token().await?;
    let m = c.market(ft.id(), 3, 0, 100 * ONE).await?;
    let (alice, bob) = (c.holder(ft.id(), 100_000 * ONE).await?, c.holder(ft.id(), 100_000 * ONE).await?);
    c.buy(&m, &alice, 0, true, 5_000 * ONE + 7).await?;
    c.buy(&m, &bob, 1, false, 1_000 * ONE).await?;
    let tickets = 500 * ONE;
    let mut refunds = Vec::new();
    for (who, range, yes) in [(&alice, 0, true), (&bob, 1, false)] {
        let r = u(&c.q(m.id).await?["payouts"][0]);
        ensure!(r < WAD, "the pot is not short");
        let after = u(&c.quote(&m, "quote_sell", range, yes, tickets).await?["prices_after"][range]);
        let curve_at_least = tickets * if yes { after } else { WAD - after } / WAD;
        let had = c.balance(ft.id(), who.id()).await?;
        let refund = u(&event(&c.sell(&m, who, range, yes, tickets).await?, "sell")?["refund"]);
        ensure!(refund == tickets * r / WAD, "refund {refund}, tickets times payout {}", tickets * r / WAD);
        ensure!(refund < curve_at_least, "the cap did not bind: refund {refund}, curve at least {curve_at_least}");
        ensure!(c.balance(ft.id(), who.id()).await? == had + refund, "the seller was not paid {refund}");
        ensure!(u(&c.q(m.id).await?["payouts"][0]) >= r, "the sale lowered the payout");
        refunds.push(refund);
    }
    c.past(m.close).await?;
    let q = c.q(m.id).await?;
    let pot = u(&q["pot"]);
    let winning = u(&q["yes_supply"][0]) + u(&q["no_supply"][1]) + u(&q["no_supply"][2]);
    let payout = pot * WAD / winning;
    let keep = (winning * payout).div_ceil(WAD);
    let ev = event(&c.resolve(&m, 0).await?, "resolve")?;
    ensure!(u(&ev["payout"]) == payout && payout < WAD, "payout {}, pot over winning tokens {payout}", ev["payout"]);
    ensure!(u(&ev["surplus"]) == pot - keep, "surplus {}, expected {}", ev["surplus"], pot - keep);
    let mut paid = Vec::new();
    for (who, tokens) in [(&alice, 4_500 * ONE + 7), (&bob, 500 * ONE)] {
        let want = tokens * payout / WAD;
        let had = c.balance(ft.id(), who.id()).await?;
        let got = u(&event(&c.yocto(who, "redeem", json!({"question_id": m.id})).await?, "redeem")?["amount"]);
        ensure!(got == want && c.balance(ft.id(), who.id()).await? == had + want, "redeemed {got}, floor(tokens times payout) {want}");
        paid.push(want);
    }
    event(&c.yocto(&m.operator, "claim_surplus", json!({"question_id": m.id})).await?, "claim_surplus")?;
    let dust = keep - paid.iter().sum::<u128>();
    let left = c.balance(ft.id(), c.aimm.id()).await?;
    ensure!(dust > 0, "no rounding dust to check");
    ensure!(left == dust && u(&c.q(m.id).await?["pot"]) == dust, "the contract holds {left}, the rounding dust is {dust}");
    Ok(format!("capped refunds {} and {} of {tickets} tickets; payout {}; redeemed {} and {}; surplus {}; the contract holds {left} units, the dust",
        refunds[0], refunds[1], payout as f64 / 1e18, paid[0], paid[1], pot - keep))
}

async fn fees(c: &Chain) -> Result<String> {
    let ft = c.token().await?;
    let rate = WAD / 50;
    let m = c.market(ft.id(), 5, rate, 1_000 * ONE).await?;
    let (t, stranger) = (c.holder(ft.id(), 10_000 * ONE).await?, c.worker.dev_create_account().await?);
    let bought = c.buy(&m, &t, 2, true, 300 * ONE).await?;
    let sold = event(&c.sell(&m, &t, 2, true, 100 * ONE).await?, "sell")?;
    let (cost, refund) = (u(&bought["cost"]), u(&sold["refund"]));
    let (fee_in, fee_out) = (u(&bought["fee"]), u(&sold["fee"]));
    ensure!(fee_in == (cost * rate).div_ceil(WAD) && fee_out == (refund * rate).div_ceil(WAD), "fees {fee_in} and {fee_out} are not the rate on {cost} and {refund}");
    ensure!(c.balance(ft.id(), t.id()).await? == 10_000 * ONE - cost - fee_in + refund - fee_out, "the trader's balance is off");
    let owed = fee_in + fee_out;
    ensure!(u(&c.q(m.id).await?["fees_owed"]) == owed, "fees_owed is not {owed}");
    refused(&c.yocto(&stranger, "claim_fees", json!({"question_id": m.id})).await?, "AIMM: only the operator")?;
    let had = c.balance(ft.id(), m.operator.id()).await?;
    let ev = event(&c.yocto(&m.operator, "claim_fees", json!({"question_id": m.id})).await?, "claim_fees")?;
    ensure!(u(&ev["amount"]) == owed && c.balance(ft.id(), m.operator.id()).await? == had + owed, "claim_fees did not pay exactly {owed}");
    let again = u(&event(&c.yocto(&m.operator, "claim_fees", json!({"question_id": m.id})).await?, "claim_fees")?["amount"]);
    let q = c.q(m.id).await?;
    let left = c.balance(ft.id(), c.aimm.id()).await?;
    ensure!(again == 0 && u(&q["fees_owed"]) == 0 && left == u(&q["pot"]), "a second claim paid {again}, the contract holds {left}, the pot is {}", q["pot"]);
    Ok(format!("fees {fee_in} on the buy and {fee_out} on the sale; claim_fees paid exactly {owed}, then 0; the contract holds {left}, exactly the pot"))
}

async fn failed_payout(c: &Chain) -> Result<String> {
    let ft = c.ft.id();
    let m = c.market(ft, 16, 0, 500 * ONE).await?;
    let (buyer, third) = (c.holder(ft, 1_000 * ONE).await?, c.worker.dev_create_account().await?);
    c.buy(&m, &buyer, 3, true, 100 * ONE).await?;
    // a holder who never registered with the token: they got their tokens by transfer
    let seller = c.worker.dev_create_account().await?;
    let token = format!("{}:yes:3", m.id);
    let res = buyer.call(c.aimm.id(), "mt_transfer").args_json(json!({"receiver_id": seller.id(), "token_id": token, "amount": (100 * ONE).to_string()}))
        .deposit(NearToken::from_near(1).saturating_div(100)).gas(Gas::from_tgas(100)).transact().await?;
    event(&res, "transfer")?;
    let pending_row = (1 + 8 + 4 + seller.id().len() as u128 + 16 + 40) * BYTE_COST;
    let before = c.storage(m.id).await?;
    let res = c.sell(&m, &seller, 3, true, 40 * ONE).await?;
    ensure!(res.is_success(), "the sale failed: {}", why(&res));
    refused(&res, "is not registered")?;
    let proceeds = u(&event(&res, "sell")?["refund"]);
    let failed = event(&res, "payment_failed")?;
    ensure!(u(&failed["amount"]) == proceeds, "payment_failed says {}, the sale paid {proceeds}", failed["amount"]);
    let on_paid = res.outcomes().iter().find(|o| o.logs.iter().any(|l| l.contains("payment_failed"))).map(|o| o.gas_burnt.as_gas()).unwrap_or(0);
    let check = async |money: u128, storage: u128| -> Result<()> {
        let (held, kept, left) = (c.holdings(m.id, seller.id()).await?, c.pending(m.id, seller.id()).await?, c.storage(m.id).await?);
        ensure!(u(&held["yes"][3]) == 60 * ONE, "the tokens did not stay burned: {}", held["yes"][3]);
        ensure!(kept == money, "pending_of is {kept}, not {money}");
        ensure!(left == storage, "the storage balance is {left}, not {storage}");
        Ok(())
    };
    check(proceeds, before - pending_row).await?;
    // a retry before the seller is registered fails the same way and loses nothing
    let args = json!({"question_id": m.id, "account_id": seller.id()});
    event(&c.yocto(&third, "withdraw_pending", args.clone()).await?, "payment_failed")?;
    check(proceeds, before - pending_row).await?;
    third.call(ft, "storage_deposit").args_json(json!({"account_id": seller.id()})).deposit(NearToken::from_millinear(10)).transact().await?.into_result()?;
    let res = c.yocto(&third, "withdraw_pending", args.clone()).await?;
    let delivered = u(&event(&res, "withdraw_pending")?["amount"]);
    ensure!(delivered == proceeds && c.balance(ft, seller.id()).await? == proceeds, "the seller holds {}, not {proceeds}", c.balance(ft, seller.id()).await?);
    check(0, before).await?;
    refused(&c.yocto(&third, "withdraw_pending", args).await?, "AIMM: nothing pending")?;
    Ok(format!("sale of 40e6 by an unregistered holder: transfer failed, payment_failed event, pending_of {proceeds}, tokens burned, on_paid burnt {} of its 10; \
        a retry kept it pending; once registered, a third party's withdraw_pending delivered {proceeds} and the row's {} came back", tgas(on_paid), near(pending_row)))
}

async fn private_callback(c: &Chain) -> Result<String> {
    let m = c.market(c.ft.id(), 3, 0, 100 * ONE).await?;
    let mallory = c.worker.dev_create_account().await?;
    let before = c.storage(m.id).await?;
    let res = mallory.call(c.aimm.id(), "on_paid").args_json(json!({"question_id": m.id, "to": mallory.id(), "amount": "1000000"}))
        .gas(Gas::from_tgas(30)).transact().await?;
    refused(&res, "on_paid is private")?;
    ensure!(c.pending(m.id, mallory.id()).await? == 0 && c.storage(m.id).await? == before, "the call changed something");
    Ok(format!("refused: {}; pending_of stayed 0", said(&why(&res))))
}

async fn one_yocto(c: &Chain) -> Result<String> {
    let m = c.market(c.ft.id(), 3, 0, 100 * ONE).await?;
    c.past(m.close).await?; // so nothing but the key or the deposit stands in the way of a report
    let sk = SecretKey::from_random(KeyType::ED25519);
    let key = AccessKey::function_call_access(c.aimm.id(), &["halt", "resolve"], Some(NearToken::from_near(1)));
    c.resolver.batch(c.resolver.id()).add_key(sk.public_key(), key).transact().await?.into_result()?;
    let limited = Account::from_secret_key(c.resolver.id().clone(), sk, &c.worker);
    for (method, args) in [("halt", json!({"question_id": m.id})), ("resolve", json!({"question_id": m.id, "winner": 0}))] {
        // a function-call key cannot attach the yocto: the node turns the transaction away
        let sent = limited.call(c.aimm.id(), method).args_json(&args).deposit(NearToken::from_yoctonear(1)).transact().await;
        let rejection = match sent {
            Ok(res) => bail!("{method} with 1 yocto from a function-call key was accepted: {:?}", res.is_success()),
            Err(e) => format!("{e:?}"),
        };
        ensure!(rejection.contains("InvalidAccessKeyError(DepositWithFunctionCall)"), "{method}: got {rejection}");
        // without the yocto the contract refuses it, from either key
        for who in [&limited, &c.resolver] {
            refused(&who.call(c.aimm.id(), method).args_json(&args).transact().await?, "Requires attached deposit of exactly 1 yoctoNEAR")?;
        }
    }
    let q = c.q(m.id).await?;
    ensure!(q["halted"] == false && q["resolved"] == false, "a refused call changed the question");
    // the full-access key with the yocto does both
    event(&c.yocto(&c.resolver, "halt", json!({"question_id": m.id})).await?, "halt")?;
    event(&c.resolve(&m, 0).await?, "resolve")?;
    Ok("halt and resolve: a function-call key with the yocto is turned away by the node (DepositWithFunctionCall); without the yocto the contract refuses either key; a full-access key with it does both".into())
}

async fn readable_errors(c: &Chain) -> Result<String> {
    let ft = c.ft.id();
    let m = c.market(ft, 3, 0, 100 * ONE).await?;
    let t = c.holder(ft, 1_000 * ONE).await?;
    c.buy(&m, &t, 0, true, 10 * ONE).await?;
    let close = c.now().await? + TRADING;
    // at depth 1e24, selling 1 of 2,000,000 tokens would raise computed C
    let (deep, _) = c.create(&t, t.id(), 3, 0, 100 * MILLI).await?;
    c.as_token(&t, open_msg(deep, 1_000_000 * WAD, &[WAD / 2, 3 * WAD / 10, WAD / 5])).await?.into_result()?;
    c.as_token(&t, buy_msg(deep, 0, true, 2_000_000, ONE)).await?.into_result()?;
    let cases = [
        ("AIMM: BadRange", c.create_call(&t, ft, 2, 0, 500 * MILLI, close).await?),
        ("AIMM: attach at least ", c.create_call(&t, ft, 3, 0, 0, close).await?),
        (" above max_cost", c.send(&m, &t, 0, true, 10 * ONE, 100 * ONE, ONE).await?),
        ("AIMM: not enough tokens", c.sell(&m, &t, 0, true, 11 * ONE).await?),
        ("AIMM: only the resolver or the operator", c.yocto(&t, "halt", json!({"question_id": m.id})).await?),
        ("AIMM: SaleTooSmall", c.yocto(&t, "sell", json!({"question_id": deep, "range": 0, "yes": true, "tickets": "1", "min_proceeds": "0"})).await?),
        ("AIMM: nothing pending", c.yocto(&t, "withdraw_pending", json!({"question_id": m.id})).await?),
        ("AIMM: token id is not in its plain form", c.yocto(&t, "mt_transfer", json!({"receiver_id": m.operator.id(), "token_id": format!("0{}:yes:0", m.id), "amount": "1"})).await?),
    ];
    let mut seen = Vec::new();
    for (text, res) in &cases {
        refused(res, text)?;
        let reason = why(res);
        ensure!(reason.contains("Smart contract panicked: ") && !reason.contains("WebAssembly") && !reason.contains("unreachable"), "not readable: {reason}");
        seen.push(said(&reason).to_string());
    }
    let view = c.aimm.view("get_question").args_json(json!({"question_id": 999})).await;
    let view = view.err().map(|e| format!("{e:?}")).unwrap_or_default();
    ensure!(view.contains("AIMM: no such question"), "a view of no question gave {view:?}");
    seen.push("AIMM: no such question (a view)".into());
    Ok(seen.join(" | "))
}

/// The attack of a question whose collateral is the attacker's own account, with no real money anywhere,
/// and a storage row for every fresh name until the question's own balance is gone.
/// A fake token can name any buyer, but only on its own question: the rows it makes come out of that question's balance.
async fn fake_collateral(c: &Chain) -> Result<String> {
    let (free0, live0) = c.free_near().await?;
    let mallory = c.worker.dev_create_account().await?;
    let (id, _) = c.create(&mallory, mallory.id(), 16, 0, 150 * MILLI).await?;
    c.as_token(&mallory, open_msg(id, DEPTH, &even(16))).await?.into_result()?;
    let start = c.storage(id).await?;
    let mut names = 0;
    let refusal = loop {
        let res = c.as_token_for(&mallory, &fresh_name(id, names), buy_msg(id, 0, true, 1, ONE)).await?;
        if res.is_failure() {
            break why(&res);
        }
        names += 1;
        ensure!(names <= start / HOLDER_ROW, "name {names} went through with only {} for it", near(start));
    };
    ensure!(refusal.contains("AIMM: this question's storage balance is") && refusal.contains("anyone can call add_storage"), "refused otherwise: {refusal}");
    let left = c.storage(id).await?;
    ensure!(names == start / HOLDER_ROW && left == start - names * HOLDER_ROW, "{names} names left {left} of {start}");

    let m = c.market(c.ft.id(), 5, 0, 100 * ONE).await?;
    refused(&c.as_token(&mallory, buy_msg(m.id, 0, true, 1, ONE)).await?, "AIMM: wrong collateral")?;
    let honest = c.holder(c.ft.id(), 100 * ONE).await?;
    c.buy(&m, &honest, 0, true, 10 * ONE).await?;
    ensure!(u(&c.holdings(m.id, honest.id()).await?["yes"][0]) == 10 * ONE, "the honest buyer holds nothing");

    let (free1, live1) = c.free_near().await?;
    ensure!(free1 >= live1, "the contract's free NEAR {free1} is below the questions' balances {live1}");
    ensure!(free1 - live1 >= free0 - live0, "the contract lost NEAR: {} beyond the balances before, {} after", free0 - live0, free1 - live1);
    Ok(format!("{names} names took {} of the question's {} at exactly {} each, then refused; an honest new buyer on question {} went through; \
        free NEAR {free1} >= balances {live1} yocto, {} yocto above them before and {} after", near(start - left), near(start), near(HOLDER_ROW), m.id, free0 - live0, free1 - live1))
}

/// A holder of one unit cannot spend an honest question's balance on names of their choosing: the caller pays for the row.
async fn transfer_pays_for_its_row(c: &Chain) -> Result<String> {
    let ft = c.ft.id();
    let m = c.market(ft, 16, WAD / 50, 500 * ONE).await?;
    let mallory = c.holder(ft, 100 * ONE).await?;
    c.buy(&m, &mallory, 0, true, ONE).await?;
    let (start, burnt_at) = (c.storage(m.id).await?, c.free_near().await?);
    refused(&c.transfer(m.id, &mallory, &fresh_name(m.id, 0), 1, 1).await?, "for the receiver's row")?;
    refused(&c.transfer(m.id, &mallory, &fresh_name(m.id, 0), 1, HOLDER_ROW - 1).await?, "for the receiver's row")?;
    // a whole row moved on pays for the new row, so buying and moving cannot make the question pay again and again
    let walker = c.holder(ft, 100 * ONE).await?;
    let before_walk = c.storage(m.id).await?;
    let row = (1 + 8 + 4 + walker.id().len() as u128 + 4 + 32 * 16 + 40) * BYTE_COST;
    for i in 0..3 {
        c.buy(&m, &walker, 0, true, ONE).await?;
        ensure!(c.storage(m.id).await? == before_walk - row, "the purchase did not take one row");
        let name = fresh_name(m.id, 100 + i);
        refused(&c.transfer(m.id, &walker, &name, ONE, 1).await?, "for the receiver's row")?;
        event(&c.transfer(m.id, &walker, &name, ONE, 10 * MILLI).await?, "transfer")?;
        ensure!(c.storage(m.id).await? == before_walk, "the question paid for a moved row");
    }
    let held = c.aimm.view_account().await?.balance.as_yoctonear();
    for i in 0..5 {
        event(&c.transfer(m.id, &mallory, &fresh_name(m.id, i), 1, 10 * MILLI).await?, "transfer")?;
    }
    ensure!(c.storage(m.id).await? == start, "the question paid for a row");
    let rose = c.aimm.view_account().await?.balance.as_yoctonear() - held;
    ensure!(rose == 5 * HOLDER_ROW, "the contract kept {rose}, not 5 rows of {HOLDER_ROW}");
    let free = c.free_near().await?;
    ensure!(free.0 - free.1 >= burnt_at.0 - burnt_at.1, "the contract's own NEAR fell");
    Ok(format!("at 1 yocto and at one yocto under the row's cost: refused. 5 rows on fresh names left the question's balance at {} yocto, and the contract kept exactly {} of the 0.05 NEAR attached", start, near(rose)))
}

async fn refused_purchase(c: &Chain) -> Result<String> {
    let ft = c.ft.id();
    let deposit = 100 * MILLI;
    let spare = c.storage(c.market_with(ft, 3, 0, 100 * ONE, deposit).await?.id).await?;
    let m = c.market_with(ft, 3, 0, 100 * ONE, deposit - spare).await?;
    ensure!(c.storage(m.id).await? == 0, "the question opened with a storage balance");
    let (buyer, stranger) = (c.holder(ft, 1_000 * ONE).await?, c.worker.dev_create_account().await?);
    let check = async || -> Result<()> {
        let (b, a, pot) = (c.balance(ft, buyer.id()).await?, c.balance(ft, c.aimm.id()).await?, c.q(m.id).await?["pot"].clone());
        ensure!(b == 1_000 * ONE, "the buyer holds {b}");
        ensure!(u(&pot) == 100 * ONE, "the pot is {pot}, the contract {a}");
        Ok(())
    };
    let held = c.balance(ft, c.aimm.id()).await?;
    let res = c.send(&m, &buyer, 0, true, 10 * ONE, 500 * ONE, 500 * ONE).await?;
    refused(&res, "AIMM: this question's storage balance is")?;
    check().await?;
    // enough for the buyer's row and nothing more
    let row = (1 + 8 + 4 + buyer.id().len() as u128 + 4 + 32 * 3 + 40) * BYTE_COST;
    stranger.call(c.aimm.id(), "add_storage").args_json(json!({"question_id": m.id})).deposit(NearToken::from_yoctonear(row)).transact().await?.into_result()?;
    let res = c.send(&m, &buyer, 0, true, 10 * ONE, 500 * ONE, ONE).await?;
    refused(&res, " above max_cost")?;
    check().await?;
    ensure!(c.balance(ft, c.aimm.id()).await? == held, "the contract kept money from a refused purchase");
    let res = c.send(&m, &buyer, 0, true, 10 * ONE, 500 * ONE, 500 * ONE).await?;
    let cost = u(&event(&res, "buy")?["cost"]);
    ensure!(c.balance(ft, buyer.id()).await? == 1_000 * ONE - cost, "the buyer did not get back what the purchase left");
    ensure!(c.storage(m.id).await? == 0, "the row did not cost {row}");
    // with nothing to hold for a failed transfer, a sale is refused before it burns anything
    refused(&c.sell(&m, &buyer, 0, true, 5 * ONE).await?, "AIMM: this question's storage balance is")?;
    ensure!(u(&c.holdings(m.id, buyer.id()).await?["yes"][0]) == 10 * ONE, "the refused sale burned tokens");
    // selling out frees the holder's row, which pays for the hold
    let res = c.sell(&m, &buyer, 0, true, 10 * ONE).await?;
    ensure!(res.failures().is_empty(), "selling out failed: {}", why(&res));
    event(&res, "sell")?;
    ensure!(u(&c.holdings(m.id, buyer.id()).await?["yes"][0]) == 0, "the seller still holds tokens");
    Ok(format!("500e6 sent: refused for storage and then for max_cost, the buyer holding 1000e6 after each; after add_storage of {row} yocto, the row's cost, the purchase took {cost} and sent the rest back; \
        with the balance at 0 a part sale was refused and burned nothing, and selling out went through"))
}

async fn withdraw_storage(c: &Chain) -> Result<String> {
    let m = c.market(c.ft.id(), 3, 0, 100 * ONE).await?;
    let t = c.holder(c.ft.id(), 100 * ONE).await?;
    c.buy(&m, &t, 0, true, 10 * ONE).await?;
    let args = json!({"question_id": m.id});
    refused(&c.yocto(&m.operator, "withdraw_storage", args.clone()).await?, "AIMM: NotResolved")?;
    c.past(m.close).await?;
    event(&c.resolve(&m, 0).await?, "resolve")?;
    refused(&c.yocto(&t, "withdraw_storage", args.clone()).await?, "AIMM: only the operator")?;
    let left = c.storage(m.id).await?;
    let (contract, operator) = (c.aimm.view_account().await?.balance.as_yoctonear(), m.operator.view_account().await?.balance.as_yoctonear());
    let res = c.yocto(&m.operator, "withdraw_storage", args.clone()).await?;
    let burnt: u128 = res.outcomes().iter().map(|o| o.tokens_burnt.as_yoctonear()).sum();
    let paid: Value = res.json()?;
    ensure!(u(&paid) == left && left > 0, "withdraw_storage returned {paid}, the balance was {left}");
    let fell = contract + 1 - c.aimm.view_account().await?.balance.as_yoctonear();
    let rose = m.operator.view_account().await?.balance.as_yoctonear() + 1 - operator;
    ensure!(fell == left, "the contract's balance fell by {fell}, not {left}");
    ensure!(rose <= left && left - rose < 2 * MILLI, "the operator gained {rose} of {left}");
    ensure!(c.storage(m.id).await? == 0, "the storage balance is not empty");
    // the winner comes later and their row pays for their transfer
    let had = c.balance(c.ft.id(), t.id()).await?;
    let got = u(&event(&c.yocto(&t, "redeem", args.clone()).await?, "redeem")?["amount"]);
    ensure!(got == 10 * ONE && c.balance(c.ft.id(), t.id()).await? == had + got, "the winner was not paid after the balance went");
    let row = c.storage(m.id).await?;
    ensure!(row > 0, "the freed row did not come back");
    let again: Value = c.yocto(&m.operator, "withdraw_storage", args.clone()).await?.json()?;
    ensure!(u(&again) == row, "the second withdrawal took {again}, not {row}");
    // a question that never opened gives its balance back once it cannot open
    let (id, close) = c.create(&m.operator, c.ft.id(), 3, 0, 100 * MILLI).await?;
    let unopened = json!({"question_id": id});
    refused(&c.yocto(&m.operator, "withdraw_storage", unopened.clone()).await?, "AIMM: NotResolved")?;
    let left2 = c.storage(id).await?;
    c.past(close).await?;
    let back: Value = c.yocto(&m.operator, "withdraw_storage", unopened).await?.json()?;
    ensure!(u(&back) == left2 && left2 > 0, "the unopened question gave back {back}, not {left2}");
    Ok(format!("refused before the result; then paid {left} yocto: the contract fell by exactly that, the operator rose by that less {} yocto of gas ({burnt} burnt); \
        the winner redeemed after it, and the row's {row} could be taken too; a question never opened gave back {left2} once closed", left - rose))
}

/// Anyone can settle a holder. The money only ever goes to the holder, and every row can go, on names nobody controls too.
async fn anyone_can_settle(c: &Chain) -> Result<String> {
    let ft = c.ft.id();
    let m = c.market(ft, 3, 0, 100 * ONE).await?;
    let (winner, loser, stranger) = (c.holder(ft, 100 * ONE).await?, c.holder(ft, 100 * ONE).await?, c.worker.dev_create_account().await?);
    c.buy(&m, &winner, 0, true, 10 * ONE).await?;
    c.buy(&m, &loser, 1, true, 10 * ONE).await?;
    let dead = fresh_name(m.id, 7);
    event(&c.transfer(m.id, &winner, &dead, ONE, 10 * MILLI).await?, "transfer")?;
    c.past(m.close).await?;
    event(&c.resolve(&m, 0).await?, "resolve")?;
    let settle = |who: &AccountId| json!({"question_id": m.id, "account_id": who});
    let (had, start) = (c.balance(ft, winner.id()).await?, c.storage(m.id).await?);
    let ev = event(&c.yocto(&stranger, "redeem", settle(winner.id())).await?, "redeem")?;
    ensure!(u(&ev["amount"]) == 9 * ONE && c.balance(ft, winner.id()).await? == had + 9 * ONE, "the holder was not paid: {ev}");
    ensure!(c.balance(ft, stranger.id()).await.unwrap_or(0) == 0, "the stranger was paid");
    let ev = event(&c.yocto(&stranger, "redeem", settle(loser.id())).await?, "redeem")?;
    ensure!(u(&ev["amount"]) == 0 && u(&c.q(m.id).await?["yes_supply"][1]) == 0, "the loser's row did not go: {ev}");
    let res = c.yocto(&stranger, "redeem", settle(&dead)).await?;
    ensure!(u(&event(&res, "redeem")?["amount"]) == ONE, "the name nobody controls was not settled");
    event(&res, "payment_failed")?;
    ensure!(c.pending(m.id, &dead).await? == ONE, "the money did not stay under the name");
    let q = c.q(m.id).await?;
    ensure!(u(&q["yes_supply"][0]) == 0, "winning tokens are left");
    ensure!(c.storage(m.id).await? > start, "freeing the rows did not fill the balance");
    Ok(format!("a stranger settled a winner (paid to the winner), a loser (row freed) and a name nobody controls (its {ONE} kept under that name); no winning tokens left and the balance rose by {}", near(c.storage(m.id).await? - start)))
}

async fn receiver_is_not_the_contract(c: &Chain) -> Result<String> {
    let (ft, aimm) = (c.ft.id(), c.aimm.id());
    let m = c.market(ft, 3, WAD / 50, 100 * ONE).await?;
    let t = c.holder(ft, 1_000 * ONE).await?;
    c.buy(&m, &t, 0, true, 10 * ONE).await?;
    refused(&c.transfer(m.id, &t, aimm, ONE, 10 * MILLI).await?, "AIMM: the receiver is this contract")?;
    ensure!(u(&c.holdings(m.id, t.id()).await?["yes"][0]) == 10 * ONE, "a refusal moved tokens");
    ensure!(u(&c.holdings(m.id, aimm).await?["yes"][0]) == 0, "the contract holds tokens");
    Ok("mt_transfer refused it, and nothing moved; every payment goes to the caller or the holder, so nothing else can name it".into())
}

async fn halt_is_final(c: &Chain) -> Result<String> {
    const CLOSED: &str = "AIMM: TradingClosed";
    let ft = c.ft.id();
    let m = c.market(ft, 3, 0, 100 * ONE).await?;
    let t = c.holder(ft, 1_000 * ONE).await?;
    c.buy(&m, &t, 0, true, 10 * ONE).await?;
    let args = json!({"question_id": m.id});
    refused(&c.yocto(&t, "halt", args.clone()).await?, "AIMM: only the resolver or the operator")?;
    event(&c.yocto(&c.resolver, "halt", args.clone()).await?, "halt")?;
    ensure!(c.q(m.id).await?["halted"] == true, "the question is not halted");
    refused(&c.send(&m, &t, 0, true, ONE, 10 * ONE, 10 * ONE).await?, CLOSED)?;
    refused(&c.sell(&m, &t, 0, true, ONE).await?, CLOSED)?;
    refused(&c.resolve(&m, 0).await?, "AIMM: TooEarly")?;
    // nothing brings trading back, however long it waits
    c.past(c.now().await? + 30_000_000_000).await?;
    refused(&c.send(&m, &t, 0, true, ONE, 10 * ONE, 10 * ONE).await?, CLOSED)?;
    let still_open = c.now().await? < m.close;
    ensure!(still_open, "the clock passed the close, so that refusal proved nothing");
    c.past(m.close).await?;
    let ev = event(&c.resolve(&m, 0).await?, "resolve")?;
    refused(&c.resolve(&m, 1).await?, "AIMM: AlreadyResolved")?;
    let got = u(&event(&c.yocto(&t, "redeem", args).await?, "redeem")?["amount"]);
    ensure!(got == 10 * ONE, "redeemed {got}");
    Ok(format!("buy and sell refused with TradingClosed at once and 30 s later, before the close; the resolver then reported, payout {}, and {got} was redeemed", u(&ev["payout"]) as f64 / 1e18))
}

/// A failed receipt keeps its logs, so a call that pays makes the payment first, and one refused for storage logs no event.
async fn refused_calls_log_nothing(c: &Chain) -> Result<String> {
    const SHORT: &str = "AIMM: this question's storage balance is";
    let ft = c.ft.id();
    let deposit = 100 * MILLI;
    let spare = c.storage(c.market_with(ft, 3, WAD / 50, 100 * ONE, deposit).await?.id).await?;
    let m = c.market_with(ft, 3, WAD / 50, 100 * ONE, deposit - spare).await?;
    ensure!(c.storage(m.id).await? == 0, "the question opened with a storage balance");
    let buyer = c.holder(ft, 1_000 * ONE).await?;
    let silent = |res: &ExecutionFinalResult, text: &str| -> Result<()> {
        refused(res, text)?;
        let logs = res.logs();
        ensure!(!logs.iter().any(|l| l.contains("\"standard\":\"aimm\"")), "a refused call logged {logs:?}");
        Ok(())
    };
    silent(&c.send(&m, &buyer, 0, true, 10 * ONE, 500 * ONE, 500 * ONE).await?, SHORT)?;
    let row = (1 + 8 + 4 + buyer.id().len() as u128 + 4 + 32 * 3 + 40) * BYTE_COST;
    m.operator.call(c.aimm.id(), "add_storage").args_json(json!({"question_id": m.id})).deposit(NearToken::from_yoctonear(row)).transact().await?.into_result()?;
    event(&c.send(&m, &buyer, 0, true, 10 * ONE, 500 * ONE, 500 * ONE).await?, "buy")?;
    ensure!(c.storage(m.id).await? == 0, "the row did not cost {row}");
    silent(&c.sell(&m, &buyer, 0, true, 5 * ONE).await?, SHORT)?;
    silent(&c.yocto(&m.operator, "claim_fees", json!({"question_id": m.id})).await?, SHORT)?;
    silent(&c.transfer(m.id, &buyer, &fresh_name(m.id, 0), 1, 1).await?, "for the receiver's row")?;
    c.past(m.close).await?;
    event(&c.resolve(&m, 1).await?, "resolve")?;
    silent(&c.yocto(&m.operator, "claim_surplus", json!({"question_id": m.id})).await?, SHORT)?;
    Ok("a purchase, a part sale, claim_fees, a transfer short of its row and claim_surplus were each refused and logged no aimm event".into())
}

fn report(failed: &mut usize, claim: &str, result: Result<String>) {
    match result {
        Ok(s) => println!("PASS {claim}: {s}"),
        Err(e) => {
            *failed += 1;
            println!("FAIL {claim}: {e:#}");
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let root = env!("CARGO_MANIFEST_DIR");
    let aimm_wasm = std::fs::read(format!("{root}/../target/wasm32-unknown-unknown/release/aimm_contract.wasm"))?;
    let ft_wasm = std::fs::read(format!("{root}/../target/wasm32-unknown-unknown/release/test_token.wasm"))?;
    let worker = near_workspaces::sandbox().await?;
    let aimm = worker.dev_deploy(&aimm_wasm).await?;
    aimm.call("new").max_gas().transact().await?.into_result()?;
    let resolver = worker.dev_create_account().await?;
    let a = aimm.view_account().await?;
    let free = a.balance.as_yoctonear() - a.storage_usage as u128 * BYTE_COST;
    aimm.as_account().transfer_near(resolver.id(), NearToken::from_yoctonear(free - SPARE)).await?.into_result()?;
    let ft = worker.dev_deploy(&ft_wasm).await?;
    ft.call("new").max_gas().transact().await?.into_result()?;
    resolver.call(ft.id(), "storage_deposit").args_json(json!({"account_id": aimm.id()})).deposit(NearToken::from_millinear(100)).transact().await?.into_result()?;
    let c = Chain { worker, aimm, ft, ft_wasm, resolver, fast_forwarding: Cell::new(Duration::ZERO) };
    let (spare, _) = c.free_near().await?;
    println!("sandbox up: AIMM at {}; its account keeps {spare} yoctoNEAR ({}) beyond its state", c.aimm.id(), near(spare));

    let mut failed = 0;
    report(&mut failed, "1. 16 answers", sixteen_answers(&c).await);
    report(&mut failed, "2. short pot", short_pot(&c).await);
    report(&mut failed, "3. fees", fees(&c).await);
    report(&mut failed, "4. failed payout", failed_payout(&c).await);
    report(&mut failed, "5. on_paid is private", private_callback(&c).await);
    report(&mut failed, "6. resolve and halt need a full-access key and 1 yocto", one_yocto(&c).await);
    report(&mut failed, "7. readable errors", readable_errors(&c).await);
    report(&mut failed, "8. fake collateral, buys for fresh names", fake_collateral(&c).await);
    report(&mut failed, "9. mt_transfer pays for its own row", transfer_pays_for_its_row(&c).await);
    report(&mut failed, "10. a refused purchase returns the money, and an empty balance refuses a payment whole", refused_purchase(&c).await);
    report(&mut failed, "11. withdraw_storage", withdraw_storage(&c).await);
    report(&mut failed, "12. the contract cannot be a receiver", receiver_is_not_the_contract(&c).await);
    report(&mut failed, "13. halt is final and a resolver can report", halt_is_final(&c).await);
    report(&mut failed, "14. anyone can settle a holder, and the money goes to the holder", anyone_can_settle(&c).await);
    report(&mut failed, "15. a call refused for storage logs nothing", refused_calls_log_nothing(&c).await);

    let (free, live) = c.free_near().await?;
    println!("at the end: free NEAR {free} yocto, the questions' balances {live} yocto, the difference {} (it started at {spare})", free as i128 - live as i128);
    println!("moving the clock took {:.0} s in all", c.fast_forwarding.get().as_secs_f64());
    if free < live + spare {
        failed += 1;
        println!("FAIL the contract's own NEAR fell");
    }
    if failed > 0 {
        bail!("{failed} of 15 claims failed");
    }
    println!("all 15 claims passed");
    Ok(())
}
