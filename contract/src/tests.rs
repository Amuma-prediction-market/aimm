//! The contract around the core: who may do what, money in through `ft_on_transfer`, money out as the receipts show it,
//! tokens per holder, the clock, the storage balance, the views and the events.
use super::*;
use aimm_core::math::{mul_div, mul_div_up};
use near_sdk::mock::{with_mocked_blockchain, MockAction};
use near_sdk::serde_json::{self, json, Value};
use near_sdk::test_utils::{get_created_receipts, get_logs, VMContextBuilder};
use near_sdk::{testing_env, PromiseResult};
use std::any::Any;
use std::collections::HashMap;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::OnceLock;

const HOUR: u64 = 3_600_000_000_000;
const T0: u64 = 1_800_000_000_000_000_000;
const ONE: u128 = 1_000_000; // one USDC in its smallest unit
const CLOSE: u64 = T0 + 24 * HOUR;
const RESOLVE_AFTER: u64 = T0 + 25 * HOUR;
const FALLBACK_AFTER: u64 = T0 + 72 * HOUR;
const TRADING: u64 = T0 + 3 * HOUR;
const LATE: u64 = T0 + 26 * HOUR;
/// Always covers a new holder's row in `mt_transfer`.
const ROW_DEPOSIT: u128 = 10u128.pow(22);
const OPEN: &str = r#"{"action":"open","question_id":1,"depth":"2000000000","prices":["380000000000000000","290000000000000000","190000000000000000","110000000000000000","30000000000000000"]}"#;

fn acc(s: &str) -> AccountId {
    s.parse().unwrap()
}

/// `test_vm_config` builds the runtime's whole configuration on every call, which is nearly all the time these tests take.
fn once<T: Clone + Send + Sync + 'static>(make: fn() -> T) -> T {
    static MADE: OnceLock<Box<dyn Any + Send + Sync>> = OnceLock::new();
    MADE.get_or_init(|| Box::new(make())).downcast_ref::<T>().unwrap().clone()
}

fn ctx_via(who: &str, signer: &str, deposit: u128, now: u64) {
    let mut b = VMContextBuilder::new();
    b.current_account_id(acc("aimm.near"))
        .predecessor_account_id(acc(who))
        .signer_account_id(acc(signer))
        .attached_deposit(NearToken::from_yoctonear(deposit))
        .block_timestamp(now);
    testing_env!(b.build(), once(near_sdk::test_vm_config));
}

fn ctx(who: &str, deposit: u128, now: u64) {
    ctx_via(who, who, deposit, now);
}

fn value(r: PromiseOrValue<U128>) -> u128 {
    match r {
        PromiseOrValue::Value(v) => v.0,
        _ => panic!("expected a value"),
    }
}

fn five_answers() -> CreateArgs {
    CreateArgs {
        collateral: acc("usdc.near"),
        title: "Highest temperature in NYC on December 1".into(),
        answers: vec!["49°F or below".into(), "50-51°F".into(), "52-53°F".into(), "54-55°F".into(), "56°F or higher".into()],
        resolver: acc("oracle.near"),
        fallback_resolver: acc("council.near"),
        close_time: U64(CLOSE),
        resolve_after: U64(RESOLVE_AFTER),
        fallback_after: U64(FALLBACK_AFTER),
        fee_rate: U128(WAD / 50),
    }
}

fn create(args: CreateArgs) -> (Contract, u64) {
    mock_with(HashMap::new());
    ctx("operator.near", 0, T0);
    let mut c = Contract::new();
    ctx("operator.near", 10u128.pow(23), T0);
    let id = c.create_question(args);
    (c, id)
}

fn created() -> (Contract, u64) {
    create(five_answers())
}

/// Depth 2,000 USDC worth of tokens, with `fund` of real money behind it.
fn funded(fund: u128) -> (Contract, u64) {
    let (mut c, id) = created();
    ctx("usdc.near", 0, T0 + 1);
    assert_eq!(value(c.ft_on_transfer(acc("operator.near"), U128(fund), OPEN.into())), 0, "the whole fund stays");
    (c, id)
}

fn setup() -> (Contract, u64) {
    funded(1_000 * ONE)
}

fn buy_msg(id: u64, range: u8, yes: bool, tickets: u128, max_cost: u128) -> String {
    format!(r#"{{"action":"buy","question_id":{id},"range":{range},"yes":{yes},"tickets":"{tickets}","max_cost":"{max_cost}"}}"#)
}

fn buy_on(c: &mut Contract, now: u64, who: &str, range: u8, yes: bool, tickets: u128, send: u128) -> u128 {
    // the token calls in; whoever sent the money signed
    ctx_via("usdc.near", who, 0, now);
    value(c.ft_on_transfer(acc(who), U128(send), buy_msg(1, range, yes, tickets, send)))
}

fn buy(c: &mut Contract, who: &str, range: u8, yes: bool, tickets: u128, send: u128) -> u128 {
    buy_on(c, T0 + 2 * HOUR, who, range, yes, tickets, send)
}

/// One function call this call scheduled.
struct Call {
    contract: String,
    method: String,
    args: Value,
    deposit: u128,
    gas: u64,
    waits_for: usize,
}

fn calls() -> Vec<Call> {
    let mut out = vec![];
    for r in get_created_receipts() {
        for a in r.actions {
            if let MockAction::FunctionCallWeight { method_name, args, attached_deposit, prepaid_gas, .. } = a {
                out.push(Call {
                    contract: r.receiver_id.to_string(),
                    method: String::from_utf8(method_name).unwrap(),
                    args: serde_json::from_slice(&args).unwrap(),
                    deposit: attached_deposit.as_yoctonear(),
                    gas: prepaid_gas.as_gas(),
                    waits_for: r.receipt_indices.len(),
                });
            }
        }
    }
    out
}

/// The money this call sent: exactly one transfer of the question's token, and `on_paid` waiting for its outcome.
fn assert_paid_in(token: &str, id: u64, to: &str, amount: u128) {
    let c = calls();
    assert_eq!(c.len(), 2, "one transfer and one callback");
    assert_eq!((c[0].contract.as_str(), c[0].method.as_str()), (token, "ft_transfer"));
    assert_eq!((c[0].deposit, c[0].gas, c[0].waits_for), (1, GAS_FT_TRANSFER.as_gas(), 0));
    assert_eq!(c[0].args, json!({"receiver_id": to, "amount": amount.to_string(), "memo": "AIMM"}));
    assert_eq!((c[1].contract.as_str(), c[1].method.as_str()), ("aimm.near", "on_paid"));
    assert_eq!((c[1].deposit, c[1].gas, c[1].waits_for), (0, GAS_CALLBACK.as_gas(), 1));
    assert_eq!(c[1].args, json!({"question_id": id, "to": to, "amount": amount.to_string()}));
}

fn assert_paid(id: u64, to: &str, amount: u128) {
    assert_paid_in("usdc.near", id, to, amount);
}

fn assert_nothing_sent() {
    assert!(get_created_receipts().is_empty(), "nothing leaves the contract");
}

/// The call `on_paid` runs in, with what the token's transfer came to.
fn callback(result: PromiseResult) {
    let mut b = VMContextBuilder::new();
    b.current_account_id(acc("aimm.near")).predecessor_account_id(acc("aimm.near")).signer_account_id(acc("aimm.near")).block_timestamp(LATE);
    testing_env!(b.build(), once(near_sdk::test_vm_config), near_sdk::RuntimeFeesConfig::test(), Default::default(), vec![result]);
}

/// The outcome of a payment comes back to `on_paid`.
fn delivered(c: &mut Contract, id: u64, to: &str, amount: u128) {
    callback(PromiseResult::Successful(vec![]));
    c.on_paid(id, acc(to), U128(amount));
}

fn bounced(c: &mut Contract, id: u64, to: &str, amount: u128) {
    callback(PromiseResult::Failed);
    c.on_paid(id, acc(to), U128(amount));
}

/// The NEAR this call sent: (receiver, yoctoNEAR).
fn near_sent() -> Vec<(String, u128)> {
    let mut out = vec![];
    for r in get_created_receipts() {
        for a in r.actions {
            if let MockAction::Transfer { deposit, .. } = a {
                out.push((r.receiver_id.to_string(), deposit.as_yoctonear()));
            }
        }
    }
    out
}

/// The one event of that name this call logged, as its data.
fn logged(name: &str) -> Value {
    let all: Vec<Value> = get_logs().iter().filter_map(|l| l.strip_prefix("EVENT_JSON:")).map(|l| serde_json::from_str(l).unwrap()).collect();
    let found: Vec<&Value> = all.iter().filter(|e| e["event"] == name).collect();
    assert_eq!(found.len(), 1, "one {name} in {all:?}");
    assert_eq!((found[0]["standard"].as_str(), found[0]["version"].as_str()), (Some("aimm"), Some("1.0.0")));
    found[0]["data"][0].clone()
}

fn curve_refund(c: &Contract, id: u64, range: usize, yes: bool, tickets: u128) -> u128 {
    c.market(id).q.pool.as_ref().unwrap().sell_refund(range, yes, tickets)
}

/// A holder's row: the key (prefix, question, name), the length, the balances of every answer, YES and NO, and NEAR's 40
/// bytes for a record.
fn row(name: &str, answers: u64) -> u128 {
    storage_cost(1 + 8 + 4 + name.len() as u64 + 4 + 2 * answers * 16 + 40)
}

fn balance(c: &Contract, id: u64) -> u128 {
    c.get_question(id).storage.0
}

/// What a call could change, as the views show it.
fn state_of(c: &Contract, id: u64) -> String {
    let people: Vec<_> = ["alice.near", "bob.near", "operator.near"].iter().map(|p| (c.holdings(id, acc(p)), c.pending_of(id, acc(p)))).collect();
    serde_json::to_string(&(c.get_question(id), c.book(id), people)).unwrap()
}

fn mock_with(storage: HashMap<Vec<u8>, Vec<u8>>) {
    let mut b = VMContextBuilder::new();
    b.current_account_id(acc("aimm.near"));
    env::set_blockchain_interface(near_sdk::MockedBlockchain::new(
        b.build(),
        once(near_sdk::test_vm_config),
        near_sdk::RuntimeFeesConfig::test(),
        vec![],
        storage,
        Default::default(),
        None,
    ));
}

/// Runs a call that must fail and undoes it as the chain undoes a panic: whatever it wrote is dropped. Returns the
/// contract as it stood, read back from storage, and the panic's message.
fn refused(c: Contract, call: impl FnOnce(&mut Contract)) -> (Contract, String) {
    env::state_write(&c);
    drop(c);
    let saved = with_mocked_blockchain(|b| b.take_storage());
    mock_with(saved.clone());
    let mut c: Contract = env::state_read().unwrap();
    let r = catch_unwind(AssertUnwindSafe(|| call(&mut c)));
    std::mem::forget(c);
    let msg = *r.expect_err("the call went through").downcast::<String>().unwrap();
    mock_with(saved);
    (env::state_read().unwrap(), msg)
}

/// One yoctoNEAR short of `need` the call is refused and changes nothing, and anyone can add that yoctoNEAR; with exactly
/// `need` it goes through and leaves the balance empty.
fn needs_exactly(mut c: Contract, id: u64, need: u128, call: &dyn Fn(&mut Contract)) -> Contract {
    c.market_mut(id).storage = need - 1;
    let before = state_of(&c, id);
    let (mut c, msg) = refused(c, call);
    assert!(msg.contains("storage balance is 1 yoctoNEAR short"), "{msg}");
    assert_eq!(state_of(&c, id), before, "a refused call changes nothing");
    ctx("anyone.near", 1, T0 + 2);
    assert_eq!(c.add_storage(id).0, need);
    call(&mut c);
    assert_eq!(balance(&c, id), 0);
    c
}

/// A state from the contract's own storage, as every call on chain starts: unit tests otherwise run off the maps' cache.
fn reload(c: Contract) -> Contract {
    env::state_write(&c);
    drop(c);
    env::state_read().expect("state")
}

#[test]
fn open_buy_and_view() {
    let (mut c, id) = setup();
    let v = c.get_question(id);
    assert!(v.opened && !v.resolved);
    assert_eq!(v.pot.0, 1_000 * ONE);
    // each price is rounded down, so they add up to 1 less at most one wei per answer
    let total: u128 = v.prices.iter().map(|p| p.0).sum();
    assert!(total <= WAD && WAD - total <= v.prices.len() as u128, "prices add up to {total}");
    let q = c.quote_buy(id, 3, true, U128(100 * ONE));
    let unused = buy(&mut c, "alice.near", 3, true, 100 * ONE, 100 * ONE);
    assert_eq!(unused, 100 * ONE - q.total.0, "the buyer gets back what the trade did not use");
    assert_eq!(c.holdings(id, acc("alice.near")).yes[3].0, 100 * ONE);
    assert_eq!(c.mt_balance_of(acc("alice.near"), "1:yes:3".into()).0, 100 * ONE);
    let v = c.get_question(id);
    assert_eq!(v.pot.0, 1_000 * ONE + q.amount.0);
    assert_eq!(v.fees_owed, q.fee);
    for (i, iv) in v.intervals.iter().enumerate() {
        assert!(iv[0].0 <= v.prices[i].0 && v.prices[i].0 <= iv[1].0);
    }
}

#[test]
#[should_panic(expected = "wrong collateral")]
fn rejects_another_token() {
    let (mut c, _) = setup();
    ctx("fake-usdc.near", 0, T0 + HOUR);
    let _ = c.ft_on_transfer(acc("alice.near"), U128(ONE), r#"{"action":"buy","question_id":1,"range":0,"yes":true,"tickets":"1000","max_cost":"1000000"}"#.into());
}

#[test]
#[should_panic(expected = "above max_cost")]
fn respects_max_cost() {
    let (mut c, _) = setup();
    buy(&mut c, "alice.near", 0, true, 100 * ONE, ONE);
}

#[test]
#[should_panic(expected = "only the operator opens")]
fn only_the_operator_opens() {
    ctx("operator.near", 0, T0);
    let mut c = Contract::new();
    ctx("operator.near", 10u128.pow(23), T0);
    c.create_question(CreateArgs {
        collateral: acc("usdc.near"),
        title: "t".into(),
        answers: vec!["a".into(), "b".into(), "c".into()],
        resolver: acc("oracle.near"),
        fallback_resolver: acc("council.near"),
        close_time: U64(T0 + HOUR),
        resolve_after: U64(T0 + HOUR),
        fallback_after: U64(T0 + HOUR),
        fee_rate: U128(0),
    });
    ctx("usdc.near", 0, T0);
    let _ = c.ft_on_transfer(acc("mallory.near"), U128(ONE), r#"{"action":"open","question_id":1,"depth":"1000","prices":["500000000000000000","300000000000000000","200000000000000000"]}"#.into());
}

#[test]
fn sell_returns_the_quote_and_burns() {
    let (mut c, id) = setup();
    buy(&mut c, "alice.near", 1, false, 300 * ONE, 300 * ONE);
    let quote = c.quote_sell(id, 1, false, U128(100 * ONE));
    ctx("alice.near", 1, T0 + 3 * HOUR);
    let got = c.sell(id, 1, false, U128(100 * ONE), U128(0));
    assert_eq!(got, quote.total);
    assert_eq!(c.holdings(id, acc("alice.near")).no[1].0, 200 * ONE);
    assert_paid(id, "alice.near", got.0);
}

#[test]
#[should_panic(expected = "not enough tokens")]
fn cannot_sell_what_you_do_not_hold() {
    let (mut c, id) = setup();
    ctx("bob.near", 1, T0 + 3 * HOUR);
    c.sell(id, 0, true, U128(ONE), U128(0));
}

#[test]
#[should_panic(expected = "TradingClosed")]
fn no_trading_after_the_close() {
    let (mut c, _) = setup();
    ctx("usdc.near", 0, T0 + 30 * HOUR);
    let _ = c.ft_on_transfer(acc("alice.near"), U128(ONE), r#"{"action":"buy","question_id":1,"range":0,"yes":true,"tickets":"1000","max_cost":"1000000"}"#.into());
}

#[test]
#[should_panic(expected = "StillTrading")]
fn the_resolver_waits_for_the_close() {
    let (mut c, id) = setup();
    ctx("oracle.near", 1, T0 + 3 * HOUR);
    c.resolve(id, 2);
}

#[test]
fn resolve_redeem_and_claim() {
    let (mut c, id) = setup();
    let mut sent = 1_000 * ONE;
    for (who, range, yes, n) in [("alice.near", 3, true, 500), ("bob.near", 3, false, 400), ("carol.near", 0, true, 200)] {
        sent += n * ONE - buy(&mut c, who, range, yes, n * ONE, n * ONE);
    }
    ctx("oracle.near", 1, T0 + 26 * HOUR);
    let rate = c.resolve(id, 3).0;
    let v = c.get_question(id);
    assert_eq!(v.winner, Some(3));
    ctx("alice.near", 1, T0 + 27 * HOUR);
    let a = c.redeem(id, None).0;
    assert_eq!(a, mul_div(500 * ONE, rate, WAD));
    assert_paid(id, "alice.near", a);
    assert_eq!(c.holdings(id, acc("alice.near")).yes[3].0, 0);
    ctx("operator.near", 1, T0 + 27 * HOUR);
    let s = c.claim_surplus(id).0;
    assert_paid(id, "operator.near", s);
    ctx("operator.near", 1, T0 + 27 * HOUR);
    let f = c.claim_fees(id).0;
    assert!(f > 0);
    assert_paid(id, "operator.near", f);
    // bob's NO on 3 and carol's YES on 0 lost: everything taken in is accounted for, to the unit
    let v = c.get_question(id);
    assert_eq!(v.fund.0, 1_000 * ONE);
    assert_eq!(sent, a + s + f + v.pot.0);
    assert!(v.pot.0 <= 1, "only the rounding of one holder is left");
}

#[test]
#[should_panic(expected = "AlreadyClaimed")]
fn surplus_is_claimed_once() {
    let (mut c, id) = setup();
    ctx("oracle.near", 1, T0 + 26 * HOUR);
    c.resolve(id, 0);
    ctx("operator.near", 1, T0 + 27 * HOUR);
    c.claim_surplus(id);
    c.claim_surplus(id);
}

#[test]
fn transfer_moves_tokens() {
    let (mut c, id) = setup();
    buy(&mut c, "alice.near", 2, true, 50 * ONE, 50 * ONE);
    ctx("alice.near", ROW_DEPOSIT, T0 + 3 * HOUR);
    c.mt_transfer(acc("bob.near"), "1:yes:2".into(), U128(20 * ONE), None);
    assert_eq!(c.holdings(id, acc("alice.near")).yes[2].0, 30 * ONE);
    assert_eq!(c.holdings(id, acc("bob.near")).yes[2].0, 20 * ONE);
}

#[test]
#[should_panic(expected = "only the resolver or the fallback resolver")]
fn strangers_cannot_resolve() {
    let (mut c, id) = setup();
    ctx("mallory.near", 1, T0 + 80 * HOUR);
    c.resolve(id, 0);
}

#[test]
fn the_fallback_resolver_can_settle_late() {
    let (mut c, id) = setup();
    ctx("council.near", 1, T0 + 73 * HOUR);
    c.resolve(id, 4);
    assert_eq!(c.get_question(id).winner, Some(4));
}

fn question(c: &mut Contract, op: &str, token: &str, deposit: u128) -> u64 {
    ctx(op, deposit, T0);
    c.create_question(CreateArgs {
        collateral: acc(token),
        title: "t".into(),
        answers: vec!["a".into(), "b".into(), "c".into()],
        resolver: acc("oracle.near"),
        fallback_resolver: acc("council.near"),
        close_time: U64(T0 + 24 * HOUR),
        resolve_after: U64(T0 + 24 * HOUR),
        fallback_after: U64(T0 + 48 * HOUR),
        fee_rate: U128(0),
    })
}

fn open_at(c: &mut Contract, id: u64, op: &str, token: &str) {
    ctx(token, 0, T0 + 1);
    let msg = format!(r#"{{"action":"open","question_id":{id},"depth":"1000000000","prices":["500000000000000000","300000000000000000","200000000000000000"]}}"#);
    let _ = c.ft_on_transfer(acc(op), U128(100 * ONE), msg);
}

fn buy_at(c: &mut Contract, id: u64, token: &str, who: &str) -> u128 {
    ctx(token, 0, T0 + 2);
    let msg = format!(r#"{{"action":"buy","question_id":{id},"range":0,"yes":true,"tickets":"1000","max_cost":"1000000"}}"#);
    match c.ft_on_transfer(acc(who), U128(ONE), msg) {
        PromiseOrValue::Value(v) => v.0,
        _ => panic!("expected a value"),
    }
}

/// The storage balance moves by exactly what the bytes cost, down for a new holder and up when the row goes.
#[test]
fn the_question_pays_for_its_rows_and_gets_them_back() {
    let (mut c, id) = setup();
    let start = c.get_question(id).storage.0;
    buy(&mut c, "alice.near", 0, true, ONE, ONE);
    // the key (prefix, question, name) and the balances of five answers, YES and NO
    let bytes = |name: u64| (1 + 8 + 4 + name + 4 + 10 * 16) + 40;
    assert_eq!(c.get_question(id).storage.0, start - storage_cost(bytes(10)));
    buy(&mut c, "alice.near", 1, true, ONE, ONE);
    assert_eq!(c.get_question(id).storage.0, start - storage_cost(bytes(10)), "a second purchase adds no row");
    ctx("alice.near", ROW_DEPOSIT, T0 + 3 * HOUR);
    c.mt_transfer(acc("bob.near"), "1:yes:0".into(), U128(ONE), None);
    assert_eq!(c.get_question(id).storage.0, start - storage_cost(bytes(10)), "alice paid for bob's row, not the question");
    ctx("alice.near", 1, T0 + 3 * HOUR);
    c.mt_transfer(acc("bob.near"), "1:yes:1".into(), U128(ONE), None);
    assert_eq!(c.get_question(id).storage.0, start, "alice's row is gone and its cost is back");
}

/// A question that runs out of storage stops only itself, and anyone can refill it.
#[test]
fn an_empty_storage_balance_stops_only_its_own_question() {
    let (mut c, good) = setup();
    let id = question(&mut c, "mallory.near", "fake.near", 10u128.pow(22));
    open_at(&mut c, id, "mallory.near", "fake.near");
    let left = c.market(id).storage;
    let row = storage_cost(1 + 8 + 4 + 10 + 4 + 6 * 16 + 40);
    let mut holders = 0;
    while c.market(id).storage >= row {
        buy_at(&mut c, id, "fake.near", &format!("h{holders:04}.near"));
        holders += 1;
        assert!(holders <= left / row, "each holder must take a row from the balance");
    }
    assert!(holders > 0);
    assert_eq!(c.market(id).storage, left - holders as u128 * row);
    let before = c.get_question(good).storage.0;
    buy_at(&mut c, good, "usdc.near", "alice.near");
    assert!(c.get_question(good).storage.0 < before, "the other question goes on, on its own balance");
    ctx("anyone.near", 10u128.pow(22), T0 + 2);
    assert_eq!(c.add_storage(id).0, c.market(id).storage);
    buy_at(&mut c, id, "fake.near", "another.near");
}

#[test]
#[should_panic(expected = "storage balance is")]
fn a_new_holder_needs_storage() {
    let mut c = { ctx("operator.near", 0, T0); Contract::new() };
    let id = question(&mut c, "operator.near", "usdc.near", 10u128.pow(22));
    open_at(&mut c, id, "operator.near", "usdc.near");
    c.market_mut(id).storage = storage_cost(10);
    buy_at(&mut c, id, "usdc.near", "alice.near");
}

/// A fake token can name any sender. It can only spend its own question's balance.
#[test]
#[should_panic(expected = "wrong collateral")]
fn a_token_cannot_reach_another_question() {
    let (mut c, good) = setup();
    buy_at(&mut c, good, "fake.near", "alice.near");
}

#[test]
fn a_payment_holds_a_record_and_a_failed_one_is_kept() {
    let (mut c, id) = setup();
    buy(&mut c, "alice.near", 0, true, 100 * ONE, 100 * ONE);
    let start = c.get_question(id).storage.0;
    ctx("alice.near", 1, T0 + 3 * HOUR);
    let paid = c.sell(id, 0, true, U128(ONE), U128(0)).0;
    let held = pending_row_cost(&acc("alice.near"));
    assert_eq!(c.get_question(id).storage.0, start - held, "the record's cost is held");
    ctx("aimm.near", 0, T0 + 3 * HOUR);
    delivered(&mut c, id, "alice.near", paid);
    assert_eq!(c.get_question(id).storage.0, start, "a good transfer gives it all back");
    assert_eq!(c.pending_of(id, acc("alice.near")).0, 0);

    ctx("alice.near", 1, T0 + 3 * HOUR);
    let paid = c.sell(id, 0, true, U128(ONE), U128(0)).0;
    ctx("aimm.near", 0, T0 + 3 * HOUR);
    let used = env::storage_usage();
    bounced(&mut c, id, "alice.near", paid);
    assert_eq!(storage_cost(env::storage_usage() - used), held, "the hold is the record's exact cost");
    assert_eq!(c.pending_of(id, acc("alice.near")).0, paid);
    assert_eq!(c.get_question(id).storage.0, start - held, "a failed one pays for its record");

    // anyone can push it out again; the row's cost comes back, and the transfer holds a new one
    ctx("bob.near", 1, T0 + 3 * HOUR);
    c.withdraw_pending(id, Some(acc("alice.near")));
    assert_eq!(c.pending_of(id, acc("alice.near")).0, 0);
    assert_eq!(c.get_question(id).storage.0, start - held, "freeing the row pays for the new hold");
    ctx("aimm.near", 0, T0 + 3 * HOUR);
    delivered(&mut c, id, "alice.near", paid);
    assert_eq!(c.get_question(id).storage.0, start);
}

#[test]
#[should_panic(expected = "nothing pending")]
fn nothing_to_withdraw() {
    let (mut c, id) = setup();
    ctx("alice.near", 1, T0);
    c.withdraw_pending(id, None);
}

#[test]
fn the_operator_takes_the_storage_back_after_the_result() {
    let (mut c, id) = setup();
    ctx("oracle.near", 1, T0 + 26 * HOUR);
    c.resolve(id, 0);
    let left = c.get_question(id).storage.0;
    ctx("operator.near", 1, T0 + 26 * HOUR);
    assert_eq!(c.withdraw_storage(id).0, left);
    assert_eq!(c.get_question(id).storage.0, 0);
}

/// The operator need not wait for anyone: a unit on a name nobody controls cannot keep the balance from it, and a winner
/// who comes later pays for their own record with the row they free.
#[test]
fn the_balance_can_go_after_the_result_and_winners_still_redeem() {
    let (mut c, id) = setup();
    buy(&mut c, "alice.near", 0, true, 100 * ONE, 100 * ONE);
    ctx("oracle.near", 1, T0 + 26 * HOUR);
    c.resolve(id, 0);
    ctx("operator.near", 1, T0 + 26 * HOUR);
    let left = c.withdraw_storage(id).0;
    assert!(left > 0);
    assert_eq!(balance(&c, id), 0);
    ctx("alice.near", 1, T0 + 26 * HOUR);
    let a = c.redeem(id, None).0;
    assert_paid(id, "alice.near", a);
    delivered(&mut c, id, "alice.near", a);
    ctx("operator.near", 1, T0 + 26 * HOUR);
    assert_eq!(c.withdraw_storage(id).0, row("alice.near", 5), "what the row left can be taken too");
}

#[test]
#[should_panic(expected = "NotResolved")]
fn storage_stays_before_the_result() {
    let (mut c, id) = setup();
    ctx("operator.near", 1, T0);
    c.withdraw_storage(id);
}

#[test]
#[should_panic(expected = "only the operator")]
fn only_the_operator_takes_the_storage_back() {
    let (mut c, id) = setup();
    ctx("oracle.near", 1, T0 + 26 * HOUR);
    c.resolve(id, 0);
    ctx("mallory.near", 1, T0 + 26 * HOUR);
    c.withdraw_storage(id);
}

#[test]
#[should_panic(expected = "Requires attached deposit of exactly 1 yoctoNEAR")]
fn resolving_needs_a_full_access_key() {
    let (mut c, id) = setup();
    ctx("oracle.near", 0, T0 + 26 * HOUR);
    c.resolve(id, 0);
}

#[test]
#[should_panic(expected = "Requires attached deposit of exactly 1 yoctoNEAR")]
fn halting_needs_a_full_access_key() {
    let (mut c, id) = setup();
    ctx("oracle.near", 0, T0);
    c.halt(id);
}

#[test]
#[should_panic(expected = "TradingClosed")]
fn no_trading_after_a_halt() {
    let (mut c, id) = setup();
    ctx("oracle.near", 1, T0 + 1);
    c.halt(id);
    assert!(c.get_question(id).halted);
    buy(&mut c, "alice.near", 0, true, ONE, ONE);
}

#[test]
#[should_panic(expected = "AlreadyResolved")]
fn a_report_is_final() {
    let (mut c, id) = setup();
    ctx("oracle.near", 1, T0 + 26 * HOUR);
    c.resolve(id, 0);
    c.resolve(id, 1);
}

#[test]
#[should_panic(expected = "AlreadyOpened")]
fn a_question_opens_once() {
    let (mut c, id) = setup();
    assert_eq!(id, 1);
    ctx("usdc.near", 0, T0 + 1);
    let msg = r#"{"action":"open","question_id":1,"depth":"2000000000","prices":["380000000000000000","290000000000000000","190000000000000000","110000000000000000","30000000000000000"]}"#;
    let _ = c.ft_on_transfer(acc("operator.near"), U128(ONE), msg.into());
}

#[test]
#[should_panic(expected = "the receiver is this contract")]
fn tokens_cannot_be_sent_to_the_contract() {
    let (mut c, _) = setup();
    buy(&mut c, "alice.near", 0, true, 100 * ONE, 100 * ONE);
    ctx("alice.near", 1, T0 + 3 * HOUR);
    c.mt_transfer(acc("aimm.near"), "1:yes:0".into(), U128(ONE), None);
}

#[test]
#[should_panic(expected = "plain form")]
fn a_token_id_has_one_spelling() {
    let (mut c, _) = setup();
    buy(&mut c, "alice.near", 2, true, 100 * ONE, 100 * ONE);
    ctx("alice.near", 1, T0 + 3 * HOUR);
    c.mt_transfer(acc("bob.near"), "01:yes:+2".into(), U128(ONE), None);
}

#[test]
#[should_panic(expected = "max_cost")]
fn max_cost_is_a_hard_limit() {
    let (mut c, _) = setup();
    ctx("usdc.near", 0, T0 + 2);
    let _ = c.ft_on_transfer(acc("alice.near"), U128(100 * ONE), r#"{"action":"buy","question_id":1,"range":0,"yes":true,"tickets":"100000000","max_cost":"1"}"#.into());
}

#[test]
#[should_panic(expected = "min_proceeds")]
fn min_proceeds_is_a_hard_limit() {
    let (mut c, id) = setup();
    buy(&mut c, "alice.near", 0, true, 100 * ONE, 100 * ONE);
    ctx("alice.near", 1, T0 + 3 * HOUR);
    c.sell(id, 0, true, U128(ONE), U128(100 * ONE));
}

// ------------------------------------------------------------------------------------------------ money out

#[test]
fn a_sale_pays_the_seller() {
    let (mut c, id) = setup();
    buy(&mut c, "alice.near", 0, true, 300 * ONE, 300 * ONE);
    ctx("alice.near", 1, TRADING);
    let got = c.sell(id, 0, true, U128(100 * ONE), U128(0)).0;
    assert_paid(id, "alice.near", got);
    assert_eq!(c.holdings(id, acc("alice.near")).yes[0].0, 200 * ONE);
}

#[test]
fn a_sale_pays_the_refund_less_the_fee_and_the_fee_is_owed_to_the_operator() {
    let (mut c, id) = setup();
    buy(&mut c, "alice.near", 0, true, 300 * ONE, 300 * ONE);
    let start = c.get_question(id);
    let refund = curve_refund(&c, id, 0, true, 100 * ONE);
    let fee = mul_div_up(refund, WAD / 50, WAD);
    ctx("alice.near", 1, TRADING);
    let got = c.sell(id, 0, true, U128(100 * ONE), U128(0)).0;
    assert_eq!(got, refund - fee);
    assert_paid(id, "alice.near", got);
    let v = c.get_question(id);
    assert_eq!(v.pot.0, start.pot.0 - refund, "the whole refund leaves the pot");
    assert_eq!(v.fees_owed.0, start.fees_owed.0 + fee);
    assert_eq!(v.fees_paid.0, start.fees_paid.0 + fee);
    assert_eq!(v.yes_supply[0].0, 200 * ONE);
}

#[test]
fn a_sale_too_small_to_refund_anything_sends_nothing() {
    let (mut c, id) = setup();
    buy(&mut c, "alice.near", 4, true, 100 * ONE, 100 * ONE);
    ctx("alice.near", 1, TRADING);
    assert_eq!(c.sell(id, 4, true, U128(1), U128(0)).0, 0);
    assert_nothing_sent();
    assert_eq!(c.holdings(id, acc("alice.near")).yes[4].0, 100 * ONE - 1);
}

/// At this depth and on this book the rounding of exp and ln makes one unit's sale raise the computed C by two million
/// units. Refunding 0 there would let a buy and two sales come out ahead.
#[test]
fn a_sale_that_would_raise_the_computed_cost_is_refused() {
    let mut c = {
        ctx("operator.near", 0, T0);
        Contract::new()
    };
    let id = question(&mut c, "operator.near", "usdc.near", 10u128.pow(23));
    ctx("usdc.near", 0, T0 + 1);
    let msg = r#"{"action":"open","question_id":1,"depth":"1000000000000000000000000","prices":["500000000000000000","300000000000000000","200000000000000000"]}"#;
    let _ = c.ft_on_transfer(acc("operator.near"), U128(1 << 90), msg.into());
    buy(&mut c, "alice.near", 1, true, 264_559_923_705_010_875_000_000, 1 << 90);
    buy(&mut c, "alice.near", 2, true, 3_333_331, 1 << 90);
    buy(&mut c, "alice.near", 1, true, 4_000_001, 1 << 90);
    let (c, msg) = refused(c, |c| {
        c.quote_sell(id, 1, true, U128(1));
    });
    assert!(msg.contains("SaleTooSmall"), "{msg}");
    let (_, msg) = refused(c, |c| {
        ctx("alice.near", 1, TRADING);
        c.sell(id, 1, true, U128(1), U128(0));
    });
    assert!(msg.contains("SaleTooSmall"), "{msg}");
}

/// A buy costs the rise in C plus one unit and the sale of the same tokens refunds the fall less one, so the book comes
/// back to where it was and the two differ by at most two units.
#[test]
fn a_purchase_and_its_sale_differ_by_two_units_at_most() {
    let (mut c, id) = setup();
    let book = c.book(id);
    let q = c.quote_buy(id, 1, false, U128(250 * ONE));
    buy(&mut c, "alice.near", 1, false, 250 * ONE, 250 * ONE);
    assert_ne!(c.book(id), book);
    ctx("alice.near", 1, TRADING);
    let s = c.quote_sell(id, 1, false, U128(250 * ONE));
    c.sell(id, 1, false, U128(250 * ONE), U128(0));
    assert_eq!(c.book(id), book);
    assert!(q.amount.0 >= s.amount.0 && q.amount.0 - s.amount.0 <= 2, "{} then {}", q.amount.0, s.amount.0);
}

#[test]
fn fees_are_paid_once_and_include_sales() {
    let (mut c, id) = setup();
    buy(&mut c, "alice.near", 0, true, 300 * ONE, 300 * ONE);
    ctx("alice.near", 1, TRADING);
    c.sell(id, 0, true, U128(100 * ONE), U128(0));
    let v = c.get_question(id);
    assert_eq!(v.fees_owed, v.fees_paid);
    ctx("operator.near", 1, TRADING);
    let f = c.claim_fees(id).0;
    assert_eq!(f, v.fees_owed.0);
    assert_paid(id, "operator.near", f);
    let after = c.get_question(id);
    assert_eq!((after.fees_owed.0, after.fees_paid.0), (0, f), "what was paid stays on the record");
    ctx("operator.near", 1, TRADING);
    assert_eq!(c.claim_fees(id).0, 0);
    assert_nothing_sent();
}

#[test]
fn fees_can_be_claimed_at_any_time_and_never_touch_the_pot() {
    let (mut c, id) = setup();
    buy(&mut c, "alice.near", 0, true, 300 * ONE, 300 * ONE);
    let pot = c.get_question(id).pot.0;
    ctx("operator.near", 1, TRADING);
    let f = c.claim_fees(id).0;
    assert_paid(id, "operator.near", f);
    assert_eq!(c.get_question(id).pot.0, pot);
}

#[test]
fn the_surplus_goes_to_the_operator_and_leaves_the_winners_pot() {
    let (mut c, id) = setup();
    buy(&mut c, "alice.near", 0, true, 300 * ONE, 300 * ONE);
    let pot = c.get_question(id).pot.0;
    ctx("oracle.near", 1, LATE);
    assert_eq!(c.resolve(id, 0).0, WAD);
    let v = c.get_question(id);
    assert_eq!((v.pot.0, v.surplus.0), (300 * ONE, pot - 300 * ONE));
    ctx("operator.near", 1, LATE);
    let s = c.claim_surplus(id).0;
    assert_eq!(s, pot - 300 * ONE);
    assert_paid(id, "operator.near", s);
    assert_eq!(c.get_question(id).surplus.0, 0);
    ctx("alice.near", 1, LATE);
    let a = c.redeem(id, None).0;
    assert_eq!(a, 300 * ONE);
    assert_paid(id, "alice.near", a);
    assert_eq!(c.get_question(id).pot.0, 0);
}

#[test]
fn a_surplus_of_nothing_sends_nothing() {
    let (mut c, id) = funded(ONE);
    buy(&mut c, "alice.near", 3, true, 500 * ONE, 500 * ONE);
    ctx("oracle.near", 1, LATE);
    c.resolve(id, 3);
    assert_eq!(c.get_question(id).surplus.0, 0);
    ctx("operator.near", 1, LATE);
    assert_eq!(c.claim_surplus(id).0, 0);
    assert_nothing_sent();
}

#[test]
fn each_question_pays_in_its_own_token() {
    let (mut c, _) = setup();
    let other = question(&mut c, "operator.near", "dai.near", 10u128.pow(23));
    open_at(&mut c, other, "operator.near", "dai.near");
    buy_at(&mut c, other, "dai.near", "alice.near");
    ctx("alice.near", 1, T0 + 3);
    let got = c.sell(other, 0, true, U128(500), U128(0)).0;
    assert!(got > 0);
    assert_paid_in("dai.near", other, "alice.near", got);
    bounced(&mut c, other, "alice.near", got);
    ctx("alice.near", 1, T0 + 3);
    c.withdraw_pending(other, None);
    assert_paid_in("dai.near", other, "alice.near", got);
}

// ------------------------------------------------------------------------------------------------ the short pot

#[test]
fn a_short_pot_caps_a_yes_sale_at_the_payout() {
    let (mut c, id) = funded(ONE);
    buy(&mut c, "alice.near", 3, true, 500 * ONE, 500 * ONE);
    let r = c.get_question(id).payouts[3].0;
    let cap = mul_div(100 * ONE, r, WAD);
    assert!(r < WAD && cap < curve_refund(&c, id, 3, true, 100 * ONE), "the cap must bind");
    let quote = c.quote_sell(id, 3, true, U128(100 * ONE));
    assert_eq!(quote.amount.0, cap);
    assert_eq!(quote.fee.0, mul_div_up(cap, WAD / 50, WAD));
    assert_eq!(quote.total.0, cap - quote.fee.0);
    ctx("alice.near", 1, TRADING);
    let got = c.sell(id, 3, true, U128(100 * ONE), U128(0)).0;
    assert_eq!(got, quote.total.0);
    assert_paid(id, "alice.near", got);
    assert!(c.get_question(id).payouts[3].0 >= r, "no sale lowers the payout on what it sold");
}

#[test]
fn a_short_pot_caps_a_no_sale_at_the_least_payout() {
    let (mut c, id) = funded(ONE);
    buy(&mut c, "alice.near", 0, true, 400 * ONE, 400 * ONE);
    buy(&mut c, "bob.near", 1, false, 300 * ONE, 300 * ONE);
    let v = c.get_question(id);
    // a NO on 1 pays on every other answer, and answer 0 carries the most tokens
    let least = v.payouts[0].0;
    assert!(least < WAD && least < v.payouts[2].0 && least < v.payouts[1].0);
    let cap = mul_div(100 * ONE, least, WAD);
    assert!(cap < curve_refund(&c, id, 1, false, 100 * ONE), "the cap must bind");
    let quote = c.quote_sell(id, 1, false, U128(100 * ONE));
    assert_eq!(quote.amount.0, cap);
    ctx("bob.near", 1, TRADING);
    let got = c.sell(id, 1, false, U128(100 * ONE), U128(0)).0;
    assert_eq!(got, quote.total.0);
    assert_paid(id, "bob.near", got);
    let after = c.get_question(id);
    for j in [0, 2, 3, 4] {
        assert!(after.payouts[j].0 >= v.payouts[j].0, "no sale lowers the payout on an answer the token pays on");
    }
}

#[test]
fn a_no_sale_is_not_capped_by_the_answer_it_loses_on() {
    let (mut c, id) = funded(ONE);
    buy(&mut c, "alice.near", 1, true, 900 * ONE, 900 * ONE);
    buy(&mut c, "bob.near", 1, false, 100 * ONE, 100 * ONE);
    let v = c.get_question(id);
    assert!(v.payouts[1].0 < WAD && [0, 2, 3, 4].iter().all(|&j| v.payouts[j].0 == WAD));
    let refund = curve_refund(&c, id, 1, false, 50 * ONE);
    assert!(refund > mul_div(50 * ONE, v.payouts[1].0, WAD), "the payout on 1 would cap it");
    assert_eq!(c.quote_sell(id, 1, false, U128(50 * ONE)).amount.0, refund);
}

#[test]
fn a_capped_sale_still_moves_the_curve_the_whole_way() {
    let (mut c, id) = funded(ONE);
    buy(&mut c, "alice.near", 3, true, 500 * ONE, 500 * ONE);
    let before: Vec<i128> = c.book(id).0.iter().map(|x| x.parse().unwrap()).collect();
    ctx("alice.near", 1, TRADING);
    c.sell(id, 3, true, U128(100 * ONE), U128(0));
    let after: Vec<i128> = c.book(id).0.iter().map(|x| x.parse().unwrap()).collect();
    assert_eq!(after[3], before[3] - (100 * ONE) as i128);
}

/// The whole of a short question, each payment as the receipts show it and to the unit.
#[test]
fn a_short_pot_settles_below_one_and_every_payment_is_exact() {
    let (mut c, id) = funded(ONE);
    let mut sent = ONE;
    sent += 500 * ONE - buy(&mut c, "alice.near", 3, true, 500 * ONE, 500 * ONE);
    sent += 300 * ONE - buy(&mut c, "bob.near", 1, false, 300 * ONE, 300 * ONE);
    ctx("alice.near", 1, TRADING);
    let refund = c.sell(id, 3, true, U128(100 * ONE), U128(0)).0;
    assert_paid(id, "alice.near", refund);
    let before = c.get_question(id);
    let owed = 400 * ONE + 300 * ONE;
    assert_eq!(before.payouts[3].0, mul_div(before.pot.0, WAD, owed));

    ctx("oracle.near", 1, LATE);
    let r = c.resolve(id, 3).0;
    assert!(r < WAD);
    assert_eq!(r, before.payouts[3].0);
    let v = c.get_question(id);
    assert_eq!(v.pot.0, mul_div_up(owed, r, WAD));
    assert_eq!(v.surplus.0, before.pot.0 - v.pot.0);

    ctx("alice.near", 1, LATE);
    let a = c.redeem(id, None).0;
    assert_eq!(a, mul_div(400 * ONE, r, WAD));
    assert_paid(id, "alice.near", a);
    ctx("bob.near", 1, LATE);
    let b = c.redeem(id, None).0;
    assert_eq!(b, mul_div(300 * ONE, r, WAD));
    assert_paid(id, "bob.near", b);
    ctx("operator.near", 1, LATE);
    let f = c.claim_fees(id).0;
    assert_eq!(f, before.fees_owed.0);
    assert_paid(id, "operator.near", f);
    ctx("operator.near", 1, LATE);
    let s = c.claim_surplus(id).0;
    assert_eq!(s, v.surplus.0);
    let left = c.get_question(id).pot.0;
    assert_eq!(sent, refund + a + b + f + s + left, "every unit in is out or still in the pot");
    assert!(left <= 2, "what is left is the rounding of two holders");
}

#[test]
fn many_small_holders_are_paid_and_the_rounding_stays_small() {
    let (mut c, id) = funded(ONE);
    let mut holders = vec![];
    for i in 0..30u128 {
        let (name, n, yes) = (format!("h{i:02}.near"), 1_000_003 * (i + 1) + i, i % 2 == 0);
        // YES on 2 and NO on 3 both pay on 2
        buy(&mut c, &name, if yes { 2 } else { 3 }, yes, n, 10 * n);
        holders.push((name, n));
    }
    ctx("oracle.near", 1, LATE);
    let r = c.resolve(id, 2).0;
    assert!(r < WAD);
    let kept = c.get_question(id).pot.0;
    assert_eq!(kept, mul_div_up(holders.iter().map(|h| h.1).sum(), r, WAD));
    let mut paid = 0;
    for (name, n) in &holders {
        ctx(name, 1, LATE);
        let a = c.redeem(id, None).0;
        assert_eq!(a, mul_div(*n, r, WAD));
        assert_paid(id, name, a);
        paid += a;
    }
    let left = c.get_question(id).pot.0;
    assert_eq!(kept, paid + left);
    assert!(left <= holders.len() as u128);
    ctx("operator.near", 1, LATE);
    c.withdraw_storage(id);
}

#[test]
fn nothing_on_the_answer_sends_the_whole_pot_to_the_operator() {
    let (mut c, id) = funded(ONE);
    buy(&mut c, "alice.near", 3, true, 500 * ONE, 500 * ONE);
    let pot = c.get_question(id).pot.0;
    ctx("oracle.near", 1, LATE);
    assert_eq!(c.resolve(id, 0).0, WAD);
    ctx("operator.near", 1, LATE);
    let s = c.claim_surplus(id).0;
    assert_eq!(s, pot);
    assert_paid(id, "operator.near", s);
}

// ------------------------------------------------------------------------------------------------ the NO side

#[test]
fn buying_no_credits_no_and_moves_every_other_answer() {
    let (mut c, id) = setup();
    let before: Vec<i128> = c.book(id).0.iter().map(|x| x.parse().unwrap()).collect();
    buy(&mut c, "alice.near", 2, false, 50 * ONE, 50 * ONE);
    let h = c.holdings(id, acc("alice.near"));
    assert_eq!(h.no[2].0, 50 * ONE);
    assert!(h.yes.iter().all(|x| x.0 == 0));
    assert_eq!(h.no.iter().map(|x| x.0).sum::<u128>(), 50 * ONE);
    let v = c.get_question(id);
    assert_eq!(v.no_supply[2].0, 50 * ONE);
    assert!(v.yes_supply.iter().all(|x| x.0 == 0));
    let after: Vec<i128> = c.book(id).0.iter().map(|x| x.parse().unwrap()).collect();
    for j in 0..5 {
        assert_eq!(after[j] - before[j], if j == 2 { 0 } else { (50 * ONE) as i128 });
    }
}

#[test]
fn buying_yes_moves_only_its_own_answer() {
    let (mut c, id) = setup();
    let before: Vec<i128> = c.book(id).0.iter().map(|x| x.parse().unwrap()).collect();
    buy(&mut c, "alice.near", 2, true, 50 * ONE, 50 * ONE);
    let after: Vec<i128> = c.book(id).0.iter().map(|x| x.parse().unwrap()).collect();
    for j in 0..5 {
        assert_eq!(after[j] - before[j], if j == 2 { (50 * ONE) as i128 } else { 0 });
    }
}

#[test]
fn selling_no_burns_no_and_leaves_yes_alone() {
    let (mut c, id) = setup();
    buy(&mut c, "alice.near", 2, true, 40 * ONE, 40 * ONE);
    buy(&mut c, "alice.near", 2, false, 50 * ONE, 50 * ONE);
    ctx("alice.near", 1, TRADING);
    let got = c.sell(id, 2, false, U128(20 * ONE), U128(0)).0;
    assert_paid(id, "alice.near", got);
    let h = c.holdings(id, acc("alice.near"));
    assert_eq!((h.yes[2].0, h.no[2].0), (40 * ONE, 30 * ONE));
    let v = c.get_question(id);
    assert_eq!((v.yes_supply[2].0, v.no_supply[2].0), (40 * ONE, 30 * ONE));
    // a NO on 2 pays on every other answer, so it left the supply on 0 and not on 2
    assert_eq!(c.market(id).q.winning_supply(0), 30 * ONE);
    assert_eq!(c.market(id).q.winning_supply(2), 40 * ONE);
}

#[test]
#[should_panic(expected = "not enough tokens")]
fn a_yes_holder_cannot_sell_no() {
    let (mut c, id) = setup();
    buy(&mut c, "alice.near", 2, true, 40 * ONE, 40 * ONE);
    ctx("alice.near", 1, TRADING);
    c.sell(id, 2, false, U128(ONE), U128(0));
}

#[test]
#[should_panic(expected = "not enough tokens")]
fn a_no_holder_cannot_sell_yes() {
    let (mut c, id) = setup();
    buy(&mut c, "alice.near", 2, false, 40 * ONE, 40 * ONE);
    ctx("alice.near", 1, TRADING);
    c.sell(id, 2, true, U128(ONE), U128(0));
}

#[test]
fn no_tokens_transfer_as_no() {
    let (mut c, id) = setup();
    buy(&mut c, "alice.near", 2, true, 40 * ONE, 40 * ONE);
    buy(&mut c, "alice.near", 2, false, 50 * ONE, 50 * ONE);
    ctx("alice.near", ROW_DEPOSIT, TRADING);
    c.mt_transfer(acc("bob.near"), "1:no:2".into(), U128(20 * ONE), None);
    let (a, b) = (c.holdings(id, acc("alice.near")), c.holdings(id, acc("bob.near")));
    assert_eq!((a.yes[2].0, a.no[2].0, b.yes[2].0, b.no[2].0), (40 * ONE, 30 * ONE, 0, 20 * ONE));
    assert_eq!(c.mt_balance_of(acc("bob.near"), "1:no:2".into()).0, 20 * ONE);
    assert_eq!(c.mt_balance_of(acc("bob.near"), "1:yes:2".into()).0, 0);
    let v = c.get_question(id);
    assert_eq!((v.yes_supply[2].0, v.no_supply[2].0), (40 * ONE, 50 * ONE), "a transfer changes no supply");
}

#[test]
fn a_transfer_moves_money_nowhere() {
    let (mut c, id) = setup();
    buy(&mut c, "alice.near", 2, false, 50 * ONE, 50 * ONE);
    let before = c.get_question(id);
    let book = c.book(id);
    ctx("alice.near", row("bob.near", 5), TRADING);
    c.mt_transfer(acc("bob.near"), "1:no:2".into(), U128(20 * ONE), None);
    assert_nothing_sent();
    let v = c.get_question(id);
    assert_eq!((v.pot, v.fees_owed), (before.pot, before.fees_owed));
    assert_eq!(c.book(id), book);
}

#[test]
fn a_redemption_pays_every_winning_token_and_drops_the_losing_ones() {
    let (mut c, id) = setup();
    buy(&mut c, "alice.near", 1, true, 50 * ONE, 50 * ONE);
    buy(&mut c, "alice.near", 2, false, 70 * ONE, 70 * ONE);
    buy(&mut c, "alice.near", 4, false, 30 * ONE, 30 * ONE);
    buy(&mut c, "alice.near", 1, false, 11 * ONE, 11 * ONE);
    buy(&mut c, "alice.near", 3, true, 13 * ONE, 13 * ONE);
    ctx("oracle.near", 1, LATE);
    let r = c.resolve(id, 1).0;
    ctx("alice.near", 1, LATE);
    let a = c.redeem(id, None).0;
    assert_eq!(a, mul_div(150 * ONE, r, WAD));
    assert_paid(id, "alice.near", a);
    assert_eq!(logged("redeem")["tickets"], json!((150 * ONE).to_string()));
    let h = c.holdings(id, acc("alice.near"));
    assert!(h.yes.iter().chain(&h.no).all(|x| x.0 == 0), "the whole row goes");
    let v = c.get_question(id);
    assert!(v.yes_supply.iter().chain(&v.no_supply).all(|x| x.0 == 0), "and the supply with it");
    assert_eq!(c.market(id).q.winning_supply(1), 0);
}

#[test]
#[should_panic(expected = "ZeroAmount")]
fn a_no_winner_cannot_redeem_twice() {
    let (mut c, id) = setup();
    buy(&mut c, "alice.near", 2, false, 70 * ONE, 70 * ONE);
    buy(&mut c, "bob.near", 2, false, 70 * ONE, 70 * ONE);
    ctx("oracle.near", 1, LATE);
    c.resolve(id, 1);
    ctx("alice.near", 1, LATE);
    assert_eq!(c.redeem(id, None).0, 70 * ONE);
    c.redeem(id, None);
}

#[test]
#[should_panic(expected = "ZeroAmount")]
fn a_yes_winner_cannot_redeem_twice() {
    let (mut c, id) = setup();
    buy(&mut c, "alice.near", 1, true, 70 * ONE, 70 * ONE);
    buy(&mut c, "bob.near", 1, true, 70 * ONE, 70 * ONE);
    ctx("oracle.near", 1, LATE);
    c.resolve(id, 1);
    ctx("alice.near", 1, LATE);
    assert_eq!(c.redeem(id, None).0, 70 * ONE);
    c.redeem(id, None);
}

#[test]
fn a_no_on_the_winner_pays_nothing_and_its_row_goes() {
    let (mut c, id) = setup();
    buy(&mut c, "alice.near", 1, false, 70 * ONE, 70 * ONE);
    ctx("oracle.near", 1, LATE);
    c.resolve(id, 1);
    let (start, pot) = (balance(&c, id), c.get_question(id).pot.0);
    ctx("anyone.near", 1, LATE);
    assert_eq!(c.redeem(id, Some(acc("alice.near"))).0, 0);
    assert_nothing_sent();
    assert_eq!(logged("redeem"), json!({"id": 1, "holder": "alice.near", "tickets": "0", "amount": "0"}));
    assert_eq!(c.holdings(id, acc("alice.near")).no[1].0, 0);
    let v = c.get_question(id);
    assert_eq!((v.no_supply[1].0, v.pot.0), (0, pot));
    assert_eq!(balance(&c, id), start + row("alice.near", 5));
}

#[test]
fn a_yes_on_a_loser_pays_nothing_and_its_row_goes() {
    let (mut c, id) = setup();
    buy(&mut c, "alice.near", 2, true, 70 * ONE, 70 * ONE);
    ctx("oracle.near", 1, LATE);
    c.resolve(id, 1);
    let start = balance(&c, id);
    ctx("alice.near", 1, LATE);
    assert_eq!(c.redeem(id, None).0, 0);
    assert_eq!(c.get_question(id).yes_supply[2].0, 0);
    assert_eq!(balance(&c, id), start + row("alice.near", 5));
}

/// Anyone can settle a holder, and the money still only goes to the holder.
#[test]
fn a_stranger_can_redeem_for_a_holder_and_the_holder_is_paid() {
    let (mut c, id) = setup();
    buy(&mut c, "alice.near", 2, true, 70 * ONE, 70 * ONE);
    ctx("oracle.near", 1, LATE);
    c.resolve(id, 2);
    ctx("mallory.near", 1, LATE);
    let a = c.redeem(id, Some(acc("alice.near"))).0;
    assert_paid(id, "alice.near", a);
    assert_eq!(logged("redeem")["holder"], "alice.near");
}

/// A name nobody controls can hold a winning token. Settling it leaves the money under that name and nothing stuck.
#[test]
fn a_winner_nobody_controls_can_be_settled_by_anyone() {
    let (mut c, id) = setup();
    buy(&mut c, "dead.near", 2, true, 70 * ONE, 70 * ONE);
    ctx("oracle.near", 1, LATE);
    c.resolve(id, 2);
    ctx("operator.near", 1, LATE);
    let a = c.redeem(id, Some(acc("dead.near"))).0;
    bounced(&mut c, id, "dead.near", a);
    assert_eq!(c.pending_of(id, acc("dead.near")).0, a);
    assert_eq!(c.market(id).q.winning_supply(2), 0);
}

#[test]
#[should_panic(expected = "NotResolved")]
fn nobody_redeems_before_the_result() {
    let (mut c, id) = setup();
    buy(&mut c, "alice.near", 2, true, 70 * ONE, 70 * ONE);
    ctx("alice.near", 1, TRADING);
    c.redeem(id, None);
}

#[test]
fn tokens_moved_after_the_result_are_redeemed_by_whoever_holds_them() {
    let (mut c, id) = setup();
    buy(&mut c, "alice.near", 2, false, 70 * ONE, 70 * ONE);
    ctx("oracle.near", 1, LATE);
    c.resolve(id, 1);
    ctx("alice.near", ROW_DEPOSIT, LATE);
    c.mt_transfer(acc("bob.near"), "1:no:2".into(), U128(30 * ONE), None);
    ctx("bob.near", 1, LATE);
    let got = c.redeem(id, None).0;
    assert_eq!(got, 30 * ONE);
    assert_paid(id, "bob.near", got);
}

#[test]
fn sixteen_answers_keep_yes_and_no_apart() {
    let mut args = five_answers();
    args.answers = (0..16).map(|i| format!("a{i}")).collect();
    let (mut c, id) = create(args);
    ctx("usdc.near", 0, T0 + 1);
    let prices: Vec<String> = (0..16).map(|i| format!("\"{}\"", if i == 0 { 100_000_000_000_000_000u128 } else { 60_000_000_000_000_000 })).collect();
    let msg = format!(r#"{{"action":"open","question_id":1,"depth":"2000000000","prices":[{}]}}"#, prices.join(","));
    assert_eq!(value(c.ft_on_transfer(acc("operator.near"), U128(1_000 * ONE), msg)), 0);
    let start = balance(&c, id);
    buy(&mut c, "alice.near", 15, false, 10 * ONE, 10 * ONE);
    buy(&mut c, "alice.near", 15, true, 20 * ONE, 20 * ONE);
    assert_eq!(balance(&c, id), start - row("alice.near", 16));
    let h = c.holdings(id, acc("alice.near"));
    assert_eq!((h.yes.len(), h.no.len(), h.yes[15].0, h.no[15].0), (16, 16, 20 * ONE, 10 * ONE));
    ctx("alice.near", ROW_DEPOSIT, TRADING);
    c.mt_transfer(acc("bob.near"), "1:no:15".into(), U128(ONE), None);
    assert_eq!(c.mt_balance_of(acc("bob.near"), "1:no:15".into()).0, ONE);
    ctx("oracle.near", 1, LATE);
    c.resolve(id, 0);
    ctx("alice.near", 1, LATE);
    assert_eq!(c.redeem(id, None).0, 9 * ONE);
    assert_eq!(c.holdings(id, acc("alice.near")).yes[15].0, 0);
}

// ------------------------------------------------------------------------------------------------ who may do what

#[test]
#[should_panic(expected = "only the resolver or the operator")]
fn strangers_cannot_halt() {
    let (mut c, id) = setup();
    ctx("mallory.near", 1, T0 + HOUR);
    c.halt(id);
}

#[test]
#[should_panic(expected = "only the resolver or the operator")]
fn the_fallback_resolver_cannot_halt() {
    let (mut c, id) = setup();
    ctx("council.near", 1, T0 + HOUR);
    c.halt(id);
}

#[test]
fn the_operator_can_halt_and_it_stops_the_trading() {
    let (mut c, id) = setup();
    buy(&mut c, "alice.near", 0, true, 100 * ONE, 100 * ONE);
    ctx("operator.near", 1, T0 + HOUR);
    c.halt(id);
    let v = c.get_question(id);
    assert!(v.halted && !v.resolved, "a halt says nothing about the result");
    let (c, msg) = refused(c, |c| {
        ctx("alice.near", 1, TRADING);
        c.sell(id, 0, true, U128(ONE), U128(0));
    });
    assert!(msg.contains("TradingClosed"), "{msg}");
    let (_, msg) = refused(c, |c| {
        c.quote_buy(id, 0, true, U128(ONE));
    });
    assert!(msg.contains("TradingClosed"), "{msg}");
}

#[test]
fn a_second_halt_changes_nothing() {
    let (mut c, id) = setup();
    ctx("oracle.near", 1, T0 + HOUR);
    c.halt(id);
    let before = state_of(&c, id);
    ctx("operator.near", 1, T0 + 2 * HOUR);
    c.halt(id);
    assert_eq!(state_of(&c, id), before);
}

#[test]
#[should_panic(expected = "NotOpen")]
fn an_unopened_question_cannot_be_halted() {
    let (mut c, id) = created();
    ctx("operator.near", 1, T0 + HOUR);
    c.halt(id);
}

#[test]
#[should_panic(expected = "AlreadyResolved")]
fn a_settled_question_cannot_be_halted() {
    let (mut c, id) = setup();
    ctx("oracle.near", 1, LATE);
    c.resolve(id, 0);
    c.halt(id);
}

#[test]
#[should_panic(expected = "only the operator")]
fn strangers_cannot_claim_fees() {
    let (mut c, id) = setup();
    buy(&mut c, "alice.near", 0, true, 100 * ONE, 100 * ONE);
    ctx("mallory.near", 1, TRADING);
    c.claim_fees(id);
}

#[test]
#[should_panic(expected = "only the operator")]
fn the_resolver_cannot_claim_fees() {
    let (mut c, id) = setup();
    buy(&mut c, "alice.near", 0, true, 100 * ONE, 100 * ONE);
    ctx("oracle.near", 1, TRADING);
    c.claim_fees(id);
}

#[test]
#[should_panic(expected = "only the operator")]
fn strangers_cannot_claim_the_surplus() {
    let (mut c, id) = setup();
    ctx("oracle.near", 1, LATE);
    c.resolve(id, 0);
    ctx("mallory.near", 1, LATE);
    c.claim_surplus(id);
}

#[test]
#[should_panic(expected = "only the operator")]
fn the_resolver_cannot_claim_the_surplus() {
    let (mut c, id) = setup();
    ctx("oracle.near", 1, LATE);
    c.resolve(id, 0);
    c.claim_surplus(id);
}

#[test]
#[should_panic(expected = "only the operator")]
fn the_resolver_cannot_take_the_storage_back() {
    let (mut c, id) = setup();
    ctx("oracle.near", 1, LATE);
    c.resolve(id, 0);
    c.withdraw_storage(id);
}

#[test]
#[should_panic(expected = "only the resolver or the fallback resolver")]
fn the_operator_cannot_resolve() {
    let (mut c, id) = setup();
    ctx("operator.near", 1, FALLBACK_AFTER + HOUR);
    c.resolve(id, 0);
}

#[test]
#[should_panic(expected = "only the operator")]
fn an_operator_who_only_signed_cannot_claim_fees() {
    let (mut c, id) = setup();
    ctx_via("mallory.near", "operator.near", 1, TRADING);
    c.claim_fees(id);
}

#[test]
#[should_panic(expected = "only the operator")]
fn an_operator_who_only_signed_cannot_claim_the_surplus() {
    let (mut c, id) = setup();
    ctx("oracle.near", 1, LATE);
    c.resolve(id, 0);
    ctx_via("mallory.near", "operator.near", 1, LATE);
    c.claim_surplus(id);
}

#[test]
#[should_panic(expected = "only the operator")]
fn an_operator_who_only_signed_cannot_take_the_storage_back() {
    let (mut c, id) = setup();
    ctx("oracle.near", 1, LATE);
    c.resolve(id, 0);
    ctx_via("mallory.near", "operator.near", 1, LATE);
    c.withdraw_storage(id);
}

#[test]
#[should_panic(expected = "wrong collateral")]
fn a_contract_is_not_the_token_because_the_token_signed() {
    let (mut c, _) = setup();
    ctx_via("fake.near", "usdc.near", 0, T0 + 2);
    let _ = c.ft_on_transfer(acc("alice.near"), U128(ONE), buy_msg(1, 0, true, 1000, ONE));
}

#[test]
#[should_panic(expected = "only the resolver or the fallback resolver")]
fn a_resolver_who_only_signed_cannot_resolve() {
    let (mut c, id) = setup();
    ctx_via("mallory.near", "oracle.near", 1, LATE);
    c.resolve(id, 0);
}

#[test]
#[should_panic(expected = "only the resolver or the operator")]
fn an_operator_who_only_signed_cannot_halt() {
    let (mut c, id) = setup();
    ctx_via("mallory.near", "operator.near", 1, TRADING);
    c.halt(id);
}

/// Every call here comes through another contract, so the caller and the signer differ: only the caller counts.
#[test]
fn the_caller_counts_not_whoever_signed() {
    ctx("dao.near", 0, T0);
    let mut c = Contract::new();
    ctx_via("dao.near", "user.near", 10u128.pow(23), T0);
    let id = c.create_question(five_answers());
    assert_eq!(c.get_question(id).operator, acc("dao.near"));
    ctx("usdc.near", 0, T0 + 1);
    let _ = c.ft_on_transfer(acc("dao.near"), U128(1_000 * ONE), OPEN.into());
    buy(&mut c, "dao.near", 0, true, 100 * ONE, 100 * ONE);
    ctx_via("dao.near", "user.near", 1, TRADING);
    let got = c.sell(id, 0, true, U128(ONE), U128(0)).0;
    assert_paid(id, "dao.near", got);
    ctx_via("dao.near", "user.near", ROW_DEPOSIT, TRADING);
    c.mt_transfer(acc("bob.near"), "1:yes:0".into(), U128(ONE), None);
    assert_eq!(c.holdings(id, acc("dao.near")).yes[0].0, 98 * ONE);
    ctx_via("dao.near", "user.near", 1, TRADING);
    let f = c.claim_fees(id).0;
    assert_paid(id, "dao.near", f);
    bounced(&mut c, id, "dao.near", 5);
    ctx_via("dao.near", "user.near", 1, TRADING);
    c.withdraw_pending(id, None);
    assert_paid(id, "dao.near", 5);
    ctx("oracle.near", 1, LATE);
    c.resolve(id, 0);
    ctx_via("dao.near", "user.near", 1, LATE);
    let a = c.redeem(id, None).0;
    assert_paid(id, "dao.near", a);
    ctx_via("dao.near", "user.near", 1, LATE);
    let s = c.claim_surplus(id).0;
    assert_paid(id, "dao.near", s);
}

// ------------------------------------------------------------------------------------------------ the clock

#[test]
fn trading_runs_up_to_the_last_moment_before_the_close() {
    let (mut c, id) = setup();
    buy_on(&mut c, CLOSE - 1, "alice.near", 0, true, 100 * ONE, 100 * ONE);
    ctx("alice.near", 1, CLOSE - 1);
    c.sell(id, 0, true, U128(ONE), U128(0));
    assert_eq!(c.holdings(id, acc("alice.near")).yes[0].0, 99 * ONE);
}

#[test]
#[should_panic(expected = "TradingClosed")]
fn no_purchase_at_the_close() {
    let (mut c, _) = setup();
    buy_on(&mut c, CLOSE, "alice.near", 0, true, 100 * ONE, 100 * ONE);
}

#[test]
#[should_panic(expected = "TradingClosed")]
fn no_sale_at_the_close() {
    let (mut c, id) = setup();
    buy(&mut c, "alice.near", 0, true, 100 * ONE, 100 * ONE);
    ctx("alice.near", 1, CLOSE);
    c.sell(id, 0, true, U128(ONE), U128(0));
}

#[test]
#[should_panic(expected = "TradingClosed")]
fn no_sale_after_a_halt() {
    let (mut c, id) = setup();
    buy(&mut c, "alice.near", 0, true, 100 * ONE, 100 * ONE);
    ctx("oracle.near", 1, TRADING);
    c.halt(id);
    ctx("alice.near", 1, TRADING);
    c.sell(id, 0, true, U128(ONE), U128(0));
}

#[test]
#[should_panic(expected = "NotOpen")]
fn no_purchase_after_the_result() {
    let (mut c, id) = setup();
    ctx("oracle.near", 1, LATE);
    c.resolve(id, 0);
    buy(&mut c, "alice.near", 0, true, 100 * ONE, 100 * ONE);
}

#[test]
#[should_panic(expected = "NotOpen")]
fn no_sale_after_the_result() {
    let (mut c, id) = setup();
    buy(&mut c, "alice.near", 1, true, 100 * ONE, 100 * ONE);
    ctx("oracle.near", 1, LATE);
    c.resolve(id, 0);
    ctx("alice.near", 1, LATE);
    c.sell(id, 1, true, U128(ONE), U128(0));
}

#[test]
#[should_panic(expected = "StillTrading")]
fn the_resolver_cannot_report_during_trading() {
    let (mut c, id) = setup();
    ctx("oracle.near", 1, CLOSE - 1);
    c.resolve(id, 0);
}

#[test]
#[should_panic(expected = "TooEarly")]
fn the_resolver_waits_for_resolve_after() {
    let (mut c, id) = setup();
    ctx("oracle.near", 1, RESOLVE_AFTER - 1);
    c.resolve(id, 0);
}

#[test]
fn the_resolver_reports_at_resolve_after() {
    let (mut c, id) = setup();
    ctx("oracle.near", 1, RESOLVE_AFTER);
    c.resolve(id, 0);
    assert_eq!(c.get_question(id).winner, Some(0));
}

#[test]
fn the_resolver_can_still_report_after_fallback_after() {
    let (mut c, id) = setup();
    ctx("oracle.near", 1, FALLBACK_AFTER + HOUR);
    c.resolve(id, 2);
    assert_eq!(c.get_question(id).winner, Some(2));
}

#[test]
#[should_panic(expected = "TooEarly")]
fn the_fallback_resolver_waits_for_fallback_after() {
    let (mut c, id) = setup();
    ctx("council.near", 1, FALLBACK_AFTER - 1);
    c.resolve(id, 0);
}

#[test]
fn the_fallback_resolver_reports_at_fallback_after() {
    let (mut c, id) = setup();
    ctx("council.near", 1, FALLBACK_AFTER);
    c.resolve(id, 3);
    assert_eq!(c.get_question(id).winner, Some(3));
}

#[test]
#[should_panic(expected = "TooEarly")]
fn a_halt_does_not_bring_the_result_forward() {
    let (mut c, id) = setup();
    ctx("oracle.near", 1, T0 + HOUR);
    c.halt(id);
    ctx("oracle.near", 1, RESOLVE_AFTER - 1);
    c.resolve(id, 0);
}

#[test]
#[should_panic(expected = "TooEarly")]
fn a_halt_does_not_bring_the_fallback_forward() {
    let (mut c, id) = setup();
    ctx("oracle.near", 1, T0 + HOUR);
    c.halt(id);
    ctx("council.near", 1, FALLBACK_AFTER - 1);
    c.resolve(id, 0);
}

#[test]
fn the_resolver_reports_after_a_halt_from_resolve_after() {
    let (mut c, id) = setup();
    ctx("operator.near", 1, T0 + HOUR);
    c.halt(id);
    ctx("oracle.near", 1, RESOLVE_AFTER);
    c.resolve(id, 1);
    let v = c.get_question(id);
    assert!(v.halted && v.resolved);
}

#[test]
#[should_panic(expected = "AlreadyResolved")]
fn the_fallback_resolver_cannot_overturn_a_report() {
    let (mut c, id) = setup();
    ctx("oracle.near", 1, LATE);
    c.resolve(id, 0);
    ctx("council.near", 1, FALLBACK_AFTER + HOUR);
    c.resolve(id, 1);
}

#[test]
#[should_panic(expected = "BadRange")]
fn the_winner_must_be_an_answer() {
    let (mut c, id) = setup();
    ctx("oracle.near", 1, LATE);
    c.resolve(id, 5);
}

#[test]
#[should_panic(expected = "NotOpen")]
fn an_unopened_question_cannot_be_resolved() {
    let (mut c, id) = created();
    ctx("oracle.near", 1, LATE);
    c.resolve(id, 0);
}

#[test]
#[should_panic(expected = "NotResolved")]
fn no_redemption_before_the_result() {
    let (mut c, id) = setup();
    buy(&mut c, "alice.near", 0, true, 100 * ONE, 100 * ONE);
    ctx("alice.near", 1, CLOSE);
    c.redeem(id, None);
}

#[test]
#[should_panic(expected = "NotResolved")]
fn no_surplus_before_the_result() {
    let (mut c, id) = setup();
    ctx("operator.near", 1, CLOSE);
    c.claim_surplus(id);
}

// ------------------------------------------------------------------------------------------------ money in

#[test]
#[should_panic(expected = "wrong collateral")]
fn a_fund_in_another_token_is_refused() {
    let (mut c, _) = created();
    ctx("fake.near", 0, T0 + 1);
    let _ = c.ft_on_transfer(acc("operator.near"), U128(ONE), OPEN.into());
}

#[test]
fn the_fund_is_kept_whole_and_recorded() {
    let (mut c, id) = created();
    ctx("usdc.near", 0, T0 + 1);
    assert_eq!(value(c.ft_on_transfer(acc("operator.near"), U128(1_234 * ONE), OPEN.into())), 0);
    let v = c.get_question(id);
    assert_eq!((v.fund.0, v.pot.0), (1_234 * ONE, 1_234 * ONE));
    assert!(v.opened);
}

#[test]
fn unused_money_comes_back_to_the_unit() {
    let (mut c, _) = setup();
    let q = c.quote_buy(1, 2, true, U128(10 * ONE));
    assert!(q.fee.0 > 0);
    assert_eq!(q.total.0, q.amount.0 + q.fee.0);
    assert_eq!(buy(&mut c, "alice.near", 2, true, 10 * ONE, q.total.0 + 12_345), 12_345);
    let q = c.quote_buy(1, 2, true, U128(10 * ONE));
    assert_eq!(buy(&mut c, "alice.near", 2, true, 10 * ONE, q.total.0), 0, "the exact total leaves nothing over");
}

#[test]
#[should_panic(expected = "above the amount sent")]
fn the_amount_sent_must_cover_the_total() {
    let (mut c, _) = setup();
    let q = c.quote_buy(1, 2, true, U128(10 * ONE));
    ctx("usdc.near", 0, T0 + 2);
    let _ = c.ft_on_transfer(acc("alice.near"), U128(q.total.0 - 1), buy_msg(1, 2, true, 10 * ONE, u128::MAX));
}

#[test]
fn max_cost_can_equal_the_total() {
    let (mut c, id) = setup();
    let q = c.quote_buy(1, 2, true, U128(10 * ONE));
    ctx("usdc.near", 0, T0 + 2);
    assert_eq!(value(c.ft_on_transfer(acc("alice.near"), U128(q.total.0 + 9), buy_msg(1, 2, true, 10 * ONE, q.total.0))), 9);
    assert_eq!(c.holdings(id, acc("alice.near")).yes[2].0, 10 * ONE);
}

#[test]
#[should_panic(expected = "above max_cost")]
fn max_cost_one_below_the_total_is_refused() {
    let (mut c, _) = setup();
    let q = c.quote_buy(1, 2, true, U128(10 * ONE));
    ctx("usdc.near", 0, T0 + 2);
    let _ = c.ft_on_transfer(acc("alice.near"), U128(q.total.0 + 9), buy_msg(1, 2, true, 10 * ONE, q.total.0 - 1));
}

#[test]
fn min_proceeds_can_equal_the_proceeds() {
    let (mut c, id) = setup();
    buy(&mut c, "alice.near", 0, true, 100 * ONE, 100 * ONE);
    let q = c.quote_sell(id, 0, true, U128(ONE));
    ctx("alice.near", 1, TRADING);
    assert_eq!(c.sell(id, 0, true, U128(ONE), q.total), q.total);
}

#[test]
#[should_panic(expected = "below min_proceeds")]
fn min_proceeds_one_above_the_proceeds_is_refused() {
    let (mut c, id) = setup();
    buy(&mut c, "alice.near", 0, true, 100 * ONE, 100 * ONE);
    let q = c.quote_sell(id, 0, true, U128(ONE));
    ctx("alice.near", 1, TRADING);
    c.sell(id, 0, true, U128(ONE), U128(q.total.0 + 1));
}

#[test]
fn a_message_naming_a_receiver_is_refused() {
    let (mut c, id) = setup();
    ctx("usdc.near", 0, T0 + 2);
    let msg = r#"{"action":"buy","question_id":1,"range":0,"yes":false,"tickets":"1000000","max_cost":"1000000","receiver_id":"bob.near"}"#;
    let (c, why) = refused(c, |c| {
        let _ = c.ft_on_transfer(acc("alice.near"), U128(ONE), msg.into());
    });
    assert!(why.contains("bad message"), "{why}");
    assert_eq!(c.holdings(id, acc("alice.near")).no[0].0, 0);
}

#[test]
#[should_panic(expected = "bad message")]
fn a_message_that_is_not_understood_is_refused() {
    let (mut c, _) = setup();
    ctx("usdc.near", 0, T0 + 2);
    let _ = c.ft_on_transfer(acc("alice.near"), U128(ONE), r#"{"action":"donate"}"#.into());
}

#[test]
#[should_panic(expected = "no such question")]
fn money_for_a_question_that_does_not_exist_is_refused() {
    let (mut c, _) = setup();
    ctx("usdc.near", 0, T0 + 2);
    let _ = c.ft_on_transfer(acc("alice.near"), U128(ONE), buy_msg(9, 0, true, 1000, ONE));
}

#[test]
#[should_panic(expected = "NotOpen")]
fn no_purchase_before_the_opening() {
    let (mut c, _) = created();
    buy(&mut c, "alice.near", 0, true, 1000, ONE);
}

#[test]
#[should_panic(expected = "ZeroAmount")]
fn no_purchase_of_nothing() {
    let (mut c, _) = setup();
    buy(&mut c, "alice.near", 0, true, 0, ONE);
}

#[test]
#[should_panic(expected = "BadRange")]
fn no_purchase_of_an_answer_that_does_not_exist() {
    let (mut c, _) = setup();
    buy(&mut c, "alice.near", 5, true, 1000, ONE);
}

#[test]
#[should_panic(expected = "TooManyTickets")]
fn no_purchase_above_the_limit_of_one_trade() {
    let (mut c, _) = setup();
    buy(&mut c, "alice.near", 0, true, (1 << 100) + 1, u128::MAX);
}

#[test]
fn a_purchase_of_exactly_the_limit_goes_through_and_can_be_read() {
    let (mut c, id) = setup();
    buy(&mut c, "alice.near", 4, true, 1 << 100, u128::MAX / 2);
    let v = c.get_question(id);
    assert_eq!(v.yes_supply[4].0, 1 << 100);
    assert!(v.prices[4].0 > WAD - 10, "the book is all on one answer");
    assert_eq!(v.intervals.len(), 5);
}

#[test]
#[should_panic(expected = "TooManyTickets")]
fn no_answer_carries_more_than_the_limit() {
    let (mut c, _) = setup();
    buy(&mut c, "alice.near", 4, true, 1 << 100, u128::MAX / 2);
    buy(&mut c, "bob.near", 4, true, 1, u128::MAX / 2);
}

#[test]
#[should_panic(expected = "TooManyTickets")]
fn no_answer_carries_more_than_the_limit_through_no_tokens() {
    let (mut c, _) = setup();
    buy(&mut c, "alice.near", 4, false, 1 << 100, u128::MAX / 2);
    buy(&mut c, "bob.near", 4, false, 1, u128::MAX / 2);
}

#[test]
#[should_panic(expected = "TooManyTickets")]
fn no_quote_above_the_limit_of_one_trade() {
    let (c, id) = setup();
    c.quote_buy(id, 0, true, U128((1 << 100) + 1));
}

#[test]
fn one_token_is_never_free() {
    let (mut c, id) = setup();
    for (range, yes) in [(0, true), (0, false), (4, true), (4, false)] {
        let q = c.quote_buy(id, range, yes, U128(1));
        assert!(q.amount.0 >= 1 && q.fee.0 >= 1, "{}+{}", q.amount.0, q.fee.0);
        let unused = buy(&mut c, "alice.near", range, yes, 1, ONE);
        assert_eq!(unused, ONE - q.total.0);
    }
}

#[test]
#[should_panic(expected = "BadPrices")]
fn opening_prices_must_add_up_to_exactly_one() {
    let (mut c, _) = created();
    ctx("usdc.near", 0, T0 + 1);
    let msg = r#"{"action":"open","question_id":1,"depth":"2000000000","prices":["380000000000000000","290000000000000000","190000000000000000","110000000000000000","29999999999999999"]}"#;
    let _ = c.ft_on_transfer(acc("operator.near"), U128(ONE), msg.into());
}

#[test]
#[should_panic(expected = "BadPrices")]
fn an_opening_price_is_at_least_one_in_a_million() {
    let (mut c, _) = created();
    ctx("usdc.near", 0, T0 + 1);
    let msg = r#"{"action":"open","question_id":1,"depth":"2000000000","prices":["380000000000000000","290000000000000000","190000000000000000","139999000000000001","999999999999"]}"#;
    let _ = c.ft_on_transfer(acc("operator.near"), U128(ONE), msg.into());
}

#[test]
fn an_opening_price_of_exactly_one_in_a_million_is_taken() {
    let (mut c, id) = created();
    ctx("usdc.near", 0, T0 + 1);
    let msg = r#"{"action":"open","question_id":1,"depth":"2000000000","prices":["380000000000000000","290000000000000000","190000000000000000","139999000000000000","1000000000000"]}"#;
    let _ = c.ft_on_transfer(acc("operator.near"), U128(ONE), msg.into());
    assert!(c.get_question(id).opened);
}

#[test]
#[should_panic(expected = "BadRange")]
fn an_opening_needs_a_price_for_each_answer() {
    let (mut c, _) = created();
    ctx("usdc.near", 0, T0 + 1);
    let msg = r#"{"action":"open","question_id":1,"depth":"2000000000","prices":["500000000000000000","300000000000000000","200000000000000000"]}"#;
    let _ = c.ft_on_transfer(acc("operator.near"), U128(ONE), msg.into());
}

#[test]
#[should_panic(expected = "BadDepth")]
fn an_opening_needs_a_depth() {
    let (mut c, _) = created();
    ctx("usdc.near", 0, T0 + 1);
    let msg = r#"{"action":"open","question_id":1,"depth":"0","prices":["380000000000000000","290000000000000000","190000000000000000","110000000000000000","30000000000000000"]}"#;
    let _ = c.ft_on_transfer(acc("operator.near"), U128(ONE), msg.into());
}

#[test]
#[should_panic(expected = "BadDepth")]
fn an_opening_depth_has_a_limit() {
    let (mut c, _) = created();
    ctx("usdc.near", 0, T0 + 1);
    let msg = format!(r#"{{"action":"open","question_id":1,"depth":"{}","prices":["380000000000000000","290000000000000000","190000000000000000","110000000000000000","30000000000000000"]}}"#, (1u128 << 100) + 1);
    let _ = c.ft_on_transfer(acc("operator.near"), U128(ONE), msg);
}

#[test]
fn an_opening_depth_of_exactly_the_limit_is_taken() {
    let (mut c, id) = created();
    ctx("usdc.near", 0, T0 + 1);
    let msg = format!(r#"{{"action":"open","question_id":1,"depth":"{}","prices":["380000000000000000","290000000000000000","190000000000000000","110000000000000000","30000000000000000"]}}"#, 1u128 << 100);
    let _ = c.ft_on_transfer(acc("operator.near"), U128(ONE), msg);
    assert_eq!(c.get_question(id).depth.0, 1 << 100);
}

#[test]
#[should_panic(expected = "TradingClosed")]
fn no_opening_at_the_close() {
    let (mut c, _) = created();
    ctx("usdc.near", 0, CLOSE);
    let _ = c.ft_on_transfer(acc("operator.near"), U128(ONE), OPEN.into());
}

// ------------------------------------------------------------------------------------------------ one yoctoNEAR

/// Each method in a state where it goes through with 1 yoctoNEAR, so the deposit alone can refuse it.
fn guarded(method: &str, deposit: u128) {
    let (mut c, id) = setup();
    buy(&mut c, "alice.near", 0, true, 100 * ONE, 100 * ONE);
    match method {
        "sell" => {
            ctx("alice.near", deposit, TRADING);
            c.sell(id, 0, true, U128(ONE), U128(0));
        }
        "halt" => {
            ctx("oracle.near", deposit, TRADING);
            c.halt(id);
        }
        "resolve" => {
            ctx("oracle.near", deposit, LATE);
            c.resolve(id, 0);
        }
        "redeem" => {
            ctx("oracle.near", 1, LATE);
            c.resolve(id, 0);
            ctx("alice.near", deposit, LATE);
            c.redeem(id, None);
        }
        "claim_fees" => {
            ctx("operator.near", deposit, TRADING);
            c.claim_fees(id);
        }
        "claim_surplus" => {
            ctx("oracle.near", 1, LATE);
            c.resolve(id, 0);
            ctx("operator.near", deposit, LATE);
            c.claim_surplus(id);
        }
        "withdraw_pending" => {
            bounced(&mut c, id, "alice.near", ONE);
            ctx("alice.near", deposit, TRADING);
            c.withdraw_pending(id, None);
        }
        "withdraw_storage" => {
            ctx("oracle.near", 1, LATE);
            c.resolve(id, 1);
            ctx("operator.near", deposit, LATE);
            c.withdraw_storage(id);
        }
        _ => unreachable!(),
    }
}

macro_rules! one_yocto {
    ($($name:ident $method:literal)*) => {$(
        mod $name {
            use super::guarded;

            #[test]
            fn goes_through_with_one() {
                guarded($method, 1);
            }

            #[test]
            #[should_panic(expected = "Requires attached deposit of exactly 1 yoctoNEAR")]
            fn is_refused_with_none() {
                guarded($method, 0);
            }

            #[test]
            #[should_panic(expected = "Requires attached deposit of exactly 1 yoctoNEAR")]
            fn is_refused_with_two() {
                guarded($method, 2);
            }
        }
    )*};
}

one_yocto! {
    yocto_sell "sell"
    yocto_halt "halt"
    yocto_resolve "resolve"
    yocto_redeem "redeem"
    yocto_claim_fees "claim_fees"
    yocto_claim_surplus "claim_surplus"
    yocto_withdraw_pending "withdraw_pending"
    yocto_withdraw_storage "withdraw_storage"
}

// ------------------------------------------------------------------------------------------------ tokens

#[test]
#[should_panic(expected = "not enough tokens")]
fn cannot_transfer_what_you_do_not_hold() {
    let (mut c, _) = setup();
    buy(&mut c, "alice.near", 0, true, 100, ONE);
    ctx("alice.near", 1, TRADING);
    c.mt_transfer(acc("carol.near"), "1:yes:0".into(), U128(101), None);
}

#[test]
#[should_panic(expected = "sender is the receiver")]
fn no_transfer_to_yourself() {
    let (mut c, _) = setup();
    buy(&mut c, "alice.near", 0, true, 100, ONE);
    ctx("alice.near", 1, TRADING);
    c.mt_transfer(acc("alice.near"), "1:yes:0".into(), U128(1), None);
}

#[test]
#[should_panic(expected = "ZeroAmount")]
fn no_transfer_of_nothing() {
    let (mut c, _) = setup();
    buy(&mut c, "alice.near", 0, true, 100, ONE);
    ctx("alice.near", 1, TRADING);
    c.mt_transfer(acc("bob.near"), "1:yes:0".into(), U128(0), None);
}

#[test]
#[should_panic(expected = "the receiver is this contract")]
fn no_transfer_to_the_contract() {
    let (mut c, _) = setup();
    buy(&mut c, "alice.near", 0, true, 100, ONE);
    ctx("alice.near", 1, TRADING);
    c.mt_transfer(acc("aimm.near"), "1:yes:0".into(), U128(1), None);
}

#[test]
#[should_panic(expected = "BadRange")]
fn no_transfer_of_an_answer_that_does_not_exist() {
    let (mut c, _) = setup();
    ctx("alice.near", 1, TRADING);
    c.mt_transfer(acc("bob.near"), "1:no:5".into(), U128(1), None);
}

#[test]
#[should_panic(expected = "BadRange")]
fn no_sale_of_an_answer_that_does_not_exist() {
    let (mut c, id) = setup();
    ctx("alice.near", 1, TRADING);
    c.sell(id, 5, true, U128(1), U128(0));
}

#[test]
#[should_panic(expected = "ZeroAmount")]
fn no_sale_of_nothing() {
    let (mut c, id) = setup();
    buy(&mut c, "alice.near", 0, true, 100, ONE);
    ctx("alice.near", 1, TRADING);
    c.sell(id, 0, true, U128(0), U128(0));
}

#[test]
#[should_panic(expected = "not enough tokens")]
fn tokens_of_one_question_do_not_count_on_another() {
    let (mut c, _) = setup();
    let other = question(&mut c, "operator.near", "usdc.near", 10u128.pow(23));
    open_at(&mut c, other, "operator.near", "usdc.near");
    buy(&mut c, "alice.near", 0, true, 100 * ONE, 100 * ONE);
    ctx("alice.near", 1, T0 + 3);
    c.sell(other, 0, true, U128(ONE), U128(0));
}

macro_rules! bad_token_id {
    ($($name:ident $id:literal $msg:literal)*) => {$(
        #[test]
        #[should_panic(expected = $msg)]
        fn $name() {
            let (c, _) = setup();
            c.mt_balance_of(acc("alice.near"), $id.into());
        }
    )*};
}

bad_token_id! {
    a_token_id_needs_three_parts "1:yes" "token id is"
    a_token_id_has_no_fourth_part "1:yes:0:0" "token id is"
    a_token_id_names_a_question "x:yes:0" "bad question id"
    a_token_id_names_a_side "1:maybe:0" "side is yes or no"
    a_token_id_names_an_answer "1:yes:x" "bad answer"
    a_token_id_has_no_leading_zero "1:yes:02" "plain form"
    a_token_id_has_no_plus_sign "+1:yes:2" "plain form"
    a_balance_needs_an_answer_that_exists "1:no:5" "BadRange"
    a_balance_needs_a_question_that_exists "9:yes:0" "no such question"
}

// ------------------------------------------------------------------------------------------------ storage

#[test]
fn a_question_pays_for_its_row_and_keeps_the_rest_as_its_balance() {
    let (c, id) = created();
    let bytes = 1 + 8 + near_sdk::borsh::to_vec(c.market(id)).unwrap().len() as u64 + 40;
    assert_eq!(balance(&c, id), 10u128.pow(23) - storage_cost(bytes));
    assert_nothing_sent();
}

#[test]
fn the_exact_cost_of_the_row_leaves_an_empty_balance() {
    let (mut c, id) = created();
    let cost = 10u128.pow(23) - balance(&c, id);
    ctx("operator.near", cost, T0);
    let next = c.create_question(five_answers());
    assert_eq!(balance(&c, next), 0);
    assert_eq!(balance(&c, id), 10u128.pow(23) - cost, "the first question keeps its balance");
}

#[test]
#[should_panic(expected = "attach at least")]
fn one_yocto_below_the_cost_of_the_row_is_refused() {
    let (mut c, id) = created();
    let cost = 10u128.pow(23) - balance(&c, id);
    ctx("operator.near", cost - 1, T0);
    c.create_question(five_answers());
}

#[test]
fn opening_charges_the_balance_for_the_curve() {
    let (mut c, id) = created();
    let start = balance(&c, id);
    ctx("usdc.near", 0, T0 + 1);
    let _ = c.ft_on_transfer(acc("operator.near"), U128(ONE), OPEN.into());
    // the depth, then q and q0, each with its length
    assert_eq!(balance(&c, id), start - storage_cost(16 + 2 * (4 + 5 * 16)));
}

#[test]
fn opening_needs_the_balance_for_the_curve() {
    let (c, id) = created();
    let c = needs_exactly(c, id, storage_cost(16 + 2 * (4 + 5 * 16)), &|c| {
        ctx("usdc.near", 0, T0 + 1);
        let _ = c.ft_on_transfer(acc("operator.near"), U128(ONE), OPEN.into());
    });
    assert!(c.get_question(id).opened);
}

#[test]
fn a_new_holder_needs_exactly_the_cost_of_the_row() {
    let (c, id) = setup();
    let c = needs_exactly(c, id, row("alice.near", 5), &|c| {
        buy(c, "alice.near", 0, true, 100 * ONE, 100 * ONE);
    });
    assert_eq!(c.holdings(id, acc("alice.near")).yes[0].0, 100 * ONE);
}

#[test]
fn a_holder_who_buys_again_needs_nothing_from_the_balance() {
    let (mut c, id) = setup();
    buy(&mut c, "alice.near", 0, true, 100 * ONE, 100 * ONE);
    c.market_mut(id).storage = 0;
    buy(&mut c, "alice.near", 3, false, 100 * ONE, 100 * ONE);
    assert_eq!(balance(&c, id), 0);
}

/// The caller pays for the row it creates, so a holder of one unit cannot spend the question's balance on names of its
/// choosing, however many it picks.
#[test]
fn a_transfer_to_a_new_holder_is_paid_for_by_the_caller_and_not_by_the_question() {
    let (mut c, id) = setup();
    buy(&mut c, "alice.near", 0, true, 100 * ONE, 100 * ONE);
    let start = balance(&c, id);
    for i in 0..30 {
        let name = format!("h{i:04}.near");
        ctx("alice.near", row(&name, 5) + 9, TRADING);
        c.mt_transfer(acc(&name), "1:yes:0".into(), U128(1), None);
        assert_eq!(near_sent(), [("alice.near".to_string(), 9)], "what was not needed comes back");
    }
    assert_eq!(balance(&c, id), start, "the question paid nothing");
}

#[test]
#[should_panic(expected = "for the receiver's row")]
fn a_transfer_to_a_new_holder_one_yocto_short_is_refused() {
    let (mut c, _) = setup();
    buy(&mut c, "alice.near", 0, true, 100 * ONE, 100 * ONE);
    ctx("alice.near", row("bob.near", 5) - 1, TRADING);
    c.mt_transfer(acc("bob.near"), "1:yes:0".into(), U128(ONE), None);
}

#[test]
fn a_transfer_to_a_holder_costs_one_yocto_and_sends_back_the_rest() {
    let (mut c, id) = setup();
    buy(&mut c, "alice.near", 0, true, 100 * ONE, 100 * ONE);
    buy(&mut c, "bob.near", 0, true, 100 * ONE, 100 * ONE);
    let start = balance(&c, id);
    ctx("alice.near", 1, TRADING);
    c.mt_transfer(acc("bob.near"), "1:yes:0".into(), U128(ONE), None);
    assert_nothing_sent();
    ctx("alice.near", 5, TRADING);
    c.mt_transfer(acc("bob.near"), "1:yes:0".into(), U128(ONE), None);
    assert_eq!(near_sent(), [("alice.near".to_string(), 4)]);
    assert_eq!(balance(&c, id), start);
}

#[test]
#[should_panic(expected = "attach at least 1 yoctoNEAR")]
fn a_transfer_needs_a_deposit_so_a_function_call_key_cannot_make_one() {
    let (mut c, _) = setup();
    buy(&mut c, "alice.near", 0, true, 100 * ONE, 100 * ONE);
    buy(&mut c, "bob.near", 0, true, 100 * ONE, 100 * ONE);
    ctx("alice.near", 0, TRADING);
    c.mt_transfer(acc("bob.near"), "1:yes:0".into(), U128(ONE), None);
}

/// Moving a whole row to a new name pays for the new row and gives the old one to the question. If only the net bytes
/// were paid, a holder could buy, move on for a yoctoNEAR and buy again, and the question would pay for row after row.
#[test]
fn moving_a_whole_row_pays_for_the_new_row_and_gives_the_old_one_to_the_question() {
    let (mut c, id) = setup();
    let start = balance(&c, id);
    for (i, name) in ["h0001.near", "h0002.near", "h0003.near"].iter().enumerate() {
        buy(&mut c, "alice.near", 0, true, 100 * ONE, 100 * ONE);
        assert_eq!(balance(&c, id), start - row("alice.near", 5), "round {i}: the purchase took a row");
        ctx("alice.near", row(name, 5), TRADING);
        c.mt_transfer(acc(name), "1:yes:0".into(), U128(100 * ONE), None);
        assert_eq!(balance(&c, id), start, "round {i}: the question is whole again");
        assert_nothing_sent();
    }
}

#[test]
#[should_panic(expected = "for the receiver's row")]
fn moving_a_whole_row_to_a_new_name_for_a_yocto_is_refused() {
    let (mut c, _) = setup();
    buy(&mut c, "alice.near", 0, true, 100 * ONE, 100 * ONE);
    ctx("alice.near", 1, TRADING);
    c.mt_transfer(acc("bob0.near"), "1:yes:0".into(), U128(100 * ONE), None);
}


#[test]
fn a_transfer_to_an_existing_holder_frees_the_row_of_the_one_who_left() {
    let (mut c, id) = setup();
    buy(&mut c, "alice.near", 0, true, 100 * ONE, 100 * ONE);
    buy(&mut c, "bob.near", 0, true, 100 * ONE, 100 * ONE);
    let start = balance(&c, id);
    ctx("alice.near", 1, TRADING);
    c.mt_transfer(acc("bob.near"), "1:yes:0".into(), U128(100 * ONE), None);
    assert_eq!(balance(&c, id), start + row("alice.near", 5));
    assert!(!c.balances.contains_key(&(id, acc("alice.near"))));
    assert_eq!(c.holdings(id, acc("bob.near")).yes[0].0, 200 * ONE);
}

#[test]
fn a_sale_needs_exactly_the_hold_for_a_record() {
    let (mut c, id) = setup();
    buy(&mut c, "alice.near", 0, true, 100 * ONE, 100 * ONE);
    let c = needs_exactly(c, id, pending_row_cost(&acc("alice.near")), &|c| {
        ctx("alice.near", 1, TRADING);
        c.sell(id, 0, true, U128(ONE), U128(0));
    });
    assert_eq!(c.holdings(id, acc("alice.near")).yes[0].0, 99 * ONE);
}

#[test]
fn a_sale_holds_the_record_of_the_seller() {
    let (mut c, id) = setup();
    let long = "a-rather-longer-name.near";
    buy(&mut c, long, 0, true, 100 * ONE, 100 * ONE);
    needs_exactly(c, id, pending_row_cost(&acc(long)), &|c| {
        ctx(long, 1, TRADING);
        c.sell(id, 0, true, U128(ONE), U128(0));
    });
}

#[test]
fn a_sale_of_everything_frees_the_row_and_gives_it_all_back() {
    let (mut c, id) = setup();
    let start = balance(&c, id);
    buy(&mut c, "alice.near", 0, true, 100 * ONE, 100 * ONE);
    ctx("alice.near", 1, TRADING);
    let got = c.sell(id, 0, true, U128(100 * ONE), U128(0)).0;
    assert!(!c.balances.contains_key(&(id, acc("alice.near"))));
    let hold = pending_row_cost(&acc("alice.near"));
    assert_eq!(balance(&c, id), start - hold, "the row came back, less the hold");
    delivered(&mut c, id, "alice.near", got);
    assert_eq!(balance(&c, id), start);
}

#[test]
fn a_sale_that_frees_a_row_pays_its_own_hold_from_an_empty_balance() {
    let (mut c, id) = setup();
    buy(&mut c, "alice.near", 0, true, 100 * ONE, 100 * ONE);
    c.market_mut(id).storage = 0;
    ctx("alice.near", 1, TRADING);
    c.sell(id, 0, true, U128(100 * ONE), U128(0));
    assert_eq!(balance(&c, id), row("alice.near", 5) - pending_row_cost(&acc("alice.near")));
}

#[test]
fn a_redemption_that_empties_the_row_gives_it_all_back() {
    let (mut c, id) = setup();
    let start = balance(&c, id);
    buy(&mut c, "alice.near", 2, true, 100 * ONE, 100 * ONE);
    ctx("oracle.near", 1, LATE);
    c.resolve(id, 2);
    ctx("alice.near", 1, LATE);
    let a = c.redeem(id, None).0;
    assert!(!c.balances.contains_key(&(id, acc("alice.near"))));
    assert_eq!(balance(&c, id), start - pending_row_cost(&acc("alice.near")));
    delivered(&mut c, id, "alice.near", a);
    assert_eq!(balance(&c, id), start);
}

#[test]
fn a_winner_who_also_holds_a_loser_needs_nothing_from_the_balance() {
    let (mut c, id) = setup();
    buy(&mut c, "alice.near", 2, true, 100 * ONE, 100 * ONE);
    buy(&mut c, "alice.near", 3, true, 100 * ONE, 100 * ONE);
    ctx("oracle.near", 1, LATE);
    c.resolve(id, 2);
    c.market_mut(id).storage = 0;
    ctx("alice.near", 1, LATE);
    assert_eq!(c.redeem(id, None).0, 100 * ONE);
    assert!(!c.balances.contains_key(&(id, acc("alice.near"))));
    assert_eq!(balance(&c, id), row("alice.near", 5) - pending_row_cost(&acc("alice.near")));
}

/// The smallest row on the smallest question, at both ends of a name's length: a holder can always redeem.
#[test]
fn a_redemption_needs_nothing_from_the_balance_at_any_name_length() {
    for name in ["ab".to_string(), "a".repeat(64)] {
        let mut c = {
            mock_with(HashMap::new());
            ctx("operator.near", 0, T0);
            Contract::new()
        };
        let id = question(&mut c, "operator.near", "usdc.near", 10u128.pow(22));
        open_at(&mut c, id, "operator.near", "usdc.near");
        buy_at(&mut c, id, "usdc.near", &name);
        ctx("oracle.near", 1, T0 + 24 * HOUR);
        c.resolve(id, 0);
        c.market_mut(id).storage = 0;
        ctx(&name, 1, T0 + 24 * HOUR);
        assert!(c.redeem(id, None).0 > 0);
        assert_eq!(balance(&c, id), row(&name, 3) - pending_row_cost(&acc(&name)));
    }
}

#[test]
fn claiming_fees_needs_exactly_the_hold_for_a_record() {
    let (mut c, id) = setup();
    buy(&mut c, "alice.near", 0, true, 100 * ONE, 100 * ONE);
    needs_exactly(c, id, pending_row_cost(&acc("operator.near")), &|c| {
        ctx("operator.near", 1, TRADING);
        c.claim_fees(id);
    });
}

#[test]
fn claiming_the_surplus_needs_exactly_the_hold_for_a_record() {
    let (mut c, id) = setup();
    ctx("oracle.near", 1, LATE);
    c.resolve(id, 0);
    needs_exactly(c, id, pending_row_cost(&acc("operator.near")), &|c| {
        ctx("operator.near", 1, LATE);
        c.claim_surplus(id);
    });
}

#[test]
fn claiming_nothing_needs_no_hold() {
    let (mut c, id) = setup();
    c.market_mut(id).storage = 0;
    ctx("operator.near", 1, TRADING);
    assert_eq!(c.claim_fees(id).0, 0);
    assert_eq!(balance(&c, id), 0);
}

#[test]
fn withdrawing_a_pending_amount_works_on_an_empty_balance() {
    let (mut c, id) = setup();
    bounced(&mut c, id, "alice.near", 5);
    c.market_mut(id).storage = 0;
    ctx("alice.near", 1, TRADING);
    c.withdraw_pending(id, None);
    assert_paid(id, "alice.near", 5);
    assert_eq!(balance(&c, id), 0, "the freed record pays for the new hold");
    delivered(&mut c, id, "alice.near", 5);
    assert_eq!(balance(&c, id), pending_row_cost(&acc("alice.near")));
}

/// The hold is computed from the name, so it must match what the row really takes, at any length a name can have.
#[test]
fn the_hold_is_the_measured_cost_of_the_record_at_every_name_length() {
    let mut names = vec!["ab".to_string(), "alice.near".to_string(), "a".repeat(64)];
    names.push(format!("{}.near", "b".repeat(59)));
    for name in names {
        let (mut c, id) = setup();
        buy(&mut c, &name, 0, true, 100 * ONE, 100 * ONE);
        let start = balance(&c, id);
        ctx(&name, 1, TRADING);
        let got = c.sell(id, 0, true, U128(ONE), U128(0)).0;
        let hold = pending_row_cost(&acc(&name));
        assert_eq!(balance(&c, id), start - hold);
        callback(PromiseResult::Failed);
        let before = env::storage_usage();
        c.on_paid(id, acc(&name), U128(got));
        assert_eq!(storage_cost(env::storage_usage() - before), hold, "{name}");
        assert_eq!(balance(&c, id), start - hold, "the hold paid for the record exactly");
        assert_eq!(c.pending_of(id, acc(&name)).0, got);
    }
}

/// `ft_transfer` returns nothing, but a token that returns something anyway has still paid. The result is not read.
#[test]
fn a_token_that_returns_a_value_still_gets_its_hold_back() {
    for result in [b"true".to_vec(), b"garbage".to_vec(), vec![b'x'; 4096]] {
        let (mut c, id) = setup();
        buy(&mut c, "alice.near", 0, true, 100 * ONE, 100 * ONE);
        let start = balance(&c, id);
        ctx("alice.near", 1, TRADING);
        let got = c.sell(id, 0, true, U128(ONE), U128(0)).0;
        callback(PromiseResult::Successful(result));
        c.on_paid(id, acc("alice.near"), U128(got));
        assert_eq!(balance(&c, id), start);
        assert_eq!(c.pending_of(id, acc("alice.near")).0, 0);
    }
}

/// A second failure adds to the record and takes no bytes, so the hold comes back whole.
#[test]
fn a_second_failed_payment_adds_to_the_record_and_gives_the_hold_back() {
    let (mut c, id) = setup();
    buy(&mut c, "alice.near", 0, true, 100 * ONE, 100 * ONE);
    let start = balance(&c, id);
    ctx("alice.near", 1, TRADING);
    let first = c.sell(id, 0, true, U128(ONE), U128(0)).0;
    bounced(&mut c, id, "alice.near", first);
    ctx("alice.near", 1, TRADING);
    let second = c.sell(id, 0, true, U128(ONE), U128(0)).0;
    let hold = pending_row_cost(&acc("alice.near"));
    assert_eq!(balance(&c, id), start - 2 * hold);
    bounced(&mut c, id, "alice.near", second);
    assert_eq!(balance(&c, id), start - hold, "one record, one hold spent");
    assert_eq!(c.pending_of(id, acc("alice.near")).0, first + second);
}

#[test]
fn anyone_can_add_to_the_balance_and_it_adds_exactly_what_is_attached() {
    let (mut c, id) = setup();
    let start = balance(&c, id);
    ctx("anyone.near", 12_345, T0);
    assert_eq!(c.add_storage(id).0, start + 12_345);
    assert_eq!(balance(&c, id), start + 12_345);
    assert_nothing_sent();
}

#[test]
#[should_panic(expected = "no such question")]
fn the_balance_of_a_question_that_does_not_exist_cannot_be_added_to() {
    let (mut c, _) = setup();
    ctx("anyone.near", 1, T0);
    c.add_storage(9);
}

/// Refused, the question stops; its neighbour goes on from its own balance, and is not a yoctoNEAR poorer.
#[test]
fn a_refusal_on_one_question_takes_nothing_from_another() {
    let (mut c, short) = setup();
    let other = question(&mut c, "operator.near", "usdc.near", 10u128.pow(23));
    open_at(&mut c, other, "operator.near", "usdc.near");
    c.market_mut(short).storage = row("alice.near", 5) - 1;
    let full = balance(&c, other);
    let (mut c, msg) = refused(c, |c| {
        buy(c, "alice.near", 0, true, 100 * ONE, 100 * ONE);
    });
    assert!(msg.contains("storage balance is 1 yoctoNEAR short"), "{msg}");
    assert_eq!(balance(&c, other), full);
    buy_at(&mut c, other, "usdc.near", "alice.near");
    assert_eq!(balance(&c, other), full - row("alice.near", 3));
    assert_eq!(balance(&c, short), row("alice.near", 5) - 1);
}

#[test]
fn no_call_touches_the_balance_of_another_question() {
    let (mut c, id) = setup();
    let other = question(&mut c, "operator.near", "dai.near", 10u128.pow(23));
    open_at(&mut c, other, "operator.near", "dai.near");
    let full = balance(&c, other);
    buy(&mut c, "alice.near", 0, true, 100 * ONE, 100 * ONE);
    assert_eq!(balance(&c, other), full, "a new holder");
    ctx("alice.near", ROW_DEPOSIT, TRADING);
    c.mt_transfer(acc("carol.near"), "1:yes:0".into(), U128(ONE), None);
    assert_eq!(balance(&c, other), full, "a transfer");
    ctx("alice.near", 1, TRADING);
    let got = c.sell(id, 0, true, U128(ONE), U128(0)).0;
    assert_eq!(balance(&c, other), full, "a hold");
    bounced(&mut c, id, "alice.near", got);
    assert_eq!(balance(&c, other), full, "a record");
    ctx("alice.near", 1, TRADING);
    c.withdraw_pending(id, None);
    assert_eq!(balance(&c, other), full, "a record freed");
    delivered(&mut c, id, "alice.near", got);
    assert_eq!(balance(&c, other), full, "a hold given back");
    ctx("carol.near", 1, TRADING);
    c.sell(id, 0, true, U128(ONE), U128(0));
    assert_eq!(balance(&c, other), full, "a row freed");
    ctx("anyone.near", 5, TRADING);
    c.add_storage(id);
    assert_eq!(balance(&c, other), full, "a top-up");
    ctx("oracle.near", 1, LATE);
    c.resolve(id, 0);
    ctx("alice.near", 1, LATE);
    c.redeem(id, None);
    ctx("operator.near", 1, LATE);
    c.claim_fees(id);
    assert_eq!(balance(&c, other), full, "the claims");
    ctx("operator.near", 1, LATE);
    c.withdraw_storage(id);
    assert_eq!(balance(&c, other), full, "the balance taken back");
}

#[test]
fn the_operator_takes_the_storage_back_in_near_once() {
    let (mut c, id) = setup();
    ctx("oracle.near", 1, LATE);
    c.resolve(id, 0);
    let left = balance(&c, id);
    ctx("operator.near", 1, LATE);
    assert_eq!(c.withdraw_storage(id).0, left);
    assert_eq!(near_sent(), [("operator.near".to_string(), left)]);
    ctx("operator.near", 1, LATE);
    assert_eq!(c.withdraw_storage(id).0, 0);
    assert_nothing_sent();
}

#[test]
fn losing_tokens_do_not_keep_the_storage_in_the_contract() {
    let (mut c, id) = setup();
    buy(&mut c, "alice.near", 2, true, 100 * ONE, 100 * ONE);
    buy(&mut c, "alice.near", 0, false, 100 * ONE, 100 * ONE);
    ctx("oracle.near", 1, LATE);
    c.resolve(id, 0);
    let left = balance(&c, id);
    ctx("operator.near", 1, LATE);
    assert_eq!(c.withdraw_storage(id).0, left);
}

#[test]
fn a_payment_still_on_its_way_when_the_storage_is_taken_comes_back_and_can_be_taken_too() {
    let (mut c, id) = setup();
    buy(&mut c, "alice.near", 0, true, 100 * ONE, 100 * ONE);
    ctx("oracle.near", 1, LATE);
    c.resolve(id, 0);
    ctx("alice.near", 1, LATE);
    let a = c.redeem(id, None).0;
    ctx("operator.near", 1, LATE);
    c.withdraw_storage(id);
    assert_eq!(balance(&c, id), 0);
    delivered(&mut c, id, "alice.near", a);
    let hold = pending_row_cost(&acc("alice.near"));
    assert_eq!(balance(&c, id), hold);
    ctx("operator.near", 1, LATE);
    assert_eq!(c.withdraw_storage(id).0, hold);
}

#[test]
fn a_failed_redemption_can_still_be_withdrawn_after_the_storage_is_taken() {
    let (mut c, id) = setup();
    buy(&mut c, "alice.near", 0, true, 100 * ONE, 100 * ONE);
    ctx("oracle.near", 1, LATE);
    c.resolve(id, 0);
    ctx("alice.near", 1, LATE);
    let a = c.redeem(id, None).0;
    ctx("operator.near", 1, LATE);
    c.withdraw_storage(id);
    bounced(&mut c, id, "alice.near", a);
    assert_eq!(c.pending_of(id, acc("alice.near")).0, a);
    ctx("alice.near", 1, LATE);
    c.withdraw_pending(id, None);
    assert_paid(id, "alice.near", a);
}

#[test]
fn the_storage_of_a_question_never_opened_can_be_taken_back_after_the_close() {
    let (mut c, id) = created();
    let left = balance(&c, id);
    assert!(left > 0);
    ctx("operator.near", 1, CLOSE);
    assert_eq!(c.withdraw_storage(id).0, left);
    assert_eq!(near_sent(), [("operator.near".to_string(), left)]);
}

#[test]
#[should_panic(expected = "NotResolved")]
fn the_storage_of_a_question_that_can_still_open_stays() {
    let (mut c, id) = created();
    ctx("operator.near", 1, CLOSE - 1);
    c.withdraw_storage(id);
}

#[test]
#[should_panic(expected = "NotResolved")]
fn the_storage_of_an_open_question_with_no_result_stays() {
    let (mut c, id) = setup();
    ctx("operator.near", 1, FALLBACK_AFTER + HOUR);
    c.withdraw_storage(id);
}

// ------------------------------------------------------------------------------------------------ failed payments

#[test]
fn a_failed_payment_is_kept_and_a_good_one_is_not() {
    let (mut c, id) = setup();
    bounced(&mut c, id, "alice.near", 5);
    assert_eq!(c.pending_of(id, acc("alice.near")).0, 5);
    bounced(&mut c, id, "alice.near", 7);
    assert_eq!(c.pending_of(id, acc("alice.near")).0, 12);
    delivered(&mut c, id, "alice.near", 100);
    assert_eq!(c.pending_of(id, acc("alice.near")).0, 12);
    delivered(&mut c, id, "bob.near", 100);
    assert_eq!(c.pending_of(id, acc("bob.near")).0, 0);
}

#[test]
fn pending_money_is_per_question_and_per_person() {
    let (mut c, id) = setup();
    let other = question(&mut c, "operator.near", "dai.near", 10u128.pow(23));
    bounced(&mut c, id, "alice.near", 5);
    assert_eq!(c.pending_of(other, acc("alice.near")).0, 0);
    assert_eq!(c.pending_of(id, acc("bob.near")).0, 0);
    let (_, msg) = refused(c, |c| {
        ctx("alice.near", 1, TRADING);
        c.withdraw_pending(other, None);
    });
    assert!(msg.contains("nothing pending"), "{msg}");
}

#[test]
fn a_payout_that_fails_stays_with_the_holder_until_it_is_sent() {
    let (mut c, id) = setup();
    buy(&mut c, "alice.near", 2, true, 100 * ONE, 100 * ONE);
    ctx("oracle.near", 1, LATE);
    c.resolve(id, 2);
    ctx("alice.near", 1, LATE);
    let a = c.redeem(id, None).0;
    assert_paid(id, "alice.near", a);
    bounced(&mut c, id, "alice.near", a);
    assert_eq!(c.pending_of(id, acc("alice.near")).0, a);
    ctx("carol.near", 1, LATE);
    c.withdraw_pending(id, Some(acc("alice.near")));
    assert_paid(id, "alice.near", a);
    assert_eq!(c.pending_of(id, acc("alice.near")).0, 0);
    let (mut c, msg) = refused(c, |c| {
        ctx("alice.near", 1, LATE);
        c.withdraw_pending(id, None);
    });
    assert!(msg.contains("nothing pending"), "the same money cannot go twice: {msg}");
    bounced(&mut c, id, "alice.near", a);
    assert_eq!(c.pending_of(id, acc("alice.near")).0, a, "a second failure keeps it again");
}

#[test]
fn the_callers_own_money_is_sent_when_no_account_is_named() {
    let (mut c, id) = setup();
    bounced(&mut c, id, "alice.near", 5);
    bounced(&mut c, id, "bob.near", 6);
    ctx("bob.near", 1, TRADING);
    c.withdraw_pending(id, None);
    assert_paid(id, "bob.near", 6);
    assert_eq!((c.pending_of(id, acc("alice.near")).0, c.pending_of(id, acc("bob.near")).0), (5, 0));
}

// ------------------------------------------------------------------------------------------------ views

#[test]
fn an_unopened_question_can_be_read() {
    let (c, id) = created();
    let v = c.get_question(id);
    assert_eq!((v.id, v.title.as_str()), (id, "Highest temperature in NYC on December 1"));
    assert_eq!(v.answers, ["49°F or below", "50-51°F", "52-53°F", "54-55°F", "56°F or higher"]);
    assert_eq!(
        (v.collateral, v.operator, v.resolver, v.fallback_resolver),
        (acc("usdc.near"), acc("operator.near"), acc("oracle.near"), acc("council.near"))
    );
    assert_eq!((v.close_time.0, v.resolve_after.0, v.fallback_after.0, v.fee_rate.0), (CLOSE, RESOLVE_AFTER, FALLBACK_AFTER, WAD / 50));
    assert!(!v.opened && !v.halted && !v.resolved && v.winner.is_none());
    for x in [v.depth, v.virtual_liquidity, v.fund, v.pot, v.fees_paid, v.fees_owed, v.surplus] {
        assert_eq!(x.0, 0);
    }
    assert!(v.prices.is_empty() && v.intervals.is_empty());
    assert_eq!(v.payouts, vec![U128(WAD); 5]);
    assert_eq!(v.yes_supply, vec![U128(0); 5]);
    assert_eq!(v.no_supply, vec![U128(0); 5]);
    assert_eq!(c.book(id), (vec![], vec![]));
    assert_eq!(c.question_count(), 1);
}

#[test]
fn an_opened_question_shows_the_curve_it_was_opened_with() {
    let (c, id) = setup();
    let v = c.get_question(id);
    assert!(v.opened && !v.resolved);
    assert_eq!(v.depth.0, 2_000 * ONE);
    let want = [0.38f64, 0.29, 0.19, 0.11, 0.03];
    for (i, p) in want.iter().enumerate() {
        assert!((v.prices[i].0 as f64 / WAD as f64 - p).abs() < 1e-9, "price {i}");
    }
    let (q, q0) = c.book(id);
    assert_eq!(q, q0);
    for (i, p) in want.iter().enumerate() {
        let opening: f64 = q0[i].parse().unwrap();
        assert!((opening - 2e9 * p.ln()).abs() < 2.0, "q0 {i}");
    }
    let l = v.virtual_liquidity.0 as f64;
    assert!((l - 2e9 * (1.0 / 0.03f64).ln()).abs() < 2.0, "L is b ln(1 / min p0): {l}");
}

#[test]
fn a_settled_question_shows_the_result_and_no_prices() {
    let (mut c, id) = funded(ONE);
    buy(&mut c, "alice.near", 3, true, 500 * ONE, 500 * ONE);
    ctx("oracle.near", 1, LATE);
    let r = c.resolve(id, 3).0;
    let v = c.get_question(id);
    assert!(v.resolved);
    assert_eq!(v.winner, Some(3));
    assert!(r < WAD);
    assert_eq!(v.payouts.iter().map(|x| x.0).collect::<Vec<_>>(), [0, 0, 0, r, 0], "a loser is paid nothing");
    assert!(v.prices.is_empty() && v.intervals.is_empty());
    assert_eq!(v.depth.0, 2_000 * ONE, "the curve is still on the record");
    assert_eq!(c.book(id).0.len(), 5);
    let (c, msg) = refused(c, |c| {
        c.quote_buy(id, 0, true, U128(ONE));
    });
    assert!(msg.contains("NotOpen"), "{msg}");
    let (c, msg) = refused(c, |c| {
        c.quote_sell(id, 3, true, U128(ONE));
    });
    assert!(msg.contains("NotOpen"), "{msg}");
    let (_, msg) = refused(c, |c| {
        c.information_intervals(id);
    });
    assert!(msg.contains("NotOpen"), "{msg}");
}

#[test]
fn the_buy_quote_is_what_a_purchase_does() {
    for (range, yes) in [(1, true), (1, false)] {
        let (mut c, id) = setup();
        let q = c.quote_buy(id, range, yes, U128(250 * ONE));
        assert_eq!(q.amount.0, c.market(id).q.pool.as_ref().unwrap().buy_cost(range as usize, yes, 250 * ONE));
        assert_eq!(q.fee.0, mul_div_up(q.amount.0, WAD / 50, WAD));
        let unused = buy(&mut c, "alice.near", range, yes, 250 * ONE, 300 * ONE);
        assert_eq!(unused, 300 * ONE - q.total.0);
        let v = c.get_question(id);
        assert_eq!(v.pot.0, 1_000 * ONE + q.amount.0);
        assert_eq!(v.fees_owed, q.fee);
        assert_eq!(v.prices, q.prices_after);
    }
}

#[test]
fn the_sale_quote_is_what_a_sale_does() {
    for (range, yes) in [(1, true), (1, false)] {
        let (mut c, id) = setup();
        buy(&mut c, "alice.near", range, yes, 250 * ONE, 300 * ONE);
        let pot = c.get_question(id).pot.0;
        let q = c.quote_sell(id, range, yes, U128(100 * ONE));
        assert_eq!(q.total.0, q.amount.0 - q.fee.0);
        ctx("alice.near", 1, TRADING);
        assert_eq!(c.sell(id, range, yes, U128(100 * ONE), U128(0)), q.total);
        let v = c.get_question(id);
        assert_eq!(v.pot.0, pot - q.amount.0);
        assert_eq!(v.prices, q.prices_after);
    }
}

#[test]
fn a_quote_does_not_move_anything() {
    let (mut c, id) = setup();
    buy(&mut c, "alice.near", 1, true, 250 * ONE, 300 * ONE);
    let before = state_of(&c, id);
    c.quote_buy(id, 1, true, U128(ONE));
    c.quote_sell(id, 1, true, U128(ONE));
    assert_eq!(state_of(&c, id), before);
}

#[test]
#[should_panic(expected = "NotOpen")]
fn a_question_not_opened_gives_no_quote() {
    let (c, id) = created();
    c.quote_buy(id, 0, true, U128(ONE));
}

#[test]
#[should_panic(expected = "TradingClosed")]
fn no_buy_quote_after_the_close() {
    let (c, id) = setup();
    ctx("alice.near", 0, CLOSE);
    c.quote_buy(id, 0, true, U128(ONE));
}

#[test]
#[should_panic(expected = "TradingClosed")]
fn no_sale_quote_after_the_close() {
    let (mut c, id) = setup();
    buy(&mut c, "alice.near", 0, true, 100 * ONE, 100 * ONE);
    ctx("alice.near", 0, CLOSE);
    c.quote_sell(id, 0, true, U128(ONE));
}

#[test]
#[should_panic(expected = "BadRange")]
fn no_quote_for_an_answer_that_does_not_exist() {
    let (c, id) = setup();
    c.quote_buy(id, 5, true, U128(ONE));
}

#[test]
#[should_panic(expected = "no such question")]
fn a_question_that_does_not_exist_cannot_be_read() {
    let (c, _) = setup();
    c.get_question(9);
}

#[test]
fn holdings_and_balances_read_the_same_slots() {
    let (mut c, id) = setup();
    buy(&mut c, "alice.near", 4, false, 40 * ONE, 40 * ONE);
    buy(&mut c, "alice.near", 0, true, 30 * ONE, 30 * ONE);
    let h = c.holdings(id, acc("alice.near"));
    assert_eq!(h.yes.iter().map(|x| x.0).collect::<Vec<_>>(), [30 * ONE, 0, 0, 0, 0]);
    assert_eq!(h.no.iter().map(|x| x.0).collect::<Vec<_>>(), [0, 0, 0, 0, 40 * ONE]);
    for i in 0..5 {
        assert_eq!(c.mt_balance_of(acc("alice.near"), format!("1:yes:{i}")), h.yes[i]);
        assert_eq!(c.mt_balance_of(acc("alice.near"), format!("1:no:{i}")), h.no[i]);
    }
    let nobody = c.holdings(id, acc("nobody.near"));
    assert_eq!((nobody.yes.len(), nobody.no.len()), (5, 5));
    assert_eq!(c.mt_balance_of(acc("nobody.near"), "1:no:4".into()).0, 0);
}

#[test]
fn the_interval_is_the_price_when_the_payouts_are_all_one() {
    // the fund is above L, so the pot always covers every winning token
    let (mut c, id) = funded(8_000 * ONE);
    buy(&mut c, "alice.near", 3, true, 500 * ONE, 500 * ONE);
    let v = c.get_question(id);
    assert!(v.payouts.iter().all(|r| r.0 == WAD));
    for i in 0..5 {
        assert_eq!(v.intervals[i], [v.prices[i], v.prices[i]]);
    }
}

/// With a fund of exactly L the curve can never owe more than the pot, however far a trade pushes it.
#[test]
fn a_fund_of_exactly_l_pays_one_on_every_answer() {
    let (c, _) = setup();
    let l = c.get_question(1).virtual_liquidity.0;
    let (mut c, id) = funded(l);
    assert_eq!(c.get_question(id).fund.0, l);
    buy(&mut c, "alice.near", 4, true, 20_000 * ONE, 30_000 * ONE);
    buy(&mut c, "bob.near", 0, false, 20_000 * ONE, 30_000 * ONE);
    let v = c.get_question(id);
    assert!(v.yes_supply[4].0 + v.no_supply[0].0 > 2 * l, "the trade has gone far past the fund");
    assert_eq!(v.payouts, vec![U128(WAD); 5]);
    for i in 0..5 {
        assert_eq!(v.intervals[i], [v.prices[i], v.prices[i]]);
    }
    assert!(c.market(id).q.pot >= c.market(id).q.winning_supply(4));
}

#[test]
fn the_interval_opens_on_a_short_pot_as_the_spec_says() {
    let (mut c, id) = funded(ONE);
    buy(&mut c, "alice.near", 1, true, 900 * ONE, 900 * ONE);
    buy(&mut c, "bob.near", 3, false, 300 * ONE, 300 * ONE);
    let v = c.get_question(id);
    assert_eq!(c.information_intervals(id), v.intervals);
    for i in 0..5 {
        let p = v.prices[i].0;
        let r_no = (0..5).filter(|&j| j != i).map(|j| v.payouts[j].0).min().unwrap();
        let upper = mul_div_up(p, WAD, v.payouts[i].0).min(WAD);
        let lower = WAD.saturating_sub(mul_div_up(WAD - p, WAD, r_no));
        assert_eq!(v.intervals[i], [U128(lower), U128(upper)]);
        assert!(v.intervals[i][0].0 <= p && p <= v.intervals[i][1].0);
    }
    assert!(v.intervals[1][1].0 > v.prices[1].0, "the wide answer's upper end is above its price");
    assert!(v.intervals[0][0].0 < v.prices[0].0, "and a NO that may be paid short pulls the lower end down");
}

#[test]
fn the_lower_end_of_the_interval_is_rounded_down() {
    let (mut c, id) = funded(ONE);
    buy(&mut c, "alice.near", 0, true, 2_500 * ONE, 3_000 * ONE);
    buy(&mut c, "bob.near", 1, true, 2_600 * ONE, 3_000 * ONE);
    let v = c.get_question(id);
    let r_no = v.payouts[1].0;
    assert!(r_no < WAD && r_no < v.payouts[2].0, "a NO on 0 is paid least if 1 happens");
    let lower = WAD - mul_div_up(WAD - v.prices[0].0, WAD, r_no);
    assert!(lower > 0);
    assert_eq!(v.intervals[0][0].0, lower);
}

#[test]
fn the_upper_end_of_the_interval_stops_at_one() {
    let (mut c, id) = funded(ONE);
    buy(&mut c, "alice.near", 4, true, 10_000 * ONE, 11_000 * ONE);
    let v = c.get_question(id);
    assert!(v.prices[4].0 > v.payouts[4].0, "the price alone divided by the payout would be above 1");
    assert_eq!(v.intervals[4][1].0, WAD);
}

#[test]
fn counts_and_scale() {
    let (mut c, _) = setup();
    assert_eq!(c.question_count(), 1);
    assert_eq!(c.wad().0, WAD);
    question(&mut c, "operator.near", "usdc.near", 10u128.pow(23));
    assert_eq!(c.question_count(), 2);
}

#[test]
fn two_questions_get_two_ids_and_keep_their_own_pots() {
    let (mut c, id) = setup();
    let other = question(&mut c, "operator.near", "usdc.near", 10u128.pow(23));
    assert_ne!(id, other);
    open_at(&mut c, other, "operator.near", "usdc.near");
    buy_at(&mut c, other, "usdc.near", "alice.near");
    assert_eq!(c.get_question(id).pot.0, 1_000 * ONE);
    assert_eq!(c.holdings(id, acc("alice.near")).yes[0].0, 0);
    assert_eq!(c.holdings(other, acc("alice.near")).yes[0].0, 1000);
}

// ------------------------------------------------------------------------------------------------ creating

#[test]
#[should_panic(expected = "empty answer label")]
fn an_answer_needs_a_label() {
    let mut args = five_answers();
    args.answers[2] = String::new();
    create(args);
}

#[test]
#[should_panic(expected = "two answers share a label")]
fn two_answers_cannot_share_a_label() {
    let mut args = five_answers();
    args.answers[4] = args.answers[1].clone();
    create(args);
}

#[test]
#[should_panic(expected = "title too long")]
fn a_title_has_a_limit() {
    let mut args = five_answers();
    args.title = "x".repeat(281);
    create(args);
}

#[test]
#[should_panic(expected = "label too long")]
fn an_answer_label_has_a_limit() {
    let mut args = five_answers();
    args.answers[1] = "x".repeat(65);
    create(args);
}

#[test]
fn a_title_and_labels_of_the_longest_length_are_taken() {
    let mut args = five_answers();
    args.title = "x".repeat(280);
    args.answers[1] = "x".repeat(64);
    let (c, id) = create(args);
    assert_eq!(c.get_question(id).title.len(), 280);
}

#[test]
fn a_fee_of_a_tenth_is_taken() {
    let mut args = five_answers();
    args.fee_rate = U128(WAD / 10);
    let (c, id) = create(args);
    assert_eq!(c.get_question(id).fee_rate.0, WAD / 10);
}

#[test]
#[should_panic(expected = "BadRange")]
fn a_question_needs_three_answers() {
    let mut args = five_answers();
    args.answers.truncate(2);
    create(args);
}

#[test]
#[should_panic(expected = "BadRange")]
fn a_question_takes_at_most_sixteen_answers() {
    let mut args = five_answers();
    args.answers = (0..17).map(|i| format!("a{i}")).collect();
    create(args);
}

#[test]
#[should_panic(expected = "BadConfig")]
fn the_fee_is_at_most_a_tenth() {
    let mut args = five_answers();
    args.fee_rate = U128(WAD / 10 + 1);
    create(args);
}

#[test]
#[should_panic(expected = "BadConfig")]
fn the_close_must_be_in_the_future() {
    let mut args = five_answers();
    args.close_time = U64(T0);
    create(args);
}

#[test]
#[should_panic(expected = "BadConfig")]
fn the_result_cannot_come_before_the_close() {
    let mut args = five_answers();
    args.resolve_after = U64(CLOSE - 1);
    create(args);
}

#[test]
#[should_panic(expected = "BadConfig")]
fn the_fallback_cannot_come_before_the_resolver() {
    let mut args = five_answers();
    args.fallback_after = U64(RESOLVE_AFTER - 1);
    create(args);
}

// ------------------------------------------------------------------------------------------------ events

#[test]
fn every_call_logs_what_it_did() {
    let (mut c, id) = created();
    assert_eq!(logged("create"), json!({"id": 1, "operator": "operator.near", "collateral": "usdc.near", "title": "Highest temperature in NYC on December 1"}));
    ctx("usdc.near", 0, T0 + 1);
    let _ = c.ft_on_transfer(acc("operator.near"), U128(1_000 * ONE), OPEN.into());
    assert_eq!(
        logged("open"),
        json!({"id": 1, "depth": "2000000000", "fund": "1000000000", "prices": ["380000000000000000", "290000000000000000", "190000000000000000", "110000000000000000", "30000000000000000"]})
    );
    let q = c.quote_buy(id, 2, false, U128(30 * ONE));
    ctx("usdc.near", 0, T0 + 2);
    let msg = r#"{"action":"buy","question_id":1,"range":2,"yes":false,"tickets":"30000000","max_cost":"30000000"}"#;
    let _ = c.ft_on_transfer(acc("bob.near"), U128(30 * ONE), msg.into());
    assert_eq!(
        logged("buy"),
        json!({"id": 1, "buyer": "bob.near", "range": 2, "yes": false, "tickets": "30000000", "cost": q.amount, "fee": q.fee})
    );
    let q = c.quote_sell(id, 2, false, U128(ONE));
    ctx("bob.near", 1, TRADING);
    c.sell(id, 2, false, U128(ONE), U128(0));
    assert_eq!(
        logged("sell"),
        json!({"id": 1, "seller": "bob.near", "range": 2, "yes": false, "tickets": "1000000", "refund": q.amount, "fee": q.fee})
    );
    ctx("bob.near", ROW_DEPOSIT, TRADING);
    c.mt_transfer(acc("dave.near"), "1:no:2".into(), U128(ONE), Some("for dave".into()));
    assert_eq!(logged("transfer"), json!({"from": "bob.near", "to": "dave.near", "token_id": "1:no:2", "amount": "1000000", "memo": "for dave"}));
    ctx("operator.near", 1, TRADING);
    let f = c.claim_fees(id);
    assert_eq!(logged("claim_fees"), json!({"id": 1, "to": "operator.near", "amount": f}));
    ctx("oracle.near", 1, TRADING);
    c.halt(id);
    assert_eq!(logged("halt"), json!({"id": 1, "by": "oracle.near"}));
    ctx("oracle.near", 1, LATE);
    let r = c.resolve(id, 3);
    let surplus = c.get_question(id).surplus;
    assert_eq!(logged("resolve"), json!({"id": 1, "by": "oracle.near", "winner": 3, "payout": r, "surplus": surplus}));
    ctx("bob.near", 1, LATE);
    let a = c.redeem(id, None);
    assert_eq!(logged("redeem"), json!({"id": 1, "holder": "bob.near", "tickets": (28 * ONE).to_string(), "amount": a}));
    ctx("operator.near", 1, LATE);
    let s = c.claim_surplus(id);
    assert_eq!(logged("claim_surplus"), json!({"id": 1, "to": "operator.near", "amount": s}));
}

#[test]
fn a_failed_payment_says_so_in_the_log() {
    let (mut c, id) = setup();
    bounced(&mut c, id, "alice.near", 5);
    assert_eq!(logged("payment_failed"), json!({"id": 1, "to": "alice.near", "amount": "5", "pending": "5"}));
    bounced(&mut c, id, "alice.near", 7);
    assert_eq!(logged("payment_failed"), json!({"id": 1, "to": "alice.near", "amount": "7", "pending": "12"}));
    delivered(&mut c, id, "alice.near", 7);
    assert!(get_logs().is_empty(), "a good transfer logs nothing");
}

#[test]
fn withdrawing_pending_money_says_so_in_the_log() {
    let (mut c, id) = setup();
    bounced(&mut c, id, "alice.near", 5);
    ctx("bob.near", 1, TRADING);
    c.withdraw_pending(id, Some(acc("alice.near")));
    assert_eq!(logged("withdraw_pending"), json!({"id": 1, "to": "alice.near", "amount": "5"}));
}

// ------------------------------------------------------------------------------------------------ whole lives

/// On chain every call reads the contract from storage. This does the same between all the calls of one question.
#[test]
fn a_whole_life_with_the_state_read_back_between_every_call() {
    let (c, id) = setup();
    let mut c = reload(c);
    buy(&mut c, "alice.near", 1, true, 700 * ONE, 700 * ONE);
    let mut c = reload(c);
    buy(&mut c, "bob.near", 1, false, 400 * ONE, 400 * ONE);
    let mut c = reload(c);
    let v = c.get_question(id);
    assert!(v.opened && v.fees_owed.0 > 0);
    assert_eq!((v.yes_supply[1].0, v.no_supply[1].0), (700 * ONE, 400 * ONE));
    ctx("alice.near", ROW_DEPOSIT, TRADING);
    c.mt_transfer(acc("carol.near"), "1:yes:1".into(), U128(100 * ONE), None);
    let mut c = reload(c);
    assert_eq!(c.holdings(id, acc("carol.near")).yes[1].0, 100 * ONE);
    ctx("alice.near", 1, TRADING);
    let got = c.sell(id, 1, true, U128(100 * ONE), U128(0)).0;
    let mut c = reload(c);
    bounced(&mut c, id, "alice.near", got);
    let mut c = reload(c);
    assert_eq!(c.pending_of(id, acc("alice.near")).0, got);
    ctx("alice.near", 1, TRADING);
    c.withdraw_pending(id, None);
    let mut c = reload(c);
    assert_eq!(c.pending_of(id, acc("alice.near")).0, 0);
    delivered(&mut c, id, "alice.near", got);
    let mut c = reload(c);
    ctx("oracle.near", 1, LATE);
    let r = c.resolve(id, 1).0;
    let mut c = reload(c);
    assert!(c.get_question(id).resolved);
    ctx("carol.near", 1, LATE);
    assert_eq!(c.redeem(id, None).0, mul_div(100 * ONE, r, WAD));
    let mut c = reload(c);
    ctx("operator.near", 1, LATE);
    assert!(c.claim_fees(id).0 > 0);
    let mut c = reload(c);
    ctx("operator.near", 1, LATE);
    c.claim_surplus(id);
    let c = reload(c);
    assert_eq!(c.question_count(), 1);
}

/// Hundreds of small trades by four holders on a short pot, one payment in five failing, against a plain ledger: every
/// balance, every supply and every unit of money stays as the ledger says, and the storage balance ends to the yoctoNEAR.
#[test]
fn a_long_run_of_small_trades_matches_a_plain_ledger() {
    let names = ["alice.near", "bob.near", "carol.near", "dave.near"];
    let (mut c, id) = funded(ONE);
    let start = balance(&c, id);
    let mut seed = 88_172_645_463_325_252u64;
    let mut next = |m: u64| {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed % m
    };
    let mut held = [[0u128; 10]; 4];
    let mut stuck = [0u128; 4];
    let (mut money_in, mut money_out, mut pot, mut fees) = (ONE, 0u128, ONE, 0u128);
    let (mut sales, mut capped, mut failed, mut by_callers) = (0, 0, 0, 0u128);
    for _ in 0..600 {
        let who = next(4) as usize;
        // mostly the cheap answers: their tokens cost little, so the pot runs short of them and the cap binds
        let (range, yes) = (next(5).max(next(5)).max(next(5)) as usize, next(4) != 0);
        let slot = if yes { range } else { 5 + range };
        let tickets = 1_000 + next(40 * ONE as u64) as u128;
        let have = held[who][slot];
        let n = tickets.min(have);
        ctx(names[who], 1, TRADING);
        match next(3) {
            0 => {
                let q = c.quote_buy(id, range as u8, yes, U128(tickets));
                assert_eq!(buy(&mut c, names[who], range as u8, yes, tickets, q.total.0 + 7), 7);
                held[who][slot] += tickets;
                (money_in, pot, fees) = (money_in + q.total.0, pot + q.amount.0, fees + q.fee.0);
            }
            1 if n > 0 => {
                let q = c.quote_sell(id, range as u8, yes, U128(n));
                capped += (q.amount.0 < curve_refund(&c, id, range, yes, n)) as u32;
                let payouts = c.get_question(id).payouts;
                ctx(names[who], 1, TRADING);
                let got = c.sell(id, range as u8, yes, U128(n), q.total).0;
                assert_eq!(got, q.total.0);
                let after = c.get_question(id).payouts;
                for j in (0..5).filter(|&j| (j == range) == yes) {
                    assert!(after[j].0 >= payouts[j].0, "no sale lowers the payout on an answer the token pays on");
                }
                held[who][slot] -= n;
                (pot, fees) = (pot - q.amount.0, fees + q.fee.0);
                sales += 1;
                if got > 0 {
                    assert_paid(id, names[who], got);
                    if next(5) == 0 {
                        bounced(&mut c, id, names[who], got);
                        stuck[who] += got;
                        failed += 1;
                    } else {
                        delivered(&mut c, id, names[who], got);
                        money_out += got;
                    }
                }
            }
            2 if n > 0 => {
                let to = (who + 1 + next(3) as usize) % 4;
                let side = if yes { "yes" } else { "no" };
                ctx(names[who], ROW_DEPOSIT, TRADING);
                c.mt_transfer(acc(names[to]), format!("{id}:{side}:{range}"), U128(n), None);
                let cost = ROW_DEPOSIT - near_sent().iter().map(|x| x.1).sum::<u128>();
                by_callers += if cost > 1 { cost } else { 0 };
                held[who][slot] -= n;
                held[to][slot] += n;
            }
            _ => continue,
        }
        let v = c.get_question(id);
        assert_eq!((v.pot.0, v.fees_owed.0), (pot, fees));
        for s in 0..5 {
            assert_eq!(v.yes_supply[s].0, held.iter().map(|h| h[s]).sum::<u128>());
            assert_eq!(v.no_supply[s].0, held.iter().map(|h| h[5 + s]).sum::<u128>());
        }
        for (i, name) in names.iter().enumerate() {
            let h = c.holdings(id, acc(name));
            assert!(h.yes.iter().chain(h.no.iter()).zip(held[i].iter()).all(|(a, b)| a.0 == *b));
            assert_eq!(c.pending_of(id, acc(name)).0, stuck[i]);
        }
    }
    assert!(sales >= 150 && capped >= 30 && failed >= 40, "{sales} sales, {capped} capped, {failed} failed");

    // the answer with the most tokens on it, the one the pot is shortest for
    let winner = (0..5).max_by_key(|&w| c.market(id).q.winning_supply(w)).unwrap();
    ctx("oracle.near", 1, LATE);
    let r = c.resolve(id, winner as u8).0;
    assert!(r < WAD);
    for (i, name) in names.iter().enumerate() {
        let wins: u128 = (0..5).map(|s| if s == winner { held[i][s] } else { held[i][5 + s] }).sum();
        ctx(name, 1, LATE);
        let a = c.redeem(id, None).0;
        assert_eq!(a, mul_div(wins, r, WAD));
        assert_paid(id, name, a);
        delivered(&mut c, id, name, a);
        money_out += a;
        ctx(name, 1, LATE);
        if stuck[i] > 0 {
            c.withdraw_pending(id, None);
            assert_paid(id, name, stuck[i]);
            delivered(&mut c, id, name, stuck[i]);
            money_out += stuck[i];
        }
    }
    ctx("operator.near", 1, LATE);
    let f = c.claim_fees(id).0;
    assert_paid(id, "operator.near", f);
    delivered(&mut c, id, "operator.near", f);
    money_out += f;
    let owed = c.get_question(id).surplus.0;
    ctx("operator.near", 1, LATE);
    assert_eq!(c.claim_surplus(id).0, owed);
    money_out += owed;
    if owed > 0 {
        delivered(&mut c, id, "operator.near", owed);
    }
    let left = c.get_question(id).pot.0;
    assert_eq!(money_in, money_out + left);
    assert!(left <= 4);
    assert_eq!(balance(&c, id), start + by_callers, "every row is gone, and those the callers paid for stay with the question");
}

// ------------------------------------------------------------------------------------------------ gaps mutants found

/// Runs a call that must fail and returns its message and what it logged. A failed receipt keeps its logs on chain, and
/// `refused` starts a new mock, which has none, so a test of what a refused call logged has to read them here.
fn logs_of_a_refusal(mut c: Contract, call: impl FnOnce(&mut Contract)) -> (String, Vec<String>) {
    let r = catch_unwind(AssertUnwindSafe(|| call(&mut c)));
    let logs = get_logs();
    std::mem::forget(c);
    (*r.expect_err("the call went through").downcast::<String>().unwrap(), logs)
}

fn creation_refused(args: CreateArgs) -> String {
    mock_with(HashMap::new());
    ctx("operator.near", 0, T0);
    let mut c = Contract::new();
    ctx("operator.near", 10u128.pow(23), T0);
    let r = catch_unwind(AssertUnwindSafe(|| c.create_question(args)));
    std::mem::forget(c);
    *r.expect_err("the question was created").downcast::<String>().unwrap()
}

#[test]
fn a_call_that_cannot_hold_its_record_logs_nothing() {
    let (mut c, id) = setup();
    buy(&mut c, "alice.near", 0, true, 100 * ONE, 100 * ONE);
    c.market_mut(id).storage = 0;
    let (msg, logs) = logs_of_a_refusal(c, |c| {
        ctx("alice.near", 1, TRADING);
        c.sell(id, 0, true, U128(ONE), U128(0));
    });
    assert!(msg.contains("storage balance is"), "{msg}");
    assert!(logs.is_empty(), "{logs:?}");

    let (mut c, id) = setup();
    buy(&mut c, "alice.near", 0, true, 100 * ONE, 100 * ONE);
    c.market_mut(id).storage = 0;
    let (msg, logs) = logs_of_a_refusal(c, |c| {
        ctx("operator.near", 1, TRADING);
        c.claim_fees(id);
    });
    assert!(msg.contains("storage balance is"), "{msg}");
    assert!(logs.is_empty(), "{logs:?}");

    let (mut c, id) = setup();
    ctx("oracle.near", 1, LATE);
    c.resolve(id, 0);
    c.market_mut(id).storage = 0;
    let (msg, logs) = logs_of_a_refusal(c, |c| {
        ctx("operator.near", 1, LATE);
        c.claim_surplus(id);
    });
    assert!(msg.contains("storage balance is"), "{msg}");
    assert!(logs.is_empty(), "{logs:?}");
}

#[test]
fn a_purchase_refused_for_storage_logs_nothing() {
    let (mut c, id) = setup();
    c.market_mut(id).storage = 0;
    let (msg, logs) = logs_of_a_refusal(c, |c| {
        buy(c, "alice.near", 0, true, 100 * ONE, 100 * ONE);
    });
    assert!(msg.contains("storage balance is"), "{msg}");
    assert!(logs.is_empty(), "{logs:?}");
}

#[test]
fn an_opening_refused_for_storage_logs_nothing() {
    let (mut c, id) = created();
    c.market_mut(id).storage = 0;
    let (msg, logs) = logs_of_a_refusal(c, |c| {
        ctx("usdc.near", 0, T0 + 1);
        let _ = c.ft_on_transfer(acc("operator.near"), U128(1_000 * ONE), OPEN.into());
    });
    assert!(msg.contains("storage balance is"), "{msg}");
    assert!(logs.is_empty(), "{logs:?}");
}

#[test]
fn a_transfer_refused_for_its_row_logs_nothing() {
    let (mut c, _) = setup();
    buy(&mut c, "alice.near", 0, true, 100 * ONE, 100 * ONE);
    let (msg, logs) = logs_of_a_refusal(c, |c| {
        ctx("alice.near", row("bob.near", 5) - 1, TRADING);
        c.mt_transfer(acc("bob.near"), "1:yes:0".into(), U128(ONE), None);
    });
    assert!(msg.contains("for the receiver's row"), "{msg}");
    assert!(logs.is_empty(), "{logs:?}");
}

#[test]
fn no_answer_can_be_empty_wherever_it_stands() {
    for at in 0..5 {
        let mut args = five_answers();
        args.answers[at] = String::new();
        let msg = creation_refused(args);
        assert!(msg.contains("empty answer label"), "at {at}: {msg}");
    }
}

#[test]
fn no_two_answers_can_share_a_label_wherever_they_stand() {
    for a in 0..5 {
        for b in a + 1..5 {
            let mut args = five_answers();
            args.answers[b] = args.answers[a].clone();
            let msg = creation_refused(args);
            assert!(msg.contains("two answers share a label"), "{a} and {b}: {msg}");
        }
    }
}

/// The rule is exact: a label is refused only if it is empty, or the same bytes as another.
#[test]
fn labels_that_differ_by_case_or_are_blank_are_taken() {
    let mut args = five_answers();
    args.answers = vec!["yes".into(), "Yes".into(), "YES".into(), " ".into(), "  ".into()];
    let (c, id) = create(args);
    assert_eq!(c.get_question(id).answers[3], " ");
}

#[test]
fn one_yocto_of_change_comes_back() {
    let (mut c, _) = setup();
    buy(&mut c, "alice.near", 0, true, 100 * ONE, 100 * ONE);
    buy(&mut c, "bob.near", 0, true, 100 * ONE, 100 * ONE);
    ctx("alice.near", 2, TRADING);
    c.mt_transfer(acc("bob.near"), "1:yes:0".into(), U128(ONE), None);
    assert_eq!(near_sent(), [("alice.near".to_string(), 1)]);
    ctx("alice.near", row("carol.near", 5) + 1, TRADING);
    c.mt_transfer(acc("carol.near"), "1:yes:0".into(), U128(ONE), None);
    assert_eq!(near_sent(), [("alice.near".to_string(), 1)]);
}

#[test]
fn the_change_of_a_transfer_goes_to_the_caller_not_whoever_signed() {
    let (mut c, _) = setup();
    buy(&mut c, "dao.near", 0, true, 100 * ONE, 100 * ONE);
    buy(&mut c, "bob.near", 0, true, 100 * ONE, 100 * ONE);
    ctx_via("dao.near", "user.near", 5, TRADING);
    c.mt_transfer(acc("bob.near"), "1:yes:0".into(), U128(ONE), None);
    assert_eq!(near_sent(), [("dao.near".to_string(), 4)]);
}

#[test]
fn a_balance_of_one_yocto_is_sent_too() {
    let (mut c, id) = setup();
    ctx("oracle.near", 1, LATE);
    c.resolve(id, 0);
    c.market_mut(id).storage = 1;
    ctx("operator.near", 1, LATE);
    assert_eq!(c.withdraw_storage(id).0, 1);
    assert_eq!(near_sent(), [("operator.near".to_string(), 1)]);
}

#[test]
fn the_storage_goes_to_the_operator_not_whoever_signed() {
    ctx("dao.near", 0, T0);
    let mut c = Contract::new();
    ctx_via("dao.near", "user.near", 10u128.pow(23), T0);
    let id = c.create_question(five_answers());
    let left = balance(&c, id);
    ctx_via("dao.near", "user.near", 1, CLOSE);
    assert_eq!(c.withdraw_storage(id).0, left);
    assert_eq!(near_sent(), [("dao.near".to_string(), left)]);
}

/// A halt says nothing about the result, and a redemption still has to come.
#[test]
#[should_panic(expected = "NotResolved")]
fn the_storage_of_a_halted_question_with_no_result_stays() {
    let (mut c, id) = setup();
    ctx("oracle.near", 1, TRADING);
    c.halt(id);
    ctx("operator.near", 1, LATE);
    c.withdraw_storage(id);
}

/// A message with a field the contract does not know is refused, and the token sends the money back, so a client that
/// believes it named a receiver finds out.
#[test]
fn a_purchase_names_no_receiver_under_any_name() {
    for name in ["receiver", "receiver_id", "to", "account_id", "recipient", "beneficiary", "buyer", "owner_id", "for"] {
        let (mut c, id) = setup();
        ctx("usdc.near", 0, T0 + 2);
        let msg = format!(r#"{{"action":"buy","question_id":1,"range":0,"yes":false,"tickets":"1000000","max_cost":"1000000","{name}":"bob.near"}}"#);
        let (c, why) = refused(c, |c| {
            let _ = c.ft_on_transfer(acc("alice.near"), U128(ONE), msg);
        });
        assert!(why.contains("bad message"), "{name}: {why}");
        assert_eq!(c.holdings(id, acc("bob.near")).no[0].0, 0, "{name}");
    }
}

/// One unit on a name nobody controls was what used to strand the balance, so the smallest row has to go too.
#[test]
fn one_losing_unit_on_a_name_nobody_controls_can_be_settled() {
    for (range, yes, token) in [(2u8, true, "1:yes:2"), (1, false, "1:no:1")] {
        let (mut c, id) = setup();
        buy(&mut c, "alice.near", range, yes, 100 * ONE, 100 * ONE);
        ctx("alice.near", ROW_DEPOSIT, TRADING);
        c.mt_transfer(acc("dead.near"), token.into(), U128(1), None);
        ctx("oracle.near", 1, LATE);
        c.resolve(id, 1);
        let start = balance(&c, id);
        ctx("anyone.near", 1, LATE);
        assert_eq!(c.redeem(id, Some(acc("dead.near"))).0, 0);
        assert_nothing_sent();
        assert_eq!(logged("redeem"), json!({"id": 1, "holder": "dead.near", "tickets": "0", "amount": "0"}));
        assert!(!c.balances.contains_key(&(id, acc("dead.near"))));
        assert_eq!(balance(&c, id), start + row("dead.near", 5));
    }
}

/// On a short pot the tokens and the money differ, and the event has to keep them apart.
#[test]
fn a_redemption_on_a_short_pot_logs_its_tokens_and_its_money() {
    let (mut c, id) = funded(ONE);
    buy(&mut c, "alice.near", 1, true, 900 * ONE, 900 * ONE);
    ctx("oracle.near", 1, LATE);
    let r = c.resolve(id, 1).0;
    assert!(r < WAD);
    ctx("alice.near", 1, LATE);
    let a = c.redeem(id, None);
    assert_ne!(a.0, 900 * ONE);
    assert_eq!(logged("redeem"), json!({"id": 1, "holder": "alice.near", "tickets": (900 * ONE).to_string(), "amount": a}));
}

#[test]
fn a_pending_amount_of_one_unit_can_be_withdrawn() {
    let (mut c, id) = setup();
    bounced(&mut c, id, "alice.near", 1);
    ctx("alice.near", 1, TRADING);
    c.withdraw_pending(id, None);
    assert_paid(id, "alice.near", 1);
    assert_eq!(c.pending_of(id, acc("alice.near")).0, 0);
}

/// `with_static_gas` is what a call is sure of, and on_paid burns about 2 Tgas on a failed payment (sandbox claim 4). With
/// less than that a failed payment could not be recorded. The other tests read these constants, so they cannot see a change.
#[test]
fn a_payment_and_its_callback_each_keep_ten_tgas() {
    assert_eq!((GAS_FT_TRANSFER.as_tgas(), GAS_CALLBACK.as_tgas()), (10, 10));
}
