//! AIMM on NEAR: questions with several answers, exactly one of which happens, priced by the Automated Information
//! Market Maker (whitepaper: `whitepaper/aimm.pdf`; exact rules: `SPEC.md`).
//!
//! One contract holds every question. Each question has its own NEP-141 collateral, its own operator, resolver and
//! fallback resolver, and its own pot. The arithmetic is `aimm-core`; this file adds accounts, money and time.
//!
//! Money comes in only through `ft_on_transfer` (the operator's fund, and every purchase); it goes out only through
//! `ft_transfer`, after the state has changed, and only to the account that earned it. If a transfer out fails, the
//! amount is kept for that account under `pending` and can be withdrawn later, so no result ever depends on a transfer.
//!
//! Each question pays for its own storage from one NEAR balance, `storage`, which the operator fills at creation and
//! anyone can top up: bytes freed are credited to it and bytes added are charged to it, so no question can spend what
//! another put in. The one exception is a row `mt_transfer` adds, which its caller pays for whole, so nobody can spend a
//! question's balance on holder rows for names of their choosing: those it pays for belong to accounts that bought.
//! `ft_on_transfer` trusts nothing a token says beyond its own question, because a question's collateral can be any
//! contract. A payment out holds the cost of a `pending` row until its outcome is known, so a failed transfer can always
//! be recorded.
//!
//! Amounts of tokens and money are in the collateral's smallest unit: a winning token is paid up to one unit.
//! Prices, payouts, the fee rate and the prediction range are in WAD (1e18 = 1).

use aimm_core::question::Question;
use aimm_core::{AimmError, WAD};
use near_sdk::json_types::{U128, U64};
use near_sdk::store::LookupMap;
use near_sdk::{
    assert_one_yocto, env, ext_contract, near, require, AccountId, Gas, NearToken, PanicOnDefault, Promise, PromiseError,
    PromiseOrValue,
};

const GAS_FT_TRANSFER: Gas = Gas::from_tgas(10);
const GAS_CALLBACK: Gas = Gas::from_tgas(10);
const MAX_TITLE: usize = 280;
const MAX_ANSWER: usize = 64;

#[ext_contract(ext_ft)]
pub trait FungibleToken {
    fn ft_transfer(&mut self, receiver_id: AccountId, amount: U128, memo: Option<String>);
}

/// One question: the AIMM arithmetic plus who may do what and in which token.
#[near(serializers = [borsh])]
pub struct Market {
    pub q: Question,
    pub collateral: AccountId,
    pub operator: AccountId,
    pub resolver: AccountId,
    pub fallback_resolver: AccountId,
    pub title: String,
    pub answers: Vec<String>,
    /// yoctoNEAR that pays for this question's storage.
    pub storage: u128,
}

#[near(contract_state)]
#[derive(PanicOnDefault)]
pub struct Contract {
    questions: LookupMap<u64, Market>,
    /// (question, holder) -> YES balances on every answer, then NO balances on every answer.
    balances: LookupMap<(u64, AccountId), Vec<u128>>,
    /// (question, account) -> money whose transfer out failed, to be withdrawn.
    pending: LookupMap<(u64, AccountId), u128>,
    count: u64,
}

#[near(serializers = [json])]
pub struct CreateArgs {
    pub collateral: AccountId,
    pub title: String,
    /// The answer labels, 3 to 16, exactly one of which will happen.
    pub answers: Vec<String>,
    pub resolver: AccountId,
    pub fallback_resolver: AccountId,
    /// Nanoseconds since the Unix epoch, as NEAR block times are.
    pub close_time: U64,
    pub resolve_after: U64,
    pub fallback_after: U64,
    /// WAD, at most 0.1e18.
    pub fee_rate: U128,
}

/// What `ft_on_transfer` is asked to do with the money it receives.
#[near(serializers = [json])]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum TransferMsg {
    /// The operator opens the question: depth b and opening prices (WAD, adding up to exactly 1e18). The money sent is the fund.
    Open { question_id: u64, depth: U128, prices: Vec<U128> },
    /// Buy `tickets` YES (or NO) on `range`, paying at most `max_cost` in all. What is not needed is sent back.
    Buy { question_id: u64, range: u8, yes: bool, tickets: U128, max_cost: U128 },
}

#[near(serializers = [json])]
pub struct Quote {
    /// The curve's cost of a buy, or the capped refund of a sale.
    pub amount: U128,
    pub fee: U128,
    /// What the buyer pays in all (amount + fee), or what the seller receives (amount - fee).
    pub total: U128,
    pub prices_after: Vec<U128>,
}

#[near(serializers = [json])]
pub struct QuestionView {
    pub id: u64,
    pub title: String,
    pub answers: Vec<String>,
    pub collateral: AccountId,
    pub operator: AccountId,
    pub resolver: AccountId,
    pub fallback_resolver: AccountId,
    pub close_time: U64,
    pub resolve_after: U64,
    pub fallback_after: U64,
    pub fee_rate: U128,
    pub opened: bool,
    pub halted: bool,
    pub resolved: bool,
    pub winner: Option<u8>,
    pub depth: U128,
    pub virtual_liquidity: U128,
    pub fund: U128,
    pub pot: U128,
    pub fees_paid: U128,
    pub fees_owed: U128,
    pub surplus: U128,
    /// yoctoNEAR left to pay for this question's storage.
    pub storage: U128,
    /// Every answer's price (WAD); empty until it opens and once resolved.
    pub prices: Vec<U128>,
    /// Every answer's prediction range [lower, upper] (WAD); empty until it opens and once resolved.
    pub intervals: Vec<[U128; 2]>,
    /// What each winning token would be paid if that answer happened now (WAD); after the result, the payout.
    pub payouts: Vec<U128>,
    pub yes_supply: Vec<U128>,
    pub no_supply: Vec<U128>,
}

#[near(serializers = [json])]
pub struct Holdings {
    pub yes: Vec<U128>,
    pub no: Vec<U128>,
}

fn fail(e: AimmError) -> ! {
    env::panic_str(&format!("AIMM: {e}"))
}

fn event(name: &str, data: near_sdk::serde_json::Value) {
    let body = near_sdk::serde_json::json!({"standard": "aimm", "version": "1.0.0", "event": name, "data": [data]});
    env::log_str(&format!("EVENT_JSON:{body}"));
}

fn u(v: &[u128]) -> Vec<U128> {
    v.iter().map(|&x| U128(x)).collect()
}

fn storage_cost(bytes: u64) -> u128 {
    env::storage_byte_cost().as_yoctonear() * bytes as u128
}

/// What a `pending` row for `to` takes: the key (prefix, question, name), the amount, and NEAR's 40 bytes for a record.
fn pending_row_cost(to: &AccountId) -> u128 {
    storage_cost(1 + 8 + 4 + to.len() as u64 + 16 + 40)
}

/// Token ids for the multi-token views and transfers: "<question>:yes:<answer>" or "<question>:no:<answer>".
fn parse_token_id(token_id: &str) -> (u64, bool, usize) {
    let parts: Vec<&str> = token_id.split(':').collect();
    require!(parts.len() == 3, "AIMM: token id is <question>:yes|no:<answer>");
    let qid = parts[0].parse().unwrap_or_else(|_| env::panic_str("AIMM: bad question id"));
    let yes = match parts[1] {
        "yes" => true,
        "no" => false,
        _ => env::panic_str("AIMM: side is yes or no"),
    };
    let range = parts[2].parse().unwrap_or_else(|_| env::panic_str("AIMM: bad answer"));
    require!(token_id == format!("{qid}:{}:{range}", parts[1]), "AIMM: token id is not in its plain form");
    (qid, yes, range)
}

#[near]
impl Contract {
    #[init]
    pub fn new() -> Self {
        Self { questions: LookupMap::new(b"q"), balances: LookupMap::new(b"b"), pending: LookupMap::new(b"p"), count: 0 }
    }

    // ------------------------------------------------------------------------------------------------ questions

    /// Creates a question; the caller becomes its operator. The attached NEAR pays for the question's row, and the rest
    /// is its storage balance: about 0.002 NEAR for each holder with five answers, which comes back as rows are freed.
    /// Answer labels must differ from each other and not be empty.
    #[payable]
    pub fn create_question(&mut self, args: CreateArgs) -> u64 {
        let before = env::storage_usage();
        require!(args.title.len() <= MAX_TITLE, "AIMM: title too long");
        require!(args.answers.iter().all(|a| a.len() <= MAX_ANSWER), "AIMM: answer label too long");
        require!(args.answers.iter().all(|a| !a.is_empty()), "AIMM: empty answer label");
        require!((1..args.answers.len()).all(|i| !args.answers[..i].contains(&args.answers[i])), "AIMM: two answers share a label");
        let q = Question::new(
            args.answers.len(),
            args.fee_rate.0,
            args.close_time.0,
            args.resolve_after.0,
            args.fallback_after.0,
            env::block_timestamp(),
        )
        .unwrap_or_else(|e| fail(e));
        self.count += 1;
        let id = self.count;
        let operator = env::predecessor_account_id();
        self.questions.insert(
            id,
            Market {
                q,
                collateral: args.collateral.clone(),
                operator: operator.clone(),
                resolver: args.resolver.clone(),
                fallback_resolver: args.fallback_resolver,
                title: args.title.clone(),
                answers: args.answers,
                storage: 0,
            },
        );
        self.questions.flush();
        let cost = storage_cost(env::storage_usage() - before);
        let paid = env::attached_deposit().as_yoctonear();
        require!(paid >= cost, format!("AIMM: attach at least {cost} yoctoNEAR for storage"));
        self.market_mut(id).storage = paid - cost;
        event("create", near_sdk::serde_json::json!({"id": id, "operator": operator, "collateral": args.collateral, "title": args.title}));
        id
    }

    fn market(&self, id: u64) -> &Market {
        self.questions.get(&id).unwrap_or_else(|| env::panic_str("AIMM: no such question"))
    }

    fn market_mut(&mut self, id: u64) -> &mut Market {
        self.questions.get_mut(&id).unwrap_or_else(|| env::panic_str("AIMM: no such question"))
    }

    fn held(&self, id: u64, who: &AccountId, n: usize) -> Vec<u128> {
        self.balances.get(&(id, who.clone())).cloned().unwrap_or_else(|| vec![0; 2 * n])
    }

    fn set_held(&mut self, id: u64, who: &AccountId, v: Vec<u128>) {
        if v.iter().all(|&x| x == 0) {
            self.balances.remove(&(id, who.clone()));
        } else {
            self.balances.insert((id, who.clone()), v);
        }
    }

    /// Charges the question for the storage added since `before`, or gives it the cost of what was freed.
    fn settle_storage(&mut self, id: u64, before: u64) {
        self.questions.flush();
        self.balances.flush();
        self.pending.flush();
        let now = env::storage_usage();
        if now >= before {
            self.hold_storage(id, storage_cost(now - before));
        } else {
            self.market_mut(id).storage += storage_cost(before - now);
        }
    }

    fn hold_storage(&mut self, id: u64, amount: u128) {
        let m = self.market_mut(id);
        if m.storage < amount {
            env::panic_str(&format!("AIMM: this question's storage balance is {} yoctoNEAR short; anyone can call add_storage", amount - m.storage));
        }
        m.storage -= amount;
    }

    // ------------------------------------------------------------------------------------------------ money in

    /// NEP-141 receiver. The message says what to do with the money: `open` (the operator's fund) or `buy`.
    /// Returns what was not used, which the token contract sends back. Any failure returns everything.
    pub fn ft_on_transfer(&mut self, sender_id: AccountId, amount: U128, msg: String) -> PromiseOrValue<U128> {
        let token = env::predecessor_account_id();
        let msg: TransferMsg = near_sdk::serde_json::from_str(&msg).unwrap_or_else(|_| env::panic_str("AIMM: bad message"));
        let now = env::block_timestamp();
        match msg {
            TransferMsg::Open { question_id, depth, prices } => {
                let before = env::storage_usage();
                let m = self.market_mut(question_id);
                require!(m.collateral == token, "AIMM: wrong collateral");
                require!(m.operator == sender_id, "AIMM: only the operator opens");
                let prices: Vec<u128> = prices.iter().map(|p| p.0).collect();
                m.q.open(depth.0, amount.0, &prices, now).unwrap_or_else(|e| fail(e));
                self.settle_storage(question_id, before);
                event("open", near_sdk::serde_json::json!({"id": question_id, "depth": depth, "fund": amount, "prices": u(&prices)}));
                PromiseOrValue::Value(U128(0))
            }
            TransferMsg::Buy { question_id, range, yes, tickets, max_cost } => {
                let before = env::storage_usage();
                let m = self.market_mut(question_id);
                require!(m.collateral == token, "AIMM: wrong collateral");
                let fill = m.q.buy(range as usize, yes, tickets.0, now).unwrap_or_else(|e| fail(e));
                let total = fill.amount + fill.fee;
                require!(total <= max_cost.0, format!("AIMM: cost {total} above max_cost"));
                require!(total <= amount.0, format!("AIMM: cost {total} above the amount sent"));
                let n = m.q.n();
                let mut h = self.held(question_id, &sender_id, n);
                h[if yes { range as usize } else { n + range as usize }] += tickets.0;
                self.set_held(question_id, &sender_id, h);
                self.settle_storage(question_id, before);
                event(
                    "buy",
                    near_sdk::serde_json::json!({"id": question_id, "buyer": sender_id, "range": range, "yes": yes,
                        "tickets": tickets, "cost": U128(fill.amount), "fee": U128(fill.fee)}),
                );
                PromiseOrValue::Value(U128(amount.0 - total))
            }
        }
    }

    // ------------------------------------------------------------------------------------------------ money out

    /// Sends `amount` of the question's `token` to `to`, holding the cost of a `pending` row until the outcome is known.
    fn pay(&mut self, id: u64, token: AccountId, to: AccountId, amount: u128) {
        if amount == 0 {
            return;
        }
        self.hold_storage(id, pending_row_cost(&to));
        ext_ft::ext(token)
            .with_attached_deposit(NearToken::from_yoctonear(1))
            .with_static_gas(GAS_FT_TRANSFER)
            .ft_transfer(to.clone(), U128(amount), Some("AIMM".into()))
            .then(Self::ext(env::current_account_id()).with_static_gas(GAS_CALLBACK).on_paid(id, to, U128(amount)))
            .detach();
    }

    /// If a transfer out failed, keeps the money for the receiver to withdraw. Either way, gives the question back what
    /// `pay` held, less what the record took. Only failure counts: `ft_transfer` returns nothing, and if a token returns
    /// something anyway the payment went through, so the result is not read.
    #[private]
    pub fn on_paid(&mut self, question_id: u64, to: AccountId, amount: U128) {
        let before = env::storage_usage();
        let held = pending_row_cost(&to);
        if matches!(env::promise_result_checked(0, 0), Err(PromiseError::Failed)) {
            let key = (question_id, to);
            let owed = self.pending.get(&key).copied().unwrap_or(0) + amount.0;
            self.pending.insert(key.clone(), owed);
            self.pending.flush();
            event("payment_failed", near_sdk::serde_json::json!({"id": question_id, "to": key.1, "amount": amount, "pending": U128(owed)}));
        }
        let used = storage_cost(env::storage_usage() - before);
        self.market_mut(question_id).storage += held.saturating_sub(used);
    }

    /// Sends `account_id` (the caller by default) the money of this question whose transfer out failed earlier, for
    /// example because they were not registered with the token then. Anyone may call this. Attach exactly 1 yoctoNEAR.
    #[payable]
    pub fn withdraw_pending(&mut self, question_id: u64, account_id: Option<AccountId>) {
        assert_one_yocto();
        let who = account_id.unwrap_or_else(env::predecessor_account_id);
        let token = self.market(question_id).collateral.clone();
        let before = env::storage_usage();
        let amount = self.pending.remove(&(question_id, who.clone())).unwrap_or(0);
        require!(amount > 0, "AIMM: nothing pending");
        self.settle_storage(question_id, before);
        self.pay(question_id, token, who.clone(), amount);
        event("withdraw_pending", near_sdk::serde_json::json!({"id": question_id, "to": who, "amount": U128(amount)}));
    }

    // ------------------------------------------------------------------------------------------------ storage

    /// Adds the attached NEAR to the question's storage balance. Anyone may call this.
    #[payable]
    pub fn add_storage(&mut self, question_id: u64) -> U128 {
        let m = self.market_mut(question_id);
        m.storage += env::attached_deposit().as_yoctonear();
        U128(m.storage)
    }

    /// The operator takes the storage balance once nothing can need it: after the result, because a redemption frees the
    /// row that pays for it, or after the close of a question that never opened. Claim the fees and the surplus first:
    /// each holds a little of the balance. Rows freed later fill it again, so this can be called again. Attach exactly
    /// 1 yoctoNEAR.
    #[payable]
    pub fn withdraw_storage(&mut self, question_id: u64) -> U128 {
        assert_one_yocto();
        let caller = env::predecessor_account_id();
        let now = env::block_timestamp();
        let m = self.market_mut(question_id);
        require!(m.operator == caller, "AIMM: only the operator");
        require!(m.q.resolved || (m.q.pool.is_none() && now >= m.q.close_time), "AIMM: NotResolved");
        let amount = core::mem::take(&mut m.storage);
        if amount > 0 {
            Promise::new(caller).transfer(NearToken::from_yoctonear(amount)).detach();
        }
        U128(amount)
    }

    // ------------------------------------------------------------------------------------------------ trading

    /// Sells `tickets` of the caller's YES (or NO) on `range`, sending them the proceeds (refund less fee, at least
    /// `min_proceeds`). Attach exactly 1 yoctoNEAR. Returns the amount owed, not the amount delivered.
    #[payable]
    pub fn sell(&mut self, question_id: u64, range: u8, yes: bool, tickets: U128, min_proceeds: U128) -> U128 {
        assert_one_yocto();
        let seller = env::predecessor_account_id();
        let before = env::storage_usage();
        let n = self.market(question_id).q.n();
        require!((range as usize) < n, "AIMM: BadRange");
        let mut h = self.held(question_id, &seller, n);
        let slot = if yes { range as usize } else { n + range as usize };
        require!(h[slot] >= tickets.0, "AIMM: not enough tokens");
        let now = env::block_timestamp();
        let m = self.market_mut(question_id);
        let fill = m.q.sell(range as usize, yes, tickets.0, now).unwrap_or_else(|e| fail(e));
        let proceeds = fill.amount - fill.fee;
        require!(proceeds >= min_proceeds.0, format!("AIMM: proceeds {proceeds} below min_proceeds"));
        let token = m.collateral.clone();
        h[slot] -= tickets.0;
        self.set_held(question_id, &seller, h);
        self.settle_storage(question_id, before);
        self.pay(question_id, token, seller.clone(), proceeds);
        event(
            "sell",
            near_sdk::serde_json::json!({"id": question_id, "seller": seller, "range": range, "yes": yes,
                "tickets": tickets, "refund": U128(fill.amount), "fee": U128(fill.fee)}),
        );
        U128(proceeds)
    }

    /// Moves tokens to another account. Token ids are "<question>:yes:<answer>" or "<question>:no:<answer>". Attach at
    /// least 1 yoctoNEAR, and if the receiver holds nothing of this question, the cost of their whole row: 0.01 NEAR
    /// always covers it, and what is not needed comes back. A row the move frees goes to the question's balance, even
    /// when the receiver's row is new, so buying and moving on cannot make the question pay for row after row. Tokens
    /// sent to an account nobody controls are gone.
    #[payable]
    pub fn mt_transfer(&mut self, receiver_id: AccountId, token_id: String, amount: U128, memo: Option<String>) {
        let paid = env::attached_deposit().as_yoctonear();
        require!(paid >= 1, "AIMM: attach at least 1 yoctoNEAR");
        let from = env::predecessor_account_id();
        require!(from != receiver_id, "AIMM: sender is the receiver");
        require!(receiver_id != env::current_account_id(), "AIMM: the receiver is this contract");
        require!(amount.0 > 0, "AIMM: ZeroAmount");
        let (id, yes, range) = parse_token_id(&token_id);
        let n = self.market(id).q.n();
        require!(range < n, "AIMM: BadRange");
        let slot = if yes { range } else { n + range };
        let before = env::storage_usage();
        let mut a = self.held(id, &from, n);
        require!(a[slot] >= amount.0, "AIMM: not enough tokens");
        a[slot] -= amount.0;
        self.set_held(id, &from, a);
        self.balances.flush();
        let sender_done = env::storage_usage();
        let mut b = self.held(id, &receiver_id, n);
        b[slot] += amount.0;
        self.set_held(id, &receiver_id, b);
        self.balances.flush();
        let cost = storage_cost(env::storage_usage() - sender_done);
        require!(paid >= cost, format!("AIMM: attach {cost} yoctoNEAR for the receiver's row"));
        self.market_mut(id).storage += storage_cost(before - sender_done);
        let back = paid - cost.max(1);
        if back > 0 {
            Promise::new(from.clone()).transfer(NearToken::from_yoctonear(back)).detach();
        }
        event("transfer", near_sdk::serde_json::json!({"from": from, "to": receiver_id, "token_id": token_id, "amount": amount, "memo": memo}));
    }

    // ------------------------------------------------------------------------------------------------ operator

    /// The operator takes the fees collected so far. Attach exactly 1 yoctoNEAR. Returns the amount owed, not the amount
    /// delivered.
    #[payable]
    pub fn claim_fees(&mut self, question_id: u64) -> U128 {
        assert_one_yocto();
        let caller = env::predecessor_account_id();
        let m = self.market_mut(question_id);
        require!(m.operator == caller, "AIMM: only the operator");
        let amount = m.q.claim_fees();
        let token = m.collateral.clone();
        self.pay(question_id, token, caller.clone(), amount);
        event("claim_fees", near_sdk::serde_json::json!({"id": question_id, "to": caller, "amount": U128(amount)}));
        U128(amount)
    }

    /// The operator takes what the result left over in the pot. Attach exactly 1 yoctoNEAR. Returns the amount owed, not
    /// the amount delivered.
    #[payable]
    pub fn claim_surplus(&mut self, question_id: u64) -> U128 {
        assert_one_yocto();
        let caller = env::predecessor_account_id();
        let m = self.market_mut(question_id);
        require!(m.operator == caller, "AIMM: only the operator");
        let amount = m.q.claim_surplus().unwrap_or_else(|e| fail(e));
        let token = m.collateral.clone();
        self.pay(question_id, token, caller.clone(), amount);
        event("claim_surplus", near_sdk::serde_json::json!({"id": question_id, "to": caller, "amount": U128(amount)}));
        U128(amount)
    }

    // ------------------------------------------------------------------------------------------------ settlement

    /// Stops trading now, before the close, saying nothing about the result. The resolver or the operator. Attach
    /// exactly 1 yoctoNEAR, so a function-call key cannot do it.
    #[payable]
    pub fn halt(&mut self, question_id: u64) {
        assert_one_yocto();
        let caller = env::predecessor_account_id();
        let m = self.market_mut(question_id);
        require!(caller == m.resolver || caller == m.operator, "AIMM: only the resolver or the operator");
        m.q.halt().unwrap_or_else(|e| fail(e));
        event("halt", near_sdk::serde_json::json!({"id": question_id, "by": caller}));
    }

    /// Reports the winning answer. The resolver, once trading has stopped and from `resolve_after`; the fallback
    /// resolver from `fallback_after`. Final. It pays no one: winners redeem, the operator claims the surplus. Attach
    /// exactly 1 yoctoNEAR, so a function-call key cannot do it.
    #[payable]
    pub fn resolve(&mut self, question_id: u64, winner: u8) -> U128 {
        assert_one_yocto();
        let caller = env::predecessor_account_id();
        let now = env::block_timestamp();
        let m = self.market_mut(question_id);
        let by_fallback = if caller == m.resolver {
            false
        } else if caller == m.fallback_resolver {
            true
        } else {
            env::panic_str("AIMM: only the resolver or the fallback resolver")
        };
        let r = m.q.resolve(winner as usize, by_fallback, now).unwrap_or_else(|e| fail(e));
        event("resolve", near_sdk::serde_json::json!({"id": question_id, "by": caller, "winner": winner, "payout": U128(r), "surplus": U128(m.q.surplus)}));
        U128(r)
    }

    /// Settles `account_id` (the caller by default): pays the payout on their winning tokens to them, drops their losing
    /// tokens, which pay nothing, and frees their row, which pays for the transfer's hold and credits the rest to the
    /// storage balance. Anyone may call this: the money only ever goes to the holder. Attach exactly 1 yoctoNEAR. Returns
    /// the amount owed, not the amount delivered.
    #[payable]
    pub fn redeem(&mut self, question_id: u64, account_id: Option<AccountId>) -> U128 {
        assert_one_yocto();
        let holder = account_id.unwrap_or_else(env::predecessor_account_id);
        let before = env::storage_usage();
        let n = self.market(question_id).q.n();
        let h = self.held(question_id, &holder, n);
        let m = self.market_mut(question_id);
        let (tickets, amount) = match m.q.redeem(&h[..n], &h[n..]) {
            Ok((burned, amount)) => (burned.iter().map(|b| b.2).sum(), amount),
            Err(AimmError::ZeroAmount) if h.iter().any(|&x| x > 0) => (0, 0),
            Err(e) => fail(e),
        };
        m.q.clear_losing(&h[..n], &h[n..]).unwrap_or_else(|e| fail(e));
        let token = m.collateral.clone();
        self.balances.remove(&(question_id, holder.clone()));
        self.settle_storage(question_id, before);
        self.pay(question_id, token, holder.clone(), amount);
        event("redeem", near_sdk::serde_json::json!({"id": question_id, "holder": holder, "tickets": U128(tickets), "amount": U128(amount)}));
        U128(amount)
    }

    // ------------------------------------------------------------------------------------------------ views

    pub fn question_count(&self) -> u64 {
        self.count
    }

    pub fn get_question(&self, question_id: u64) -> QuestionView {
        let m = self.market(question_id);
        let q = &m.q;
        let n = q.n();
        let live = q.pool.is_some() && !q.resolved;
        let prices = if live { q.prices().unwrap() } else { vec![] };
        let intervals = if live {
            (0..n).map(|i| { let (lo, hi) = q.information_interval(i).unwrap(); [U128(lo), U128(hi)] }).collect()
        } else {
            vec![]
        };
        QuestionView {
            id: question_id,
            title: m.title.clone(),
            answers: m.answers.clone(),
            collateral: m.collateral.clone(),
            operator: m.operator.clone(),
            resolver: m.resolver.clone(),
            fallback_resolver: m.fallback_resolver.clone(),
            close_time: U64(q.close_time),
            resolve_after: U64(q.resolve_after),
            fallback_after: U64(q.fallback_after),
            fee_rate: U128(q.fee_rate),
            opened: q.pool.is_some(),
            halted: q.halted,
            resolved: q.resolved,
            winner: if q.resolved { Some(q.winner) } else { None },
            depth: U128(q.pool.as_ref().map(|p| p.b as u128).unwrap_or(0)),
            virtual_liquidity: U128(q.virtual_liquidity().unwrap_or(0)),
            fund: U128(q.fund),
            pot: U128(q.pot),
            fees_paid: U128(q.fees_paid),
            fees_owed: U128(q.fees_owed),
            surplus: U128(q.surplus),
            storage: U128(m.storage),
            prices: u(&prices),
            intervals,
            payouts: (0..n).map(|w| U128(q.payout_if(w).unwrap())).collect(),
            yes_supply: u(&q.yes),
            no_supply: u(&q.no),
        }
    }

    /// What buying `tickets` costs now: the curve's cost, the fee, the total to send, and every price after.
    pub fn quote_buy(&self, question_id: u64, range: u8, yes: bool, tickets: U128) -> Quote {
        let (f, after) = self.market(question_id).q.quote_buy(range as usize, yes, tickets.0, env::block_timestamp()).unwrap_or_else(|e| fail(e));
        Quote { amount: U128(f.amount), fee: U128(f.fee), total: U128(f.amount + f.fee), prices_after: u(&after) }
    }

    /// What selling `tickets` returns now: the capped refund, the fee, the proceeds, and every price after.
    pub fn quote_sell(&self, question_id: u64, range: u8, yes: bool, tickets: U128) -> Quote {
        let (f, after) = self.market(question_id).q.quote_sell(range as usize, yes, tickets.0, env::block_timestamp()).unwrap_or_else(|e| fail(e));
        Quote { amount: U128(f.amount), fee: U128(f.fee), total: U128(f.amount - f.fee), prices_after: u(&after) }
    }

    /// The prediction range [lower, upper] for every answer (WAD).
    pub fn information_intervals(&self, question_id: u64) -> Vec<[U128; 2]> {
        let q = &self.market(question_id).q;
        (0..q.n()).map(|i| { let (lo, hi) = q.information_interval(i).unwrap_or_else(|e| fail(e)); [U128(lo), U128(hi)] }).collect()
    }

    pub fn holdings(&self, question_id: u64, account_id: AccountId) -> Holdings {
        let n = self.market(question_id).q.n();
        let h = self.held(question_id, &account_id, n);
        Holdings { yes: u(&h[..n]), no: u(&h[n..]) }
    }

    pub fn mt_balance_of(&self, account_id: AccountId, token_id: String) -> U128 {
        let (id, yes, range) = parse_token_id(&token_id);
        let n = self.market(id).q.n();
        require!(range < n, "AIMM: BadRange");
        U128(self.held(id, &account_id, n)[if yes { range } else { n + range }])
    }

    pub fn pending_of(&self, question_id: u64, account_id: AccountId) -> U128 {
        U128(self.pending.get(&(question_id, account_id)).copied().unwrap_or(0))
    }

    /// The curve's book: q and q0 for every answer, in the collateral's smallest unit, as decimal strings because they
    /// can be negative. Empty until the question opens.
    pub fn book(&self, question_id: u64) -> (Vec<String>, Vec<String>) {
        match &self.market(question_id).q.pool {
            Some(p) => (p.q.iter().map(|x| x.to_string()).collect(), p.q0.iter().map(|x| x.to_string()).collect()),
            None => (vec![], vec![]),
        }
    }

    /// One, in WAD, so clients can scale prices.
    pub fn wad(&self) -> U128 {
        U128(WAD)
    }
}

#[cfg(test)]
mod tests;
