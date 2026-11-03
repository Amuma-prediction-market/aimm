//! The claims of SPEC.md one at a time, at values worked by hand. Where a number is written out it was worked outside
//! this code, in 100-digit arithmetic, so a test here cannot pass because the code agrees with itself.
mod common;
use aimm_core::lmsr::Pool;
use aimm_core::math::{mul_div, mul_div_up, WAD};
use aimm_core::question::AimmError::*;
use aimm_core::question::{Question, MIN_OPENING_PRICE};
use common::{noise, random_prices, Rng};

const TOKEN: u128 = 1_000_000;
const EVEN: [u128; 3] = [WAD - 2 * (WAD / 3), WAD / 3, WAD / 3];
const DEPTHS: [u128; 7] = [
    1_000_000,
    1_000_000_000,
    1_000_000_000_000_000,
    1_000_000_000_000_000_000,
    1_000_000_000_000_000_000_000,
    1_000_000_000_000_000_000_000_000,
    1 << 100,
];

fn question(prices: &[u128], depth: u128, fund: u128, fee: u128) -> Question {
    let mut q = Question::new(prices.len(), fee, 100, 200, 300, 0).unwrap();
    q.open(depth, fund, prices, 0).unwrap();
    q
}

/// Four answers at even prices, so that every price is exactly 1/4, with the pot and the supply set by hand. The
/// interval depends only on those, and trades could not reach numbers this round.
fn even_book(pot: u128, yes: [u128; 4], no: [u128; 4]) -> Question {
    let mut q = question(&[WAD / 4; 4], 1_000_000, 0, 0);
    q.pot = pot;
    q.yes = yes.to_vec();
    q.no = no.to_vec();
    q.no_total = no.iter().sum();
    q
}

fn c(q: &Question) -> i128 {
    let p = q.pool.as_ref().unwrap();
    p.cost_at(&p.q)
}

fn near(got: u128, want: u128, within: u128, what: &str) {
    assert!(got.abs_diff(want) <= within, "{what}: got {got}, want {want}");
}

// ------------------------------------------------------------------------------------------------ SPEC section 1: the curve

#[test]
fn l_is_the_depth_times_ln_of_the_smallest_price() {
    // 1e9 ln 5 = 1609437912.4 and 1e9 ln 1e6 = 13815510557.9
    let l = |p: &[u128]| question(p, 1_000_000_000, 0, 0).virtual_liquidity().unwrap();
    assert_eq!(l(&[WAD / 2, 3 * WAD / 10, WAD / 5]), 1_609_437_912);
    assert_eq!(l(&[WAD / 5, 3 * WAD / 10, WAD / 2]), 1_609_437_912);
    assert_eq!(l(&[3 * WAD / 10, WAD / 5, WAD / 2]), 1_609_437_912);
    assert_eq!(l(&[MIN_OPENING_PRICE, MIN_OPENING_PRICE, WAD - 2 * MIN_OPENING_PRICE]), 13_815_510_557);
    assert_eq!(l(&[WAD / 4; 4]), 1_386_294_361);
}

#[test]
fn the_curve_opens_at_zero_at_the_opening_prices() {
    let mut r = Rng(0x0fe2_1b0a_7d3c_9e41);
    for &depth in &DEPTHS[..3] {
        for _ in 0..100 {
            let n = 3 + r.below(14);
            let p = random_prices(&mut r, n);
            let q = question(&p, depth, 0, 0);
            assert_eq!(c(&q), 0);
            // the opening book is whole units, so each price can sit about one part in `depth` off
            for (got, want) in q.prices().unwrap().into_iter().zip(p) {
                near(got, want, 4 * WAD / depth + 16, "opening price");
            }
        }
    }
}

#[test]
fn costs_and_prices_at_depth_1e9_worked_to_the_unit() {
    // C(q) = 1e9 ln(sum exp(q_i / 1e9)) from .5 .3 .2, the amounts rounded as SPEC section 1 says, the fee at a tenth
    let mut q = question(&[WAD / 2, 3 * WAD / 10, WAD / 5], 1_000_000_000, 0, WAD / 10);

    let f = q.buy(0, true, 1_000_000_000, 0).unwrap();
    assert_eq!((f.amount, f.fee), (620_114_508, 62_011_451));
    for (i, want) in [731_058_578_667_507_340, 161_364_852_792_514_024, 107_576_568_539_978_635].into_iter().enumerate() {
        near(q.prices().unwrap()[i], want, 100, "price after the first buy");
    }

    let f = q.buy(0, false, 500_000_000, 0).unwrap();
    assert_eq!((f.amount, f.fee), (160_815_298, 16_081_530));
    for (i, want) in [622_459_331_246_680_010, 226_524_401_242_191_245, 151_016_267_511_128_744].into_iter().enumerate() {
        near(q.prices().unwrap()[i], want, 100, "price after the NO");
    }

    let f = q.sell(0, true, 400_000_000, 0).unwrap();
    assert_eq!((f.amount, f.fee), (229_680_324, 22_968_033));
    for (i, want) in [524_979_187_526_506_860, 285_012_487_471_764_603, 190_008_325_001_728_535].into_iter().enumerate() {
        near(q.prices().unwrap()[i], want, 100, "price after the sale");
    }
    assert_eq!(q.pot, 551_249_482);
    assert_eq!((q.fees_paid, q.fees_owed), (101_061_014, 101_061_014));
}

#[test]
fn a_yes_moves_its_own_answer_and_a_no_every_other() {
    let mut q = question(&[WAD / 2, 3 * WAD / 10, WAD / 5], 1_000_000, 0, 0);
    let q0 = q.pool.as_ref().unwrap().q0.clone();
    q.buy(1, true, 700, 0).unwrap();
    assert_eq!(q.pool.as_ref().unwrap().q, vec![q0[0], q0[1] + 700, q0[2]]);
    q.buy(1, false, 40, 0).unwrap();
    assert_eq!(q.pool.as_ref().unwrap().q, vec![q0[0] + 40, q0[1] + 700, q0[2] + 40]);
    q.sell(1, true, 200, 0).unwrap();
    assert_eq!(q.pool.as_ref().unwrap().q, vec![q0[0] + 40, q0[1] + 500, q0[2] + 40]);
    q.sell(1, false, 5, 0).unwrap();
    assert_eq!(q.pool.as_ref().unwrap().q, vec![q0[0] + 35, q0[1] + 500, q0[2] + 35]);
    assert_eq!((q.yes.clone(), q.no.clone(), q.no_total), (vec![0, 500, 0], vec![0, 35, 0], 35));
    for w in 0..3 {
        assert_eq!(q.winning_supply(w), q.pool.as_ref().unwrap().winning(w));
    }
}

#[test]
fn the_same_tokens_on_every_answer_cost_that_many_units() {
    // C(q + n) = C(q) + n exactly, at any depth, because the sum in the logarithm does not move
    let mut r = Rng(0x2c1b_87aa_55f0_3dd9);
    for &depth in &DEPTHS {
        for _ in 0..30 {
            let n = 3 + r.below(14);
            let p = random_prices(&mut r, n);
            let mut q = question(&p, depth, 0, 0);
            let set = r.log_between(depth / 1000, depth);
            let before = q.prices().unwrap();
            let paid: u128 = (0..n).map(|i| q.buy(i, true, set, 0).unwrap().amount).sum();
            assert_eq!(c(&q), set as i128 + c(&question(&p, depth, 0, 0)));
            assert_eq!(q.prices().unwrap(), before);
            assert!(paid >= set + n as u128 && paid <= set + 3 * n as u128 + noise(depth), "paid {paid} for a set of {set}");
        }
    }
}

// ------------------------------------------------------------------------------------------------ SPEC section 1: rounding

#[test]
fn a_buy_costs_exactly_the_rise_in_c_plus_one_and_a_sale_refunds_exactly_the_fall_less_one() {
    let mut r = Rng(0x64a9_d3e1_90bc_7713);
    let (mut bought, mut sold) = (0, 0);
    for &depth in &DEPTHS {
        for _ in 0..40 {
            let n = 3 + r.below(6);
            let p = random_prices(&mut r, n);
            // a fund above L, so that no cap can bind and the curve's amount is what is paid
            let mut q = question(&p, depth, 2 * depth * 14, 0);
            let mut held: Vec<(usize, bool, u128)> = vec![];
            for _ in 0..12 {
                let pot = q.pot;
                let before = c(&q);
                let pool = q.pool.clone().unwrap();
                if !held.is_empty() && r.unit() < 0.45 {
                    let k = r.below(held.len());
                    let (i, yes, h) = held[k];
                    let s = r.log_between(1, h);
                    let fall = before - pool.cost_at(&pool.book_after(i, yes, -(s as i128)));
                    match q.sell(i, yes, s, 0) {
                        Ok(f) => {
                            assert!(fall >= 0);
                            assert_eq!(f.amount as i128, (fall - 1).max(0).min(s as i128), "fall {fall}, {s} tokens");
                            assert_eq!(q.pot, pot - f.amount);
                            held[k].2 -= s;
                            if held[k].2 == 0 {
                                held.swap_remove(k);
                            }
                            sold += (f.amount > 0) as u32;
                        }
                        Err(e) => assert_eq!((e, fall < 0), (SaleTooSmall, true)),
                    }
                } else {
                    let (i, yes, t) = (r.below(n), r.unit() < 0.5, r.log_between(1, depth));
                    let rise = pool.cost_at(&pool.book_after(i, yes, t as i128)) - before;
                    let f = q.buy(i, yes, t, 0).unwrap();
                    assert_eq!(f.amount as i128, rise.max(0) + 1, "rise {rise}");
                    assert_eq!(q.pot, pot + f.amount);
                    held.push((i, yes, t));
                    bought += 1;
                }
            }
        }
    }
    assert!(bought > 1000 && sold > 500, "{bought} buys and {sold} sales");
}

#[test]
fn a_token_is_never_free() {
    // at a price of a tenth of a cent and a depth of 1000 dollars, one token's rise in C is a thousandth of a unit
    let mut q = question(&[WAD / 1000, WAD / 2, WAD / 2 - WAD / 1000], 1_000_000_000, 0, 0);
    for k in 1..=1000 {
        assert!(q.buy(0, true, 1, 0).unwrap().amount >= 1);
        assert!(q.pot >= k);
    }
}

#[test]
fn a_negative_rise_still_costs_one_unit() {
    // at depth 1e24 one wei of ln is a million units: this is the book one token below the sale of F17, so the token
    // that takes it there makes computed C fall by 1,999,999, and the buyer still pays a unit
    let mut q = question(&[5 * WAD / 10, 3 * WAD / 10, 2 * WAD / 10], 1_000_000_000_000_000_000_000_000, 0, 0);
    q.buy(0, true, 1_999_999, 0).unwrap();
    let pool = q.pool.as_ref().unwrap();
    assert_eq!(pool.cost_at(&pool.book_after(0, true, 1)) - pool.cost_at(&pool.q), -1_999_999);
    let pot = q.pot;
    assert_eq!(q.buy(0, true, 1, 0).unwrap().amount, 1);
    assert_eq!(q.pot, pot + 1);
}

#[test]
fn a_sale_too_small_to_move_the_curve_refunds_nothing() {
    let mut q = question(&[WAD / 1000, WAD / 2, WAD / 2 - WAD / 1000], 1_000_000_000, 5_000_000_000, 0);
    q.buy(0, true, 1000, 0).unwrap();
    let mut seen = [0, 0];
    for _ in 0..1000 {
        let fall = q.pool.as_ref().unwrap().sell_fall(0, true, 1);
        let f = q.sell(0, true, 1, 0).unwrap();
        assert!(fall == 0 || fall == 1, "fall {fall}");
        assert_eq!(f.amount, 0);
        seen[fall as usize] += 1;
    }
    assert!(seen[0] > 0 && seen[1] > 0, "falls of nothing and of one unit both happen: {seen:?}");
}

// ------------------------------------------------------------------------------------------------ SPEC section 2: the sell cap

#[test]
fn a_yes_sale_is_capped_at_the_payout_on_its_answer() {
    // . Three even answers, depth 1000 dollars, fund 300, 3000 YES on 0: pot 2296.310669, price .9094
    let mut q = question(&EVEN, 1_000_000_000, 300 * TOKEN, 0);
    q.buy(0, true, 3000 * TOKEN, 0).unwrap();
    assert_eq!(q.pot, 2_296_310_669);
    assert_eq!(q.payout_if(0), Ok(765_436_889_666_666_666));
    let cost = q.buy(0, true, 100 * TOKEN, 0).unwrap().amount;
    assert_eq!(cost, 91_345_016);
    assert_eq!(q.payout_if(0), Ok(770_211_511_290_322_580));
    let curve = q.pool.as_ref().unwrap().sell_refund(0, true, 100 * TOKEN);
    assert_eq!(curve, 91_345_014);
    // the refund is floor(100e6 x .770211511290322580), not the curve's amount, and the book moves back in full
    let book = q.pool.as_ref().unwrap().book_after(0, true, -((100 * TOKEN) as i128));
    assert_eq!(q.sell(0, true, 100 * TOKEN, 0).unwrap().amount, 77_021_151);
    assert_eq!(q.pool.as_ref().unwrap().q, book);
    assert_eq!(q.pot, 2_310_634_534);
}

#[test]
fn a_no_sale_is_capped_at_the_least_payout_among_the_answers_it_pays_on() {
    // NO on 0 pays on answers 1 and 2, whose payouts are .9529 and .9265, and it loses on answer 0, whose payout is 1.
    // The curve would refund 39.709423 for 40 tokens, and the cap is 40 tokens at .9265
    let mut q = question(&[WAD / 5, 2 * WAD / 5, 2 * WAD / 5], 1_000_000_000, 0, 0);
    q.buy(0, false, 100 * TOKEN, 0).unwrap();
    q.buy(1, true, 3400 * TOKEN, 0).unwrap();
    q.buy(2, true, 3500 * TOKEN, 0).unwrap();
    assert_eq!(q.payout_if(0), Ok(WAD));
    assert_eq!(q.payout_if(1), Ok(952_929_293_428_571_428));
    assert_eq!(q.payout_if(2), Ok(926_459_035_277_777_777));
    assert_eq!(q.pool.as_ref().unwrap().sell_refund(0, false, 40 * TOKEN), 39_709_423);
    assert_eq!(q.sell(0, false, 40 * TOKEN, 0).unwrap().amount, 37_058_361);
}

#[test]
fn a_no_sale_is_not_capped_by_the_answer_it_loses_on() {
    // NO on 0 pays on 1 and 2, which are not short. Answer 0 is, at payout .1291, and that must not limit the sale
    let mut q = question(&[WAD / 10, 45 * WAD / 100, 45 * WAD / 100], 1_000_000_000, 0, 0);
    q.buy(0, false, 5 * TOKEN, 0).unwrap();
    q.buy(0, true, 300 * TOKEN, 0).unwrap();
    assert_eq!(q.payout_if(0), Ok(129_123_626_666_666_666));
    assert_eq!((q.payout_if(1), q.payout_if(2)), (Ok(WAD), Ok(WAD)));
    assert_eq!(q.sell(0, false, 5 * TOKEN, 0).unwrap().amount, 4_349_300);
}

#[test]
fn a_sale_can_lower_the_payout_on_an_answer_it_does_not_pay_on() {
    // SPEC section 2: the refund leaves the pot, and the pot backs every answer
    let mut q = question(&[WAD / 2, 3 * WAD / 10, WAD / 5], 1_000_000_000, 0, 0);
    q.buy(0, true, 1000 * TOKEN, 0).unwrap();
    q.buy(1, true, 4000 * TOKEN, 0).unwrap();
    assert_eq!(q.payout_if(1), Ok(721_738_508_250_000_000));
    q.sell(0, true, 1000 * TOKEN, 0).unwrap();
    assert_eq!(q.payout_if(1), Ok(709_468_924_250_000_000));
}

#[test]
fn the_sell_cap_holds_per_sale_only() {
    // in whole tokens: fund 0, depth 10, even prices, and Mallory holds 100 YES on 0 and Alice 50
    let start = || {
        let mut q = question(&EVEN, 10 * TOKEN, 0, 0);
        assert_eq!(q.buy(0, true, 150 * TOKEN, 0).unwrap().amount, 139_013_885);
        q
    };
    assert_eq!(start().payout_if(0), Ok(926_759_233_333_333_333));

    // a plain sale pays her the cap, and Alice's payout does not fall
    let mut plain = start();
    assert_eq!(plain.sell(0, true, 100 * TOKEN, 0).unwrap().amount, 92_675_923);
    assert_eq!(plain.payout_if(0), Ok(926_759_240_000_000_000));

    // first she buys 50 YES on 1 for 452 units, then sells both. Each sale obeys the cap and guarantee 3...
    let mut lifted = start();
    assert_eq!(lifted.buy(1, true, 50 * TOKEN, 0).unwrap().amount, 452);
    assert_eq!(lifted.payout_if(0), Ok(926_762_246_666_666_666));
    assert_eq!(lifted.sell(0, true, 100 * TOKEN, 0).unwrap().amount, 92_676_224);
    assert_eq!(lifted.payout_if(0), Ok(926_762_260_000_000_000));
    assert_eq!(lifted.payout_if(1), Ok(926_762_260_000_000_000));
    assert_eq!(lifted.sell(1, true, 50 * TOKEN, 0).unwrap().amount, 6_831_244);
    assert_eq!(lifted.payout_if(1), Ok(WAD));

    // ...but over the three trades she takes 99.51 where the cap meant 92.68, and Alice's payout falls to .7901
    assert_eq!(92_676_224 + 6_831_244 - 452, 99_507_016);
    assert_eq!(lifted.payout_if(0), Ok(790_137_380_000_000_000));
}

// ------------------------------------------------------------------------------------------------ SPEC section 2: the floor

/// Trades at random for `steps` steps and calls `check` after each, with the number of purchases so far.
fn random_trading(q: &mut Question, r: &mut Rng, depth: u128, steps: usize, mut check: impl FnMut(&Question, u128)) {
    let n = q.n();
    let mut held: Vec<(usize, bool, u128)> = vec![];
    let mut buys = 0;
    for _ in 0..steps {
        if !held.is_empty() && r.unit() < 0.4 {
            let k = r.below(held.len());
            let (i, yes, h) = held[k];
            let s = if r.unit() < 0.5 { h } else { r.log_between(1, h) };
            if q.sell(i, yes, s, 0).is_ok() {
                held[k].2 -= s;
                if held[k].2 == 0 {
                    held.swap_remove(k);
                }
            }
        } else {
            let (i, yes, t) = (r.below(n), r.unit() < 0.6, r.log_between(1, 3 * depth));
            if q.buy(i, yes, t, 0).is_ok() {
                held.push((i, yes, t));
                buys += 1;
            }
        }
        check(q, buys);
    }
}

#[test]
fn after_any_sequence_the_pot_holds_the_fund_plus_the_rise_in_c() {
    // a buy adds at least the rise plus one, and a sale takes out at most the fall, so the excess can only grow
    let mut r = Rng(0x3b7f_11c2_a5d0_8e66);
    for &depth in &DEPTHS {
        for _ in 0..30 {
            let n = 3 + r.below(14);
            let p = random_prices(&mut r, n);
            let l = question(&p, depth, 0, 0).virtual_liquidity().unwrap();
            let fund = mul_div(l, r.below(1001) as u128, 2000);
            let mut q = question(&p, depth, fund, (r.below(3) as u128) * WAD / 40);
            let c0 = c(&q);
            assert!(c0 <= 0, "computed C opens at {c0}");
            random_trading(&mut q, &mut r, depth.min(1 << 98), 20, |q, buys| {
                let c = c(q);
                assert!(q.pot as i128 >= fund as i128 + c - c0 + buys as i128, "pot {} below fund {fund} + C {c} - {c0} + {buys}", q.pot);
                assert!(q.pot as i128 >= fund as i128 + c, "pot {} below fund {fund} + C {c}", q.pot);
            });
        }
    }
}

#[test]
fn every_payout_stays_at_or_above_the_opening_price() {
    // pot >= C(q) >= sum p0_i W_i >= p0_w W_w with exact C. Random books at these depths never see the noise in computed
    // C, even with no fund to help; books chosen for it can (the next test)
    let mut r = Rng(0x71ae_02f5_c944_6b1d);
    for &depth in &[1_000, 1_000_000, 1_000_000_000, 1_000_000_000_000, 1_000_000_000_000_000u128] {
        for _ in 0..300 {
            let n = 3 + r.below(14);
            let p = random_prices(&mut r, n);
            let mut q = question(&p, depth, 0, 0);
            random_trading(&mut q, &mut r, depth, 16, |q, _| {
                for w in 0..n {
                    assert!(q.payout_if(w).unwrap() >= p[w], "payout on {w} below its opening price, depth {depth}");
                }
            });
        }
    }
}

#[test]
fn a_book_chosen_for_it_leaves_a_dust_payout_a_wei_under_the_opening_price_at_an_ordinary_depth() {
    let p = [500_000_000_000_000_001, 499_998_999_999_547_012, 1_000_000_452_987];
    let mut q = question(&p, 999_999_047_012, 0, 0);
    assert_eq!(q.buy(0, true, 2, 0).unwrap().amount, 1);
    assert_eq!(q.payout_if(0).unwrap(), WAD / 2);
    assert!(q.payout_if(0).unwrap() < p[0]);
}

#[test]
fn above_depth_1e15_the_payout_can_miss_the_opening_price_by_the_noise_in_c_and_no_more() {
    let mut r = Rng(0x19c3_e8d7_b226_f05b);
    let mut under = 0;
    for &depth in &DEPTHS[3..] {
        for _ in 0..400 {
            let n = 3 + r.below(5);
            let p = random_prices(&mut r, n);
            let mut q = question(&p, depth, 0, 0);
            let (i, yes, t) = (r.below(n), r.unit() < 0.8, r.log_between(depth / WAD, depth / 1000));
            q.buy(i, yes, t, 0).unwrap();
            for w in 0..n {
                let owed = q.winning_supply(w);
                let floor = mul_div_up(owed, p[w], WAD);
                under += (q.pot < floor) as u32;
                assert!(q.pot + noise(depth) >= floor, "pot {} on {owed} tokens at {}, depth {depth}", q.pot, p[w]);
            }
        }
    }
    assert!(under > 0, "the bound was never tested: every pot met the floor outright");
}

#[test]
fn a_fund_of_b_ln_1_over_p0_i_keeps_answer_i_whole() {
    // C(q) >= q_i = q0_i + W_i, so a fund of -q0_i covers W_i whatever else happens
    let mut r = Rng(0x48b1_c7f2_3e90_a6d5);
    for &depth in &DEPTHS {
        for _ in 0..30 {
            let n = 3 + r.below(14);
            let p = random_prices(&mut r, n);
            let i = r.below(n);
            let mut q = question(&p, depth, 0, 0);
            let fund = (-q.pool.as_ref().unwrap().q0[i]) as u128;
            q.fund = fund;
            q.pot = fund;
            random_trading(&mut q, &mut r, depth.min(1 << 98), 20, |q, _| {
                assert_eq!(q.payout_if(i), Ok(WAD));
            });
        }
    }
}

// ------------------------------------------------------------------------------------------------ SPEC section 2: payout and settlement

#[test]
fn a_short_pot_is_shared_at_one_rate_rounded_against_the_holder() {
    // pot 1000 on 3000 winning tokens: r is 1/3 rounded down, ceil(3000 r) is the whole pot, three holders of 1000 are
    // paid 333 each and one unit stays behind
    let mut q = even_book(1000, [3000, 0, 0, 0], [0; 4]);
    assert_eq!(q.resolve(0, false, 200), Ok(333_333_333_333_333_333));
    assert_eq!((q.pot, q.surplus, q.payout), (1000, 0, 333_333_333_333_333_333));
    for k in 1..=3 {
        assert_eq!(q.redeem(&[1000, 0, 0, 0], &[0; 4]).unwrap().1, 333);
        assert_eq!(q.pot, 1000 - 333 * k);
    }
    assert_eq!(q.pot, 1);
}

#[test]
fn what_the_winners_are_owed_is_kept_rounded_up() {
    // pot 5 on 7 tokens: r = .714285714285714285, 7 r = 4.999999999999999995, so all 5 are kept: rounding down would
    // keep 4 and hand a unit of the winners' money to the operator
    let mut q = even_book(5, [7, 0, 0, 0], [0; 4]);
    assert_eq!(q.resolve(0, false, 200), Ok(714_285_714_285_714_285));
    assert_eq!((q.pot, q.surplus), (5, 0));
    // pot 1000 and 2999 tokens: r = .333444481493831277, 2999 r = 999.99999999999999.., so again the pot
    let mut q = even_book(1000, [2999, 0, 0, 0], [0; 4]);
    assert_eq!(q.resolve(0, false, 200), Ok(333_444_481_493_831_277));
    assert_eq!((q.pot, q.surplus), (1000, 0));
}

#[test]
fn a_full_pot_pays_one_and_the_rest_is_the_operators() {
    let mut q = even_book(5000, [3000, 0, 0, 0], [0; 4]);
    assert_eq!(q.resolve(0, false, 200), Ok(WAD));
    assert_eq!((q.pot, q.surplus), (3000, 2000));
    assert_eq!(q.redeem(&[3000, 0, 0, 0], &[0; 4]).unwrap().1, 3000);
    assert_eq!(q.claim_surplus(), Ok(2000));
    assert_eq!(q.pot, 0);
}

#[test]
fn an_answer_nobody_holds_sends_the_whole_pot_to_the_operator() {
    let mut q = even_book(700, [3000, 0, 0, 0], [0; 4]);
    assert_eq!(q.resolve(1, false, 200), Ok(WAD));
    assert_eq!((q.pot, q.surplus), (0, 700));
}

#[test]
fn winning_tokens_are_paid_the_same_whenever_they_were_bought() {
    // Proposition 1: the first buyer paid .34 and the last .90, and both are paid the same rate
    let mut q = question(&EVEN, 1_000_000_000, 100 * TOKEN, 0);
    let early = q.buy(0, true, 100 * TOKEN, 0).unwrap().amount;
    q.buy(0, true, 2800 * TOKEN, 0).unwrap();
    let late = q.buy(0, true, 100 * TOKEN, 0).unwrap().amount;
    assert!(late > 2 * early, "{early} and {late}");
    let r = q.resolve(0, false, 200).unwrap();
    assert!(r < WAD);
    let want = mul_div(100 * TOKEN, r, WAD);
    assert_eq!(q.redeem(&[100 * TOKEN, 0, 0], &[0; 3]).unwrap().1, want);
    assert_eq!(q.redeem(&[100 * TOKEN, 0, 0], &[0; 3]).unwrap().1, want);
}

#[test]
fn the_fee_stays_out_of_the_pot() {
    let mut q = question(&EVEN, 1_000_000_000, 300 * TOKEN, WAD / 10);
    let a = q.buy(0, true, 100 * TOKEN, 0).unwrap();
    assert_eq!(q.pot, 300 * TOKEN + a.amount);
    let b = q.sell(0, true, 40 * TOKEN, 0).unwrap();
    assert_eq!(q.pot, 300 * TOKEN + a.amount - b.amount);
    assert_eq!(q.fees_paid, a.fee + b.fee);
}

// ------------------------------------------------------------------------------------------------ SPEC section 3: the interval

#[test]
fn the_interval_on_a_pot_short_on_yes() {
    // pot 600 on 1200, 750, 640 and 600 YES: payouts .5, .8, .9375 and 1, at a price of exactly .25
    let q = even_book(600, [1200, 750, 640, 600], [0; 4]);
    assert_eq!(q.prices().unwrap(), vec![WAD / 4; 4]);
    assert_eq!(q.information_interval(0), Ok((62_500_000_000_000_000, WAD / 2)), "1 - .75 / .8 and .25 / .5");
    assert_eq!(q.information_interval(1), Ok((0, 312_500_000_000_000_000)), "1 - .75 / .5 < 0 and .25 / .8");
    assert_eq!(q.information_interval(2), Ok((0, 266_666_666_666_666_667)), "1 - .75 / .5 < 0 and .25 / .9375");
    assert_eq!(q.information_interval(3), Ok((0, WAD / 4)), "1 - .75 / .5 < 0 and .25 / 1");
}

#[test]
fn the_interval_on_a_pot_short_on_no() {
    // NO on 0, 1, 2 and 3 of 130, 180, 290 and 330 make winning supplies 800, 750, 640 and 600: on a pot of 600 the
    // payouts are .75, .8, .9375 and 1
    let q = even_book(600, [0; 4], [130, 180, 290, 330]);
    assert_eq!(q.information_interval(0), Ok((62_500_000_000_000_000, 333_333_333_333_333_334)), "r_no is .8");
    assert_eq!(q.information_interval(1), Ok((0, 312_500_000_000_000_000)), "r_no is .75");
    assert_eq!(q.information_interval(2), Ok((0, 266_666_666_666_666_667)));
    assert_eq!(q.information_interval(3), Ok((0, WAD / 4)));
}

#[test]
fn the_lower_end_takes_the_worst_answer_a_no_could_pay_on() {
    // for answer 0 the payouts on what a NO pays are .9375, .8 and 1, wherever in the list the worst one stands: the best
    // of them would give a lower end of .25 and the second best .2
    for (a, b, d) in [(640, 750, 600), (750, 640, 600), (600, 640, 750), (640, 600, 750)] {
        let q = even_book(600, [1200, a, b, d], [0; 4]);
        assert_eq!(q.information_interval(0).unwrap().0, 62_500_000_000_000_000, "{a} {b} {d}");
    }
    // and the answer's own payout, .5, is not among them, or the lower end would be 0
}

#[test]
fn the_interval_with_an_empty_pot() {
    // nothing to pay anyone: the price says nothing
    let q = even_book(0, [10, 20, 30, 40], [0; 4]);
    for i in 0..4 {
        assert_eq!(q.payout_if(i), Ok(0));
        assert_eq!(q.information_interval(i), Ok((0, WAD)));
    }
    // even a single token is owed more than an empty pot has
    assert_eq!(even_book(0, [1, 0, 0, 0], [0; 4]).payout_if(0), Ok(0));
    // an answer nobody holds still pays 1, so its YES has a real upper end, and a NO on it pays on empty answers
    let q = even_book(0, [0, 20, 30, 40], [0; 4]);
    assert_eq!(q.payout_if(0), Ok(WAD));
    assert_eq!(q.information_interval(0), Ok((0, WAD / 4)));
    assert_eq!(q.information_interval(1), Ok((0, WAD)));
}

#[test]
fn the_interval_stops_at_zero_and_one() {
    // pot 100 on 1000 YES on 0: .25 / .1 = 2.5 is cut to 1, and a NO on 0 pays 1 on the empty answers, so the lower end
    // is 1 - .75 = .25. For answer 1 a NO can pay .1, and 1 - .75 / .1 is cut to 0
    let q = even_book(100, [1000, 0, 0, 0], [0; 4]);
    assert_eq!(q.information_interval(0), Ok((WAD / 4, WAD)));
    assert_eq!(q.information_interval(1), Ok((0, WAD / 4)));
    // a payout of one wei: the largest divisions there are
    let q = even_book(1, [WAD, 0, 0, 0], [0; 4]);
    assert_eq!(q.payout_if(0), Ok(1));
    assert_eq!(q.information_interval(0), Ok((WAD / 4, WAD)));
    assert_eq!(q.information_interval(1), Ok((0, WAD / 4)));
}

#[test]
fn the_interval_rounds_outward() {
    // pot 300 on 900 and on 360: payouts 1/3 and 5/6 rounded down. The true ends are .75 and .1, and the rounded
    // payouts push them to .75000000000000000075 and .09999999999999999964, which are rounded outward again
    let q = even_book(300, [900, 360, 0, 0], [0; 4]);
    assert_eq!(q.payout_if(0), Ok(333_333_333_333_333_333));
    assert_eq!(q.payout_if(1), Ok(833_333_333_333_333_333));
    let (lower, upper) = q.information_interval(0).unwrap();
    assert_eq!(upper, 750_000_000_000_000_001);
    assert_eq!(lower, 99_999_999_999_999_999);
}

#[test]
fn with_nothing_owed_the_interval_is_the_price() {
    let q = even_book(0, [0; 4], [0; 4]);
    for i in 0..4 {
        assert_eq!(q.information_interval(i), Ok((WAD / 4, WAD / 4)));
    }
}

#[test]
fn the_interval_after_real_trades_short_on_yes() {
    // .25 .125 .3125 .3125 at depth 1e6, so q0 is -1386294, -2079441, -1163150, -1163150. Answers 0 and 1 are bought up
    // to the same level, 40e6, which is more than 41 depths above the rest: the prices are exactly .5, .5, 0, 0. C is
    // 40e6 + trunc(1e6 ln 2) = 40693147 and the two purchases add one unit each
    let mut q = question(&[WAD / 4, WAD / 8, 5 * WAD / 16, 5 * WAD / 16], 1_000_000, 0, 0);
    assert_eq!(q.buy(0, true, 41_386_294, 0).unwrap().amount, 40_000_001);
    assert_eq!(q.buy(1, true, 42_079_441, 0).unwrap().amount, 693_148);
    assert_eq!(q.prices().unwrap(), vec![WAD / 2, WAD / 2, 0, 0]);
    assert_eq!(q.pot, 40_693_149);
    assert_eq!(q.payout_if(0), Ok(983_251_822_451_171_878));
    assert_eq!(q.payout_if(1), Ok(967_055_360_835_235_430));
    assert_eq!(q.payout_if(2), Ok(WAD));
    assert_eq!(q.information_interval(0), Ok((482_966_518_516_421_523, 508_516_728_454_708_679)));
    assert_eq!(q.information_interval(1), Ok((491_483_271_545_291_321, 517_033_481_483_578_477)));
    assert_eq!(q.information_interval(2), Ok((0, 0)));
}

#[test]
fn the_interval_after_real_trades_short_on_no() {
    // .125 .125 .375 .375 at depth 1e6: 43e6 NO on 2 and 43e6 NO on 3 lift answers 0 and 1 by 86e6 and leave them level,
    // 42 depths above the rest, so C is 83920559 + 693147 and each of 86e6 winning tokens is paid .98388
    let mut q = question(&[WAD / 8, WAD / 8, 3 * WAD / 8, 3 * WAD / 8], 1_000_000, 0, 0);
    q.buy(2, false, 43_000_000, 0).unwrap();
    q.buy(3, false, 43_000_000, 0).unwrap();
    assert_eq!(q.prices().unwrap(), vec![WAD / 2, WAD / 2, 0, 0]);
    assert_eq!(q.pot, 84_613_708);
    assert_eq!(q.payout_if(0), Ok(983_880_325_581_395_348));
    assert_eq!(q.payout_if(2), Ok(WAD));
    assert_eq!(q.information_interval(0), Ok((491_808_112_226_921_906, 508_191_887_773_078_094)));
    assert_eq!(q.information_interval(1), Ok((491_808_112_226_921_906, 508_191_887_773_078_094)));
    assert_eq!(q.information_interval(3), Ok((0, 0)));
}

#[test]
fn the_interval_is_the_formula_on_random_short_pots() {
    let mut r = Rng(0x5ac4_1d08_e77f_b392);
    let mut wide = 0;
    for &depth in &DEPTHS[..5] {
        for _ in 0..40 {
            let n = 3 + r.below(14);
            let p = random_prices(&mut r, n);
            let mut q = question(&p, depth, 0, 0);
            random_trading(&mut q, &mut r, depth, 10, |q, _| {
                let price = q.prices().unwrap();
                // the payouts from the pot and the supply, not from the code under test
                let rate: Vec<u128> = (0..n)
                    .map(|w| {
                        let owed = q.yes[w] + q.no.iter().sum::<u128>() - q.no[w];
                        if q.pot >= owed { WAD } else { mul_div(q.pot, WAD, owed) }
                    })
                    .collect();
                for i in 0..n {
                    let r_no = (0..n).filter(|&j| j != i).map(|j| rate[j]).min().unwrap();
                    let upper = mul_div_up(price[i], WAD, rate[i]).min(WAD);
                    let lower = WAD.saturating_sub(mul_div_up(WAD - price[i], WAD, r_no));
                    assert_eq!(q.information_interval(i), Ok((lower, upper)));
                    assert!(lower <= price[i] && price[i] <= upper, "the price is outside its interval");
                    wide += (upper - lower > WAD / 100) as u32;
                }
            });
        }
    }
    assert!(wide > 1000, "only {wide} intervals were wider than 1%");
}

// ------------------------------------------------------------------------------------------------ SPEC section 4: guarantee 1

#[test]
fn guarantee_1_through_the_pot_with_a_fee_and_no_cap() {
    // the round trip loses the two units of rounding and the two fees, and nothing else
    let mut q = question(&EVEN, 1_000_000_000, 10_000 * TOKEN, WAD / 50);
    q.buy(1, true, 700 * TOKEN, 0).unwrap();
    let before = q.pot;
    let buy = q.buy(0, true, 100 * TOKEN, 0).unwrap();
    let sale = q.sell(0, true, 100 * TOKEN, 0).unwrap();
    assert_eq!(buy.amount, sale.amount + 2);
    assert_eq!(q.pot, before + 2);
    assert_eq!(buy.fee, mul_div_up(buy.amount, WAD / 50, WAD));
    assert_eq!(sale.fee, mul_div_up(sale.amount, WAD / 50, WAD));
    assert!(sale.fee >= 1);
}

#[test]
fn guarantee_1_does_not_hold_when_the_cap_binds() {
    // the buyer above the upper end loses 14.32 of 91.35, with no fee at all
    let mut q = question(&EVEN, 1_000_000_000, 300 * TOKEN, 0);
    q.buy(0, true, 3000 * TOKEN, 0).unwrap();
    let cost = q.buy(0, true, 100 * TOKEN, 0).unwrap().amount;
    let refund = q.sell(0, true, 100 * TOKEN, 0).unwrap().amount;
    assert_eq!(cost - refund, 14_323_865);
}

#[test]
fn a_dust_sale_of_what_was_just_bought_differs_by_at_most_two_units() {
    let mut q = question(&[WAD / 1000, WAD / 2, WAD / 2 - WAD / 1000], 1_000_000_000, 5_000_000_000, 0);
    for t in 1..400 {
        let cost = q.buy(0, true, t, 0).unwrap().amount;
        let refund = q.sell(0, true, t, 0).unwrap().amount;
        assert!(cost >= refund && cost - refund <= 2, "{t} tokens: cost {cost}, refund {refund}");
    }
}

// ------------------------------------------------------------------------------------------------ SPEC section 4: guarantee 2

/// One trader alone. `paid` is the money in, less the money out, fees included.
#[derive(Clone)]
struct Trader {
    q: Question,
    yes: Vec<u128>,
    no: Vec<u128>,
    paid: i128,
}

impl Trader {
    fn new(q: &Question) -> Trader {
        Trader { q: q.clone(), yes: vec![0; q.n()], no: vec![0; q.n()], paid: 0 }
    }
    fn buy(&mut self, i: usize, yes: bool, n: u128) -> bool {
        let Ok(f) = self.q.buy(i, yes, n, 0) else { return false };
        self.paid += (f.amount + f.fee) as i128;
        if yes { self.yes[i] += n } else { self.no[i] += n }
        true
    }
    fn sell(&mut self, i: usize, yes: bool, n: u128) -> bool {
        let Ok(f) = self.q.sell(i, yes, n, 0) else { return false };
        self.paid -= (f.amount - f.fee) as i128;
        if yes { self.yes[i] -= n } else { self.no[i] -= n }
        true
    }
    /// The expected result at the chances `truth`, in 1e-18 units: each answer's tokens at its payout as it stands, which
    /// is what they will be paid with nobody else in the market, less the money paid.
    fn profit(&self, truth: &[u128]) -> i128 {
        let n = self.q.n();
        let all: u128 = self.no.iter().sum();
        let ev: u128 = (0..n).map(|w| truth[w] * mul_div(self.yes[w] + all - self.no[w], self.q.payout_if(w).unwrap(), WAD)).sum();
        ev as i128 - self.paid * WAD as i128
    }
}

/// Every sequence of up to `left` trades from `t`: each buy of a size in `sizes`, each sale of all or half of a holding.
/// Returns the best expected result among the sequences of at least one trade.
fn best_sequence(t: &Trader, truth: &[u128], sizes: &[u128], left: usize, tried: &mut u64) -> i128 {
    let mut best = i128::MIN;
    let consider = |c: Trader, best: &mut i128, tried: &mut u64| {
        *tried += 1;
        *best = (*best).max(c.profit(truth));
        if left > 1 {
            *best = (*best).max(best_sequence(&c, truth, sizes, left - 1, tried));
        }
    };
    for i in 0..t.q.n() {
        for yes in [true, false] {
            for &s in sizes {
                let mut c = t.clone();
                if c.buy(i, yes, s) {
                    consider(c, &mut best, tried);
                }
            }
            let h = if yes { t.yes[i] } else { t.no[i] };
            for s in [h, h / 2] {
                let mut c = t.clone();
                if s > 0 && c.sell(i, yes, s) {
                    consider(c, &mut best, tried);
                }
            }
        }
    }
    best
}

#[test]
fn guarantee_2_no_sequence_of_three_trades_makes_money() {
    // every sequence, at the bound: no fee, and from no fund to twice L. A purchase pays at least the rise in C plus a
    // unit and C rises at least as fast as the expected payout, so every sequence loses
    let wad = WAD as i128;
    for p in [[WAD / 2, 3 * WAD / 10, WAD / 5], [98 * WAD / 100, WAD / 100, WAD / 100]] {
        for depth in [1_000, 1_000_000_000u128] {
            let l = question(&p, depth, 0, 0).virtual_liquidity().unwrap();
            for fund in [0, l / 2, l, 2 * l] {
                let q = question(&p, depth, fund, 0);
                let mut tried = 0;
                let best = best_sequence(&Trader::new(&q), &p, &[1, depth / 100, depth, 8 * depth], 3, &mut tried);
                assert!(tried > 5_000, "{tried} sequences");
                assert!(best < 0, "depth {depth}, fund {fund}: expected profit {best} in 1e-18 units");
                // the search reaches the bound: the best it finds costs no more than 2 units, the rounding of one trade
                assert!(best > -2 * wad, "depth {depth}, fund {fund}: the best sequence lost {best}");
            }
        }
    }
}

#[test]
fn guarantee_2_holds_to_the_unit_on_random_books_up_to_depth_1e15() {
    // one purchase is the tightest case: with no fee and a full pot, every size from 1 token up, at every depth from a
    // single unit, the expected result stays below zero. Books chosen for it do better (the next two tests)
    let mut r = Rng(0xc3d2_e1f0_0f1e_2d3c);
    for depth in [1, 2, 3, 10, 100, 1_000, 1_000_000, 1_000_000_000, 1_000_000_000_000, 1_000_000_000_000_000u128] {
        for _ in 0..1500 {
            let n = 3 + r.below(3);
            let p = random_prices(&mut r, n);
            let l = Pool::open(depth, &p).virtual_liquidity();
            let q = question(&p, depth, 2 * l + 10, 0);
            let (i, yes, t) = (r.below(n), r.unit() < 0.5, r.log_between(1, depth));
            let mut trader = Trader::new(&q);
            assert!(trader.buy(i, yes, t));
            assert!(trader.profit(&p) < 0, "depth {depth}: {t} tokens, expected profit {}", trader.profit(&p));
        }
    }
}

#[test]
fn a_purchase_can_be_worth_more_than_it_costs_by_the_noise_in_c() {
    // four equal answers, where a buy takes the sum from exactly a power of two to just under it: 4 NO cost 2 and are
    // worth 3 in expectation
    let even = [WAD / 4; 4];
    let l = Pool::open(100_000_000_000_000_000, &even).virtual_liquidity();
    let mut q = question(&even, 100_000_000_000_000_000, 2 * l, 0);
    assert_eq!(q.buy(0, false, 4, 0).unwrap().amount, 2);
    // a large case: at depth 1e24 about 24 b / 1e18, inside the allowance the tests use; the largest found is about 26
    let depth = 1_000_000 * WAD;
    let l = Pool::open(depth, &even).virtual_liquidity();
    let mut q = question(&even, depth, 2 * l, 0);
    let tokens = 1_133_788_905_362_175u128;
    let cost = q.buy(0, false, tokens, 0).unwrap().amount;
    let (worth, paid) = (mul_div(tokens * 3, WAD / 4, WAD), cost);
    assert_eq!(paid, 850_341_654_362_176);
    assert!(worth > paid && worth - paid <= noise(depth), "worth {worth}, paid {paid}");
}

#[test]
fn any_fee_takes_a_noise_gain_back_at_ordinary_depths() {
    let even = [WAD / 4; 4];
    let l = Pool::open(100_000_000_000_000_000, &even).virtual_liquidity();
    let mut q = question(&even, 100_000_000_000_000_000, 2 * l, 1);
    let f = q.buy(0, false, 4, 0).unwrap();
    assert_eq!(f.amount + f.fee, 3);
}

/// What a trader who holds `tokens` YES on each answer expects to make, buying them one answer at a time.
fn position_profit(q: &Question, truth: &[u128], tokens: &[u128]) -> Option<i128> {
    let mut t = Trader::new(q);
    for (i, &n) in tokens.iter().enumerate() {
        if n > 0 && !t.buy(i, true, n) {
            return None;
        }
    }
    Some(t.profit(truth))
}

/// The best position a coordinate search finds, from `starts` random positions. On a curve that gives money away the
/// value climbs for ever, so the moves are limited: the search then stops with a profit to report.
fn best_position(q: &Question, truth: &[u128], r: &mut Rng, starts: usize) -> (i128, Vec<u128>) {
    let depth = q.pool.as_ref().unwrap().b as u128;
    let n = q.n();
    let mut best = (i128::MIN, vec![0; n]);
    for _ in 0..starts {
        let mut x: Vec<u128> = (0..n).map(|_| r.log_between(1, 4 * depth)).collect();
        let mut value = position_profit(q, truth, &x).unwrap();
        let mut step = depth;
        let mut moves = 0;
        while step >= 1 && moves < 2000 {
            let mut moved = false;
            for i in 0..n {
                for candidate in [x[i] + step, x[i].saturating_sub(step)] {
                    let mut y = x.clone();
                    y[i] = candidate;
                    if let Some(v) = position_profit(q, truth, &y) {
                        if v > value {
                            (value, x, moved) = (v, y, true);
                        }
                    }
                }
            }
            if moved {
                moves += 1;
            } else {
                step /= 4;
            }
        }
        if value > best.0 && x.iter().any(|&a| a > 0) {
            best = (value, x);
        }
    }
    best
}

#[test]
fn guarantee_2_the_best_position_at_the_true_chances_is_none() {
    let mut r = Rng(0xa17c_3f44_08e2_d95b);
    for p in [vec![WAD / 2, 3 * WAD / 10, WAD / 5], vec![MIN_OPENING_PRICE, MIN_OPENING_PRICE, WAD - 2 * MIN_OPENING_PRICE], vec![WAD / 4; 4]] {
        let depth = 1_000_000_000;
        let l = question(&p, depth, 0, 0).virtual_liquidity().unwrap();
        for fund in [0, l / 3, l, 3 * l] {
            let q = question(&p, depth, fund, 0);
            let (best, x) = best_position(&q, &p, &mut r, 4);
            assert!(best < 0, "fund {fund}: expected profit {best} from {x:?}");
        }
    }
}

#[test]
fn the_search_finds_the_money_when_there_is_some() {
    // the search can be trusted only if it finds a profit that exists. With a belief away from the prices and the pot
    // full, the most a trader can make is known exactly, b times the relative entropy of the belief from the prices, so
    // the same search must come close to that and must not pass it. .7 .2 .1 from .5 .3 .2:
    // b (.7 ln(.7/.5) + .2 ln(.2/.3) + .1 ln(.1/.2)) = 0.08512282596 b
    let (p, truth) = ([WAD / 2, 3 * WAD / 10, WAD / 5], [7 * WAD / 10, WAD / 5, WAD / 10]);
    let depth: u128 = 1_000_000_000;
    let most = 85_122_825i128;
    let q = question(&p, depth, 2 * depth, 0);
    let (best, _) = best_position(&q, &truth, &mut Rng(0x6e3b_90c1_f2d7_448a), 4);
    let best = best / WAD as i128;
    assert!(best <= most, "profit {best} above the most there is, {most}");
    assert!(best >= most - 10, "the search found {best} of {most}");
}

#[test]
fn guarantee_2_cycles_that_return_to_the_same_book_always_lose() {
    // any closed cycle pays at least one unit for each purchase, however much rounding noise there is in C: costs are
    // at least rises plus one and refunds at most falls, and the rises and the falls cancel
    let mut r = Rng(0x2dd1_7f0e_b3a5_c648);
    let mut refused = 0;
    let mut closed = 0;
    for &depth in &DEPTHS {
        for _ in 0..60 {
            let n = 3 + r.below(14);
            let p = random_prices(&mut r, n);
            let l = question(&p, depth, 0, 0).virtual_liquidity().unwrap();
            let fund = if r.below(2) == 0 { l } else { 2 * l };
            let mut q = question(&p, depth, fund, 0);
            for _ in 0..4 {
                let (book, pot) = (q.pool.as_ref().unwrap().q.clone(), q.pot);
                let (mut paid, mut got, mut buys) = (0, 0, 0);
                let mut held: Vec<(usize, bool, u128)> = vec![];
                for _ in 0..1 + r.below(3) {
                    let (i, yes, t) = (r.below(n), r.unit() < 0.5, r.log_between(1, depth));
                    paid += q.buy(i, yes, t, 0).unwrap().amount;
                    buys += 1;
                    held.push((i, yes, t));
                }
                while let Some((i, yes, t)) = held.pop() {
                    let part = if r.unit() < 0.5 { r.log_between(1, t) } else { t };
                    match q.sell(i, yes, part, 0) {
                        Ok(f) => {
                            got += f.amount;
                            if part < t {
                                held.push((i, yes, t - part));
                            }
                        }
                        Err(SaleTooSmall) => {
                            refused += 1;
                            match q.sell(i, yes, t, 0) {
                                Ok(f) => got += f.amount,
                                Err(e) => {
                                    assert_eq!(e, SaleTooSmall);
                                    held.clear();
                                    break;
                                }
                            }
                        }
                        Err(e) => panic!("{e}"),
                    }
                }
                if q.pool.as_ref().unwrap().q != book {
                    break; // a position nobody could sell back: the book did not close, so nothing to measure
                }
                closed += 1;
                assert!(paid >= got + buys, "depth {depth}: paid {paid}, got {got}, on {buys} purchases");
                assert_eq!(q.pot, pot + paid - got);
            }
        }
    }
    assert!(closed > 1000, "{closed} closed cycles");
    assert!(refused > 0, "no sale was refused on the way");
}

#[test]
fn a_sale_that_would_raise_c_is_refused() {
    // the cycle at depth 1e24: 2,000,000 YES on the leader cost 1 unit, selling 1 token raised computed C by
    // 1,999,999 and paid nothing, and selling the rest then took that rise out of the pot for free
    let mut q = question(&[5 * WAD / 10, 3 * WAD / 10, 2 * WAD / 10], 1_000_000_000_000_000_000_000_000, 0, 0);
    let cost = q.buy(0, true, 2_000_000, 0).unwrap().amount;
    assert_eq!(cost, 1);
    let pool = q.pool.as_ref().unwrap();
    let fall = pool.sell_fall(0, true, 1);
    assert_eq!(fall, -1_999_999, "this sale would raise C");
    assert_eq!(pool.sell_refund(0, true, 1), 0, "and the curve alone would call it a free sale");
    assert_eq!(q.quote_sell(0, true, 1, 0).unwrap_err(), SaleTooSmall);
    let before = q.clone();
    assert_eq!(q.sell(0, true, 1, 0), Err(SaleTooSmall));
    assert_eq!(q, before);
}

#[test]
fn a_sale_that_would_raise_c_is_refused_at_every_large_depth() {
    let mut refused = 0;
    for depth in [1_000_000_000_000_000_000u128, 1_000_000_000_000_000_000_000, 10_000_000_000_000_000_000_000_000_000] {
        let unit = depth / WAD;
        for t in 2..3000 {
            let mut q = question(&[5 * WAD / 10, 3 * WAD / 10, 2 * WAD / 10], depth, 0, 0);
            q.buy(0, true, t * unit, 0).unwrap();
            let fall = q.pool.as_ref().unwrap().sell_fall(0, true, 1);
            match q.quote_sell(0, true, 1, 0) {
                Err(e) => {
                    assert_eq!((e, fall < 0), (SaleTooSmall, true));
                    refused += 1;
                }
                Ok(_) => assert!(fall >= 0),
            }
        }
    }
    assert!(refused > 100, "only {refused} sales were refused");
}

// ------------------------------------------------------------------------------------------------ quotes

fn quote_buy_matches(q: &mut Question, i: usize, yes: bool, n: u128) -> Option<aimm_core::Fill> {
    let quote = q.quote_buy(i, yes, n, 0);
    match q.buy(i, yes, n, 0) {
        Ok(f) => {
            let (fill, after) = quote.unwrap();
            assert_eq!(fill, f);
            assert_eq!(after, q.prices().unwrap());
            Some(f)
        }
        Err(_) => None,
    }
}

#[test]
fn a_quote_is_exactly_what_the_trade_then_does() {
    let mut r = Rng(0x93f0_2a6b_d1c8_47e5);
    let (mut capped, mut plain) = (0, 0);
    for &depth in &DEPTHS[..6] {
        for _ in 0..30 {
            let n = 3 + r.below(14);
            let p = random_prices(&mut r, n);
            let mut q = question(&p, depth, 0, (r.below(3) as u128) * WAD / 40);
            let mut held: Vec<(usize, bool, u128)> = vec![];
            for _ in 0..24 {
                if !held.is_empty() && r.unit() < 0.5 {
                    let k = r.below(held.len());
                    let (i, yes, h) = held[k];
                    let s = r.log_between(h / 4, h);
                    let curve = q.pool.as_ref().unwrap().sell_refund(i, yes, s);
                    let quote = q.quote_sell(i, yes, s, 0);
                    match q.sell(i, yes, s, 0) {
                        Ok(f) => {
                            let (fill, after) = quote.unwrap();
                            assert_eq!(fill, f);
                            assert_eq!(after, q.prices().unwrap());
                            if f.amount < curve { capped += 1 } else { plain += 1 }
                            held[k].2 -= s;
                            if held[k].2 == 0 {
                                held.swap_remove(k);
                            }
                        }
                        Err(e) => assert_eq!(quote.unwrap_err(), e),
                    }
                } else {
                    let (i, yes, t) = (r.below(n), r.unit() < 0.6, r.log_between(depth / 100, 4 * depth));
                    if quote_buy_matches(&mut q, i, yes, t).is_some() {
                        held.push((i, yes, t));
                    }
                }
            }
        }
    }
    assert!(capped > 100 && plain > 100, "{capped} capped sales, {plain} others");
}

#[test]
fn the_quote_of_a_capped_sale_is_the_cap() {
    let mut q = question(&EVEN, 1_000_000_000, 300 * TOKEN, WAD / 10);
    q.buy(0, true, 3100 * TOKEN, 0).unwrap();
    let (f, after) = q.quote_sell(0, true, 100 * TOKEN, 0).unwrap();
    assert_eq!((f.amount, f.fee), (77_021_151, 7_702_116));
    assert!(after[0] < q.prices().unwrap()[0], "selling YES on 0 lowers its price");
}

#[test]
fn the_quote_of_a_buy_moves_the_prices_the_way_the_buy_will() {
    let q = question(&[WAD / 2, 3 * WAD / 10, WAD / 5], 1_000_000_000, 0, WAD / 10);
    let (f, after) = q.quote_buy(0, true, 1_000_000_000, 0).unwrap();
    assert_eq!((f.amount, f.fee), (620_114_508, 62_011_451));
    near(after[0], 731_058_578_667_507_340, 100, "the price after");
    near(after[1], 161_364_852_792_514_024, 100, "the price after");
    let (_, after) = q.quote_buy(0, false, 1_000_000_000, 0).unwrap();
    assert!(after[0] < WAD / 2 && after[1] > 3 * WAD / 10);
}

#[test]
fn a_quote_changes_nothing() {
    let mut q = question(&EVEN, 1_000_000_000, 0, 0);
    q.buy(0, true, 100 * TOKEN, 0).unwrap();
    let before = q.clone();
    let _ = q.quote_buy(1, false, 5 * TOKEN, 0);
    let _ = q.quote_sell(0, true, 5 * TOKEN, 0);
    assert_eq!(q, before);
    assert_eq!(Pool::open(1_000_000, &EVEN).q, Pool::open(1_000_000, &EVEN).q0);
}
