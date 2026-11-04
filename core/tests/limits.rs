//! Every error of `question.rs` and every limit of SPEC.md section 5, on both sides of each edge. A refusal must also
//! leave the question as it was.
mod common;
use aimm_core::lmsr::Pool;
use aimm_core::math::{mul_div, mul_div_up, WAD};
use aimm_core::question::AimmError::*;
use aimm_core::question::{AimmError, Question, MAX_DEPTH, MAX_FEE_RATE, MAX_RANGES, MAX_TICKETS, MIN_OPENING_PRICE, MIN_RANGES};

const P: [u128; 3] = [WAD / 2, 3 * WAD / 10, WAD / 5];
/// 1,000 dollars at 6 decimals.
const DEPTH: u128 = 1_000_000_000;
const TOKEN: u128 = 1_000_000;
const CLOSE: u64 = 100;
const RESOLVE: u64 = 200;
const FALLBACK: u64 = 300;

fn fresh() -> Question {
    Question::new(3, 0, CLOSE, RESOLVE, FALLBACK, 0).unwrap()
}

fn opened(fee: u128, fund: u128) -> Question {
    let mut q = Question::new(3, fee, CLOSE, RESOLVE, FALLBACK, 0).unwrap();
    q.open(DEPTH, fund, &P, 0).unwrap();
    q
}

/// Opened, with YES and NO in supply on answer 0, so a sale is possible.
fn traded() -> Question {
    let mut q = opened(0, 500 * TOKEN);
    q.buy(0, true, 10 * TOKEN, 0).unwrap();
    q.buy(0, false, 10 * TOKEN, 0).unwrap();
    q
}

fn new_error(n: usize, fee: u128, close: u64, resolve: u64, fallback: u64, now: u64) -> AimmError {
    Question::new(n, fee, close, resolve, fallback, now).unwrap_err()
}

/// buy, sell and both quotes all refuse this trade with `error`, and the question does not change.
fn trade_refused(q: &Question, range: usize, tickets: u128, now: u64, error: AimmError) {
    for yes in [true, false] {
        let mut c = q.clone();
        assert_eq!(c.buy(range, yes, tickets, now), Err(error), "buy");
        assert_eq!(&c, q, "a refused buy changed the question");
        assert_eq!(c.sell(range, yes, tickets, now), Err(error), "sell");
        assert_eq!(&c, q, "a refused sale changed the question");
        assert_eq!(c.quote_buy(range, yes, tickets, now).unwrap_err(), error, "quote_buy");
        assert_eq!(c.quote_sell(range, yes, tickets, now).unwrap_err(), error, "quote_sell");
    }
}

#[test]
fn an_error_prints_as_its_name() {
    // the contract panics with "AIMM: <this>", and clients and its tests read that text
    assert_eq!(SaleTooSmall.to_string(), "SaleTooSmall");
    assert_eq!(BadRange.to_string(), "BadRange");
}

// ------------------------------------------------------------------------------------------------ creating

#[test]
fn a_new_question_holds_nothing() {
    let q = Question::new(3, 7, CLOSE, RESOLVE, FALLBACK, 0).unwrap();
    assert_eq!((q.fund, q.pot, q.fees_paid, q.fees_owed, q.no_total, q.payout, q.surplus), (0, 0, 0, 0, 0, 0, 0));
    assert_eq!((q.winner, q.halted, q.resolved, q.surplus_claimed), (0, false, false, false));
    assert_eq!((q.fee_rate, q.close_time, q.resolve_after, q.fallback_after), (7, CLOSE, RESOLVE, FALLBACK));
    assert_eq!((q.yes.clone(), q.no.clone()), (vec![0; 3], vec![0; 3]));
    assert!(q.pool.is_none());
}

#[test]
fn a_question_has_3_to_16_answers() {
    assert_eq!((MIN_RANGES, MAX_RANGES), (3, 16));
    for n in [0, 1, 2, 17, 255, 256, 259] {
        assert_eq!(new_error(n, 0, 10, 10, 10, 0), BadRange, "{n} answers");
    }
    for n in [3, 4, 16] {
        let q = Question::new(n, 0, 10, 10, 10, 0).unwrap();
        assert_eq!((q.n(), q.yes.len(), q.no.len()), (n, n, n));
    }
}

#[test]
fn the_times_must_be_ahead_and_in_order() {
    assert_eq!(new_error(3, 0, 10, 10, 10, 10), BadConfig, "closing now");
    assert_eq!(new_error(3, 0, 10, 10, 10, 11), BadConfig, "closed already");
    assert_eq!(new_error(3, 0, 10, 9, 10, 0), BadConfig, "resolving before the close");
    assert_eq!(new_error(3, 0, 10, 12, 11, 0), BadConfig, "the fallback before the resolver");
    assert!(Question::new(3, 0, 11, 11, 11, 10).is_ok(), "all three at the same time, one tick ahead");
    assert!(Question::new(3, 0, 10, 11, 12, 0).is_ok());
}

#[test]
fn the_fee_is_at_most_a_tenth() {
    assert_eq!(MAX_FEE_RATE, WAD / 10);
    assert_eq!(new_error(3, MAX_FEE_RATE + 1, 10, 10, 10, 0), BadConfig);
    assert_eq!(Question::new(3, MAX_FEE_RATE, 10, 10, 10, 0).unwrap().fee_rate, MAX_FEE_RATE);
}

// ------------------------------------------------------------------------------------------------ opening

#[test]
fn open_wants_one_price_for_each_answer() {
    let mut q = fresh();
    assert_eq!(q.open(DEPTH, 0, &P[..2], 0), Err(BadRange));
    assert_eq!(q.open(DEPTH, 0, &[WAD / 4; 4], 0), Err(BadRange));
    assert_eq!(q, fresh());
}

#[test]
fn opening_prices_add_up_to_exactly_one() {
    let mut q = fresh();
    assert_eq!(q.open(DEPTH, 0, &[WAD / 2, 3 * WAD / 10, WAD / 5 + 1], 0), Err(BadPrices));
    assert_eq!(q.open(DEPTH, 0, &[WAD / 2, 3 * WAD / 10, WAD / 5 - 1], 0), Err(BadPrices));
    assert_eq!(q.open(DEPTH, 0, &[WAD / 2, WAD / 2, WAD / 2], 0), Err(BadPrices));
    assert_eq!(q, fresh());
}

#[test]
fn an_opening_price_is_at_least_one_in_a_million() {
    assert_eq!(MIN_OPENING_PRICE, WAD / 1_000_000);
    for at in 0..3 {
        let mut p = [0; 3];
        p[at] = MIN_OPENING_PRICE - 1;
        p[(at + 1) % 3] = MIN_OPENING_PRICE;
        p[(at + 2) % 3] = WAD - 2 * MIN_OPENING_PRICE + 1;
        assert_eq!(p.iter().sum::<u128>(), WAD);
        assert_eq!(fresh().open(DEPTH, 0, &p, 0), Err(BadPrices), "{p:?}");
    }
    let mut q = fresh();
    let p = [MIN_OPENING_PRICE, MIN_OPENING_PRICE, WAD - 2 * MIN_OPENING_PRICE];
    q.open(DEPTH, 0, &p, 0).unwrap();
    assert!(q.buy(0, true, 100 * TOKEN, 0).is_ok(), "trading at the lowest price allowed");
}

#[test]
fn a_price_large_enough_to_overflow_the_sum_is_refused_as_a_bad_price() {
    let mut q = fresh();
    assert_eq!(q.open(DEPTH, 0, &[u128::MAX, MIN_OPENING_PRICE, MIN_OPENING_PRICE], 0), Err(BadPrices));
}

#[test]
fn the_depth_is_above_zero_and_at_most_2_to_the_100() {
    assert_eq!(MAX_DEPTH, 1 << 100);
    let mut q = fresh();
    assert_eq!(q.open(0, 0, &P, 0), Err(BadDepth));
    assert_eq!(q.open(MAX_DEPTH + 1, 0, &P, 0), Err(BadDepth));
    assert_eq!(q, fresh());
    q.open(MAX_DEPTH, 0, &P, 0).unwrap();
    assert_eq!(q.pool.as_ref().unwrap().b as u128, MAX_DEPTH);
    let mut q = fresh();
    q.open(1, 0, &P, 0).unwrap();
    assert_eq!(q.pool.as_ref().unwrap().b, 1);
}

#[test]
fn open_puts_the_fund_in_the_pot() {
    let q = opened(0, 123);
    assert_eq!((q.fund, q.pot), (123, 123));
    assert!(q.pool.is_some());
}

#[test]
fn a_question_opens_once() {
    let mut q = traded();
    let before = q.clone();
    assert_eq!(q.open(DEPTH, 7, &P, 0), Err(AlreadyOpened));
    assert_eq!(q, before);
}

#[test]
fn a_question_cannot_open_at_or_after_the_close() {
    assert_eq!(fresh().open(DEPTH, 0, &P, CLOSE), Err(TradingClosed));
    assert_eq!(fresh().open(DEPTH, 0, &P, CLOSE + 1), Err(TradingClosed));
    assert!(fresh().open(DEPTH, 0, &P, CLOSE - 1).is_ok());
}

#[test]
fn a_halted_question_cannot_open() {
    // `halt` refuses an unopened question, so only a state set by hand reaches this
    let mut q = fresh();
    q.halted = true;
    assert_eq!(q.open(DEPTH, 0, &P, 0), Err(TradingClosed));
}

// ------------------------------------------------------------------------------------------------ before the opening

#[test]
fn nothing_trades_before_the_opening() {
    let q = fresh();
    trade_refused(&q, 0, TOKEN, 0, NotOpen);
    assert_eq!(q.prices(), Err(NotOpen));
    assert_eq!(q.virtual_liquidity(), Err(NotOpen));
    assert_eq!(q.information_interval(0), Err(NotOpen));
    let mut c = q.clone();
    assert_eq!(c.halt(), Err(NotOpen));
    assert_eq!(c.resolve(0, false, RESOLVE), Err(NotOpen));
    assert_eq!(c.resolve(0, true, FALLBACK), Err(NotOpen));
    assert_eq!(c.claim_surplus(), Err(NotResolved));
    assert_eq!(c, q);
}

#[test]
fn before_the_opening_nothing_is_owed_so_the_payout_is_one() {
    assert_eq!(fresh().payout_if(0), Ok(WAD));
}

// ------------------------------------------------------------------------------------------------ the guards of a trade

#[test]
fn trading_stops_at_the_close() {
    let q = traded();
    let mut c = q.clone();
    assert!(c.buy(0, true, TOKEN, CLOSE - 1).is_ok());
    assert!(c.sell(0, true, TOKEN, CLOSE - 1).is_ok());
    assert!(q.quote_buy(0, true, TOKEN, CLOSE - 1).is_ok());
    assert!(q.quote_sell(0, true, TOKEN, CLOSE - 1).is_ok());
    trade_refused(&q, 0, TOKEN, CLOSE, TradingClosed);
    trade_refused(&q, 0, TOKEN, CLOSE + 1, TradingClosed);
}

#[test]
fn a_halt_stops_trading_at_once_and_for_good() {
    let mut q = traded();
    q.halt().unwrap();
    assert!(q.halted);
    trade_refused(&q, 0, TOKEN, 0, TradingClosed);
    assert_eq!(q.halt(), Ok(()), "halting twice changes nothing");
    assert!(q.halted);
}

#[test]
fn no_trade_after_the_result() {
    let mut q = traded();
    q.resolve(0, false, RESOLVE).unwrap();
    trade_refused(&q, 0, TOKEN, RESOLVE, NotOpen);
    // before the close too: the resolved test comes before the time test
    trade_refused(&q, 0, TOKEN, 0, NotOpen);
}

#[test]
fn the_answer_must_exist() {
    let q = traded();
    trade_refused(&q, 3, TOKEN, 0, BadRange);
    trade_refused(&q, 255, TOKEN, 0, BadRange);
    assert_eq!(q.payout_if(3), Err(BadRange));
    assert_eq!(q.information_interval(3), Err(BadRange));
    let mut c = q.clone();
    c.halt().unwrap();
    assert_eq!(c.resolve(3, false, RESOLVE), Err(BadRange));
    assert_eq!(c.resolve(3, true, FALLBACK), Err(BadRange));
    assert!(!c.resolved);
}

#[test]
fn a_trade_is_of_at_least_one_token() {
    trade_refused(&traded(), 0, 0, 0, ZeroAmount);
}

#[test]
fn a_trade_moves_at_most_2_to_the_100_tokens() {
    assert_eq!(MAX_TICKETS, 1 << 100);
    trade_refused(&traded(), 0, MAX_TICKETS + 1, 0, TooManyTickets);
    let mut q = Question::new(3, 0, CLOSE, RESOLVE, FALLBACK, 0).unwrap();
    q.open(MAX_DEPTH, 0, &P, 0).unwrap();
    assert!(q.quote_buy(0, true, MAX_TICKETS, 0).is_ok());
    q.buy(0, true, MAX_TICKETS, 0).unwrap();
    assert_eq!(q.yes[0], MAX_TICKETS);
    q.sell(0, true, MAX_TICKETS, 0).unwrap();
    assert_eq!(q.yes[0], 0);
}

#[test]
fn no_answer_carries_more_than_2_to_the_100_winning_tokens() {
    let at_the_limit = |range, yes| {
        let mut q = Question::new(3, 0, CLOSE, RESOLVE, FALLBACK, 0).unwrap();
        q.open(MAX_DEPTH, 0, &P, 0).unwrap();
        q.buy(range, yes, MAX_TICKETS, 0).unwrap();
        q
    };
    // YES on 0 fills answer 0, and a NO on another answer pays on 0 too
    let q = at_the_limit(0, true);
    assert_eq!(q.winning_supply(0), MAX_TICKETS);
    let mut c = q.clone();
    assert_eq!(c.buy(0, true, 1, 0), Err(TooManyTickets));
    assert_eq!(c.buy(1, false, 1, 0), Err(TooManyTickets));
    assert_eq!(c.buy(2, false, 1, 0), Err(TooManyTickets));
    assert_eq!(c, q);
    assert!(c.buy(1, true, 1, 0).is_ok(), "a YES on 1 does not pay on 0");
    assert!(c.buy(0, false, 1, 0).is_ok(), "a NO on 0 does not pay on 0");
    // a sale makes room again
    let mut c = q.clone();
    c.sell(0, true, 1, 0).unwrap();
    assert!(c.buy(0, true, 1, 0).is_ok());
    assert_eq!(c.buy(0, true, 1, 0), Err(TooManyTickets));
    // NO on 0 fills answers 1 and 2, and leaves answer 0 empty
    let q = at_the_limit(0, false);
    assert_eq!((q.winning_supply(0), q.winning_supply(1), q.winning_supply(2)), (0, MAX_TICKETS, MAX_TICKETS));
    let mut c = q.clone();
    assert_eq!(c.buy(1, true, 1, 0), Err(TooManyTickets));
    assert_eq!(c.buy(2, true, 1, 0), Err(TooManyTickets));
    assert_eq!(c.buy(1, false, 1, 0), Err(TooManyTickets), "a NO on 1 pays on 2");
    assert_eq!(c.buy(0, false, 1, 0), Err(TooManyTickets));
    assert!(c.buy(0, true, 1, 0).is_ok(), "a YES on 0 pays only on 0");
}

#[test]
fn a_sale_cannot_pass_the_supply_of_that_token() {
    let mut q = traded();
    let before = q.clone();
    assert_eq!(q.sell(0, true, 10 * TOKEN + 1, 0), Err(SupplyShort));
    assert_eq!(q.sell(0, false, 10 * TOKEN + 1, 0), Err(SupplyShort));
    assert_eq!(q.sell(1, true, 1, 0), Err(SupplyShort), "YES on 1 was never bought");
    assert_eq!(q.sell(1, false, 1, 0), Err(SupplyShort), "NO on 1 was never bought");
    assert_eq!(q, before);
    assert!(q.sell(0, true, 10 * TOKEN, 0).is_ok(), "the whole supply can go");
    assert!(q.sell(0, false, 10 * TOKEN, 0).is_ok());
    assert_eq!((q.yes[0], q.no[0], q.no_total), (0, 0, 0));
}

#[test]
fn a_refund_above_the_pot_is_refused() {
    // no sequence of trades reaches this: it takes a supply set by hand, so that the payout is 1 on a pot of 1
    let mut q = traded();
    q.no_total = 1;
    q.pot = 1;
    let before = q.clone();
    assert_eq!(q.sell(0, false, 10 * TOKEN, 0), Err(PotShort));
    assert_eq!(q, before);
}

#[test]
fn a_refund_of_the_whole_pot_is_paid() {
    // set by hand too: a pot of 4 tokens on 10 winning tokens pays .4, so selling all 10 refunds exactly the pot
    let mut q = traded();
    q.pot = 4 * TOKEN;
    assert_eq!(q.sell(0, true, 10 * TOKEN, 0).unwrap().amount, 4 * TOKEN);
    assert_eq!(q.pot, 0);
}

// ------------------------------------------------------------------------------------------------ the curve's own checks
// `Question::open` refuses all of these first, so only a direct call reaches them.

#[test]
#[should_panic(expected = "depth must be positive")]
fn the_curve_refuses_a_depth_of_zero() {
    Pool::open(0, &P);
}

#[test]
#[should_panic(expected = "depth too large")]
fn the_curve_refuses_a_depth_past_i128() {
    Pool::open(1 << 127, &P);
}

#[test]
#[should_panic(expected = "opening price out of range")]
fn the_curve_refuses_a_price_of_zero() {
    Pool::open(DEPTH, &[0, WAD]);
}

#[test]
#[should_panic(expected = "opening price out of range")]
fn the_curve_refuses_a_price_above_one() {
    Pool::open(DEPTH, &[WAD + 1]);
}

#[test]
fn the_curve_takes_prices_from_one_wei_up_to_exactly_one() {
    assert_eq!(Pool::open(DEPTH, &[WAD]).q0, vec![0]);
    // ln 1e-18 = -41.446531673892822
    assert_eq!(Pool::open(DEPTH, &[1, WAD - 1]).q0, vec![-41_446_531_673, 0]);
}

#[test]
fn on_a_book_with_nothing_below_zero_l_is_zero() {
    assert_eq!(Pool::open(DEPTH, &[WAD]).virtual_liquidity(), 0);
}

// ------------------------------------------------------------------------------------------------ a quote says what the trade will do

#[test]
fn clearing_losing_tokens_takes_them_out_of_the_supply_and_touches_nothing_else() {
    let mut q = Question::new(3, 0, 10, 10, 10, 0).unwrap();
    q.open(1_000_000, 1_000_000, &[WAD / 2, WAD / 4, WAD / 4], 0).unwrap();
    q.buy(0, true, 50, 0).unwrap();
    q.buy(1, true, 70, 0).unwrap();
    q.buy(0, false, 30, 0).unwrap();
    q.buy(2, false, 20, 0).unwrap();
    assert_eq!(q.clear_losing(&[0; 3], &[0; 3]), Err(AimmError::NotResolved));
    q.resolve(0, false, 10).unwrap();
    let (pot, owed) = (q.pot, q.winning_supply(0));
    q.clear_losing(&[50, 70, 0], &[30, 0, 20]).unwrap();
    assert_eq!((q.yes[0], q.yes[1], q.no[0], q.no[2], q.no_total), (50, 0, 0, 20, 20), "only YES on a loser and NO on the winner go");
    assert_eq!((q.pot, q.winning_supply(0)), (pot, owed));
}

#[test]
fn a_quote_is_refused_when_the_trade_would_be() {
    let q = traded();
    assert_eq!(q.quote_sell(0, true, 10 * TOKEN + 1, 0).unwrap_err(), SupplyShort, "selling more than exists");
    assert_eq!(q.quote_sell(1, true, 1, 0).unwrap_err(), SupplyShort, "selling what was never bought");
    let mut full = Question::new(3, 0, CLOSE, RESOLVE, FALLBACK, 0).unwrap();
    full.open(MAX_DEPTH, 0, &P, 0).unwrap();
    full.buy(0, true, MAX_TICKETS, 0).unwrap();
    assert_eq!(full.quote_buy(0, true, 1, 0).unwrap_err(), TooManyTickets, "buying past the limit");
}

// ------------------------------------------------------------------------------------------------ the fee

#[test]
fn the_fee_at_exactly_the_most_allowed() {
    // SPEC section 1, at depth 1e9: buying 1e9 YES at .5 costs 620114508, and a tenth of that is 62011450.8
    let mut q = opened(MAX_FEE_RATE, 0);
    assert_eq!(q.quote_buy(0, true, DEPTH, 0).unwrap().0.fee, 62_011_451);
    let f = q.buy(0, true, DEPTH, 0).unwrap();
    assert_eq!((f.amount, f.fee), (620_114_508, 62_011_451));
    assert_eq!((q.fees_paid, q.fees_owed, q.pot), (62_011_451, 62_011_451, 620_114_508));
}

#[test]
fn the_fee_rounds_up_and_only_when_it_must() {
    // one wei of fee rate on 10 tokens is far below a unit, which rounds up to a whole unit
    let mut q = opened(1, 500 * TOKEN);
    assert_eq!(q.buy(0, true, 10 * TOKEN, 0).unwrap().fee, 1);
    assert_eq!(q.sell(0, true, 10 * TOKEN, 0).unwrap().fee, 1);
    // at a tenth, a cost that is a multiple of 10 pays exactly a tenth
    let mut exact = 0;
    for n in 1..200 {
        let mut q = opened(MAX_FEE_RATE, 0);
        let f = q.buy(1, true, n * TOKEN, 0).unwrap();
        assert_eq!(f.fee, f.amount.div_ceil(10), "{} tokens", n);
        exact += (f.amount % 10 == 0) as u32;
    }
    assert!(exact >= 5, "only {exact} costs were multiples of 10");
}

#[test]
fn with_no_fee_rate_there_is_no_fee() {
    let mut q = opened(0, 500 * TOKEN);
    assert_eq!(q.buy(0, true, 10 * TOKEN, 0).unwrap().fee, 0);
    assert_eq!(q.sell(0, true, 10 * TOKEN, 0).unwrap().fee, 0);
    assert_eq!((q.fees_paid, q.fees_owed), (0, 0));
}

#[test]
fn a_capped_sale_pays_the_fee_on_the_refund_not_on_the_curve() {
    // three even answers, depth 1000 dollars, fund 300, 3000 YES on 0 (): a sale of 100 has a curve refund of
    // 91.345014 but a cap of 77.021151, and the fee at a tenth is a tenth of the cap, rounded up
    let mut q = Question::new(3, MAX_FEE_RATE, CLOSE, RESOLVE, FALLBACK, 0).unwrap();
    q.open(DEPTH, 300 * TOKEN, &[WAD - 2 * (WAD / 3), WAD / 3, WAD / 3], 0).unwrap();
    q.buy(0, true, 3100 * TOKEN, 0).unwrap();
    let f = q.sell(0, true, 100 * TOKEN, 0).unwrap();
    assert_eq!((f.amount, f.fee), (77_021_151, 7_702_116));
}

#[test]
fn the_operator_takes_the_fees_at_any_time_and_only_once() {
    let mut q = opened(MAX_FEE_RATE, 0);
    assert_eq!(q.claim_fees(), 0);
    let a = q.buy(0, true, DEPTH, 0).unwrap().fee;
    assert_eq!(q.claim_fees(), a);
    assert_eq!(q.claim_fees(), 0);
    let b = q.sell(0, true, DEPTH / 2, 0).unwrap().fee;
    assert_eq!((q.fees_paid, q.fees_owed), (a + b, b));
    q.halt().unwrap();
    assert_eq!(q.claim_fees(), b);
    assert_eq!((q.fees_paid, q.fees_owed), (a + b, 0));
}

// ------------------------------------------------------------------------------------------------ the result

#[test]
fn the_resolver_waits_for_the_close_and_for_resolve_after() {
    let mut q = traded();
    let before = q.clone();
    assert_eq!(q.resolve(0, false, CLOSE - 1), Err(StillTrading));
    assert_eq!(q.resolve(0, false, CLOSE), Err(TooEarly));
    assert_eq!(q.resolve(0, false, RESOLVE - 1), Err(TooEarly));
    assert_eq!(q, before);
    q.resolve(1, false, RESOLVE).unwrap();
    assert_eq!((q.resolved, q.winner), (true, 1));
}

#[test]
fn a_halt_ends_the_trading_but_not_the_wait_for_resolve_after() {
    let mut q = traded();
    q.halt().unwrap();
    assert_eq!(q.resolve(0, false, CLOSE - 1), Err(TooEarly));
    assert_eq!(q.resolve(0, false, RESOLVE - 1), Err(TooEarly));
    assert!(q.resolve(0, false, RESOLVE).is_ok());
}

#[test]
fn the_fallback_resolver_waits_for_fallback_after() {
    let mut q = traded();
    let before = q.clone();
    assert_eq!(q.resolve(0, true, RESOLVE), Err(TooEarly));
    assert_eq!(q.resolve(0, true, FALLBACK - 1), Err(TooEarly));
    assert_eq!(q, before);
    q.resolve(2, true, FALLBACK).unwrap();
    assert_eq!((q.resolved, q.winner), (true, 2));
}

#[test]
fn a_result_is_final() {
    let mut q = traded();
    q.resolve(1, false, RESOLVE).unwrap();
    let before = q.clone();
    assert_eq!(q.resolve(1, false, RESOLVE), Err(AlreadyResolved));
    assert_eq!(q.resolve(0, false, RESOLVE + 1), Err(AlreadyResolved));
    assert_eq!(q.resolve(0, true, FALLBACK), Err(AlreadyResolved));
    assert_eq!(q.halt(), Err(AlreadyResolved));
    assert_eq!(q, before);
}

#[test]
fn after_the_result_the_winner_is_paid_the_payout_and_everyone_else_nothing() {
    let mut q = opened(0, 0);
    q.buy(0, true, 3000 * TOKEN, 0).unwrap();
    let r = q.resolve(0, false, RESOLVE).unwrap();
    assert!(r < WAD);
    assert_eq!(q.payout_if(0), Ok(r));
    assert_eq!(q.payout_if(1), Ok(0));
    assert_eq!(q.payout_if(2), Ok(0));
    assert_eq!(q.payout_if(3), Err(BadRange));
}

#[test]
fn after_the_result_there_are_no_prices() {
    let mut q = traded();
    q.resolve(0, false, RESOLVE).unwrap();
    assert_eq!(q.prices(), Err(NotOpen));
    assert_eq!(q.information_interval(0), Err(NotOpen));
    assert_eq!(q.information_interval(3), Err(BadRange));
    assert!(q.virtual_liquidity().is_ok(), "the depth and the opening prices are still there");
}

#[test]
fn the_surplus_is_claimed_once_and_only_after_the_result() {
    let mut q = traded();
    assert_eq!(q.claim_surplus(), Err(NotResolved));
    q.resolve(1, false, RESOLVE).unwrap();
    let surplus = q.surplus;
    assert!(surplus > 0);
    assert_eq!(q.claim_surplus(), Ok(surplus));
    assert_eq!((q.surplus, q.surplus_claimed), (0, true));
    let before = q.clone();
    assert_eq!(q.claim_surplus(), Err(AlreadyClaimed));
    assert_eq!(q, before);
}

#[test]
fn redeem_needs_the_result_and_a_winning_token() {
    let (mut yes, mut no) = ([0; 3], [0; 3]);
    let mut q = traded();
    yes[0] = 10 * TOKEN;
    no[0] = 10 * TOKEN;
    assert_eq!(q.redeem(&yes, &no).unwrap_err(), NotResolved);
    // answer 0 wins: the YES on 0 pays and the NO on 0 does not
    q.resolve(0, false, RESOLVE).unwrap();
    let before = q.clone();
    assert_eq!(q.redeem(&[0; 3], &[0; 3]).unwrap_err(), ZeroAmount, "nothing at all");
    assert_eq!(q.redeem(&[0; 3], &no).unwrap_err(), ZeroAmount, "only losing tokens");
    assert_eq!(q, before);
    let (burned, paid) = q.redeem(&yes, &no).unwrap();
    assert_eq!(burned, vec![(0, true, 10 * TOKEN)], "the NO on 0 stays");
    assert_eq!(paid, 10 * TOKEN);
    assert_eq!((q.yes[0], q.no[0], q.no_total), (0, 10 * TOKEN, 10 * TOKEN));
    assert_eq!(q.redeem(&[0; 3], &[0; 3]).unwrap_err(), ZeroAmount, "and a second time there is nothing left");
}

#[test]
fn redeem_burns_every_winning_token_of_the_holder() {
    // answer 2 wins: YES on 2, NO on 0 and NO on 1 all pay
    let mut q = opened(0, 500 * TOKEN);
    q.buy(2, true, 7 * TOKEN, 0).unwrap();
    q.buy(0, false, 5 * TOKEN, 0).unwrap();
    q.buy(1, false, 3 * TOKEN, 0).unwrap();
    q.buy(2, false, 2 * TOKEN, 0).unwrap();
    q.buy(0, true, 4 * TOKEN, 0).unwrap();
    q.resolve(2, false, RESOLVE).unwrap();
    let (burned, paid) = q.redeem(&[4 * TOKEN, 0, 7 * TOKEN], &[5 * TOKEN, 3 * TOKEN, 2 * TOKEN]).unwrap();
    assert_eq!(burned, vec![(0, false, 5 * TOKEN), (1, false, 3 * TOKEN), (2, true, 7 * TOKEN)]);
    assert_eq!(paid, 15 * TOKEN);
    assert_eq!(q.winning_supply(2), 0);
    assert_eq!((q.yes[0], q.yes[2], q.no[0], q.no[1], q.no[2], q.no_total), (4 * TOKEN, 0, 0, 0, 2 * TOKEN, 2 * TOKEN));
}

// ------------------------------------------------------------------------------------------------ the exponent guard

#[test]
fn a_term_41_depths_behind_or_more_counts_as_zero() {
    let b: i128 = 1_000_000;
    let pool = Pool::open(b as u128, &[WAD / 4; 4]);
    let book = [0, -41 * b, -41 * b - 1, -100 * b];
    assert_eq!(pool.prices_at(&book), vec![WAD, 0, 0, 0]);
    assert_eq!(pool.cost_at(&book), 0);
    // a level pair above such terms is a price of exactly 1/2 and C is exactly b ln 2 above the leaders
    let book = [7, 7, 7 - 41 * b, 7 - 41 * b];
    assert_eq!(pool.prices_at(&book), vec![WAD / 2, WAD / 2, 0, 0]);
    assert_eq!(pool.cost_at(&book), 7 + 693_147);
}

#[test]
fn a_term_just_inside_41_depths_still_counts() {
    // exp(-40.5) is 2.6e-18, so the term is 2 wei and the price floor(2e18 / (1e18 + 2)) is 1 wei
    let b: i128 = 1_000_000;
    let pool = Pool::open(b as u128, &[WAD / 4; 4]);
    assert_eq!(pool.prices_at(&[0, -405 * b / 10, -41 * b, -41 * b])[1], 1);
    // exp(-41 + 1e-6) is 1.6e-18, 1 wei, and the price floors to 0, but at a large depth C sees it: fifteen such
    // terms add 15 wei to the sum, and ln(1 + 15e-18) is about 14 wei, or 14e6 units at depth 1e24
    let big: i128 = 1_000_000_000_000_000_000_000_000;
    let pool = Pool::open(big as u128, &[WAD / 16; 16]);
    let mut book = [-(big * 41 - big / 1_000_000); 16];
    book[0] = 0;
    let c = pool.cost_at(&book);
    assert!((13_000_000..=16_000_000).contains(&c), "C {c}");
}

#[test]
fn a_spread_far_past_41_depths_is_only_a_price_of_zero() {
    // the division in `term` used to overflow past about 1.7e20 depths, and these all panicked
    let mut q = Question::new(3, 0, CLOSE, RESOLVE, FALLBACK, 0).unwrap();
    q.open(1, 0, &P, 0).unwrap();
    q.buy(2, true, 141_070_387_603_912_779_016, 0).unwrap();
    q.buy(2, false, 285_454_153_162_153_937_122, 0).unwrap();
    assert!(q.quote_sell(2, true, 35_267_596_900_978_194_754, 0).is_ok());
    q.sell(2, true, 35_267_596_900_978_194_754, 0).unwrap();

    let mut q = Question::new(3, 0, CLOSE, RESOLVE, FALLBACK, 0).unwrap();
    q.open(1, 0, &P, 0).unwrap();
    assert!(q.quote_buy(0, true, MAX_TICKETS, 0).is_ok());
    q.buy(0, true, MAX_TICKETS, 0).unwrap();
    assert_eq!(q.prices().unwrap(), vec![WAD, 0, 0]);
    assert!(q.information_interval(1).is_ok());
    q.sell(0, true, MAX_TICKETS, 0).unwrap();
    assert_eq!(q.pool.as_ref().unwrap().q, q.pool.as_ref().unwrap().q0);

    // the README market at 6 decimals: the book level again, with 2^100 YES on answer 0 in one holder's hands
    let mut q = Question::new(5, 0, CLOSE, RESOLVE, FALLBACK, 0).unwrap();
    q.open(7_130 * TOKEN, 5_000 * TOKEN, &[38 * WAD / 100, 22 * WAD / 100, 18 * WAD / 100, 19 * WAD / 100, 3 * WAD / 100], 0).unwrap();
    for _ in 0..2 {
        q.buy(0, true, 1 << 99, 0).unwrap();
        q.buy(0, false, 1 << 99, 0).unwrap();
    }
    q.sell(0, true, 1 << 100, 0).unwrap();
    assert_eq!(q.yes[0], 0);
}

#[test]
fn the_payout_stays_exact_on_the_largest_book() {
    // the rate is a 256-bit product at every step here: pot and supply both near 2^100
    let mut q = Question::new(3, MAX_FEE_RATE, CLOSE, RESOLVE, FALLBACK, 0).unwrap();
    q.open(MAX_DEPTH, 0, &P, 0).unwrap();
    q.buy(1, true, MAX_TICKETS, 0).unwrap();
    let f = q.buy(2, true, MAX_TICKETS, 0).unwrap();
    assert_eq!(f.fee, mul_div_up(f.amount, MAX_FEE_RATE, WAD));
    let r = q.payout_if(1).unwrap();
    assert_eq!(r, mul_div(q.pot, WAD, MAX_TICKETS));
    assert!(r < WAD);
    let lost = q.resolve(1, false, RESOLVE).unwrap();
    assert_eq!(lost, r);
    assert_eq!(q.pot, mul_div_up(MAX_TICKETS, r, WAD));
}

// ------------------------------------------------------------------------------------------------ gaps mutants found

#[test]
fn prices_whose_sum_wraps_round_to_exactly_one_are_refused() {
    // 1e18 + 2^127 + 2^127 is 1e18 modulo 2^128
    assert_eq!([WAD, 1 << 127, 1 << 127].iter().fold(0u128, |a, &p| a.wrapping_add(p)), WAD);
    let mut q = fresh();
    assert_eq!(q.open(DEPTH, 0, &[WAD, 1 << 127, 1 << 127], 0), Err(BadPrices));
    assert_eq!(q, fresh());
}

#[test]
fn sixteen_prices_too_large_to_add_up_are_refused_as_bad_prices() {
    let mut q = Question::new(MAX_RANGES, 0, CLOSE, RESOLVE, FALLBACK, 0).unwrap();
    assert_eq!(q.open(DEPTH, 0, &[1 << 124; MAX_RANGES], 0), Err(BadPrices));
    assert!(q.pool.is_none());
}

#[test]
fn the_largest_number_of_tokens_is_refused_and_overflows_nothing() {
    trade_refused(&traded(), 0, u128::MAX, 0, TooManyTickets);
}

/// `check_trade` comes first, so a closed market says it is closed, however full it is.
#[test]
fn a_full_market_at_the_close_says_it_is_closed() {
    let mut q = Question::new(3, 0, CLOSE, RESOLVE, FALLBACK, 0).unwrap();
    q.open(MAX_DEPTH, 0, &P, 0).unwrap();
    q.buy(0, true, MAX_TICKETS, 0).unwrap();
    let mut c = q.clone();
    assert_eq!(c.buy(0, true, 1, CLOSE), Err(TradingClosed));
    assert_eq!(c.quote_buy(0, true, 1, CLOSE).unwrap_err(), TradingClosed);
    assert_eq!(c, q);
}

/// The supply is checked before the curve: on this book a NO sale on 2 would raise the computed C, and nobody holds one.
#[test]
fn a_sale_of_tokens_that_do_not_exist_says_so_even_where_the_curve_would_refuse_it() {
    let mut q = Question::new(3, 0, CLOSE, RESOLVE, FALLBACK, 0).unwrap();
    q.open(1_000_000_000_000_000_000_000_000, 1 << 90, &[5 * WAD / 10, 3 * WAD / 10, 2 * WAD / 10], 0).unwrap();
    q.buy(0, true, 2_000_000, 0).unwrap();
    assert!(q.pool.as_ref().unwrap().sell_fall(2, false, 1) < 0);
    assert_eq!(q.quote_sell(2, false, 1, 0).unwrap_err(), SupplyShort);
    assert_eq!(q.sell(2, false, 1, 0), Err(SupplyShort));
}

#[test]
fn clearing_reaches_every_answer_whichever_one_won() {
    for w in 0..3 {
        let mut q = Question::new(3, 0, 10, 10, 10, 0).unwrap();
        q.open(1_000_000, 1_000_000, &[WAD / 2, WAD / 4, WAD / 4], 0).unwrap();
        for i in 0..3 {
            q.buy(i, true, 10 + i as u128, 0).unwrap();
            q.buy(i, false, 20 + i as u128, 0).unwrap();
        }
        q.resolve(w, false, 10).unwrap();
        let (yes, no, pot, surplus) = (q.yes.clone(), q.no.clone(), q.pot, q.surplus);
        q.clear_losing(&yes, &no).unwrap();
        for i in 0..3 {
            assert_eq!((q.yes[i], q.no[i]), if i == w { (yes[i], 0) } else { (0, no[i]) }, "answer {i}, winner {w}");
        }
        assert_eq!(q.no_total, no.iter().sum::<u128>() - no[w]);
        assert_eq!((q.pot, q.surplus), (pot, surplus));
    }
}
