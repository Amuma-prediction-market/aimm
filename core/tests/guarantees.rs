//! SPEC.md section 4, checked on random trading at 6, 18 and 24 decimals, from 3 to 16 answers, with tokens from the
//! size of the noise in C up to three times the depth: a round trip on the curve returns what it paid, no sequence is
//! profitable at the true chances, a sale never lowers the payout on an answer the sold token pays on; and the
//! accounting and the information interval hold. `claims.rs` has the same guarantees at values worked by hand.
mod common;
use aimm_core::lmsr::Pool;
use aimm_core::math::{mul_div, mul_div_up, WAD};
use aimm_core::question::AimmError::SaleTooSmall;
use aimm_core::Question;
use common::{noise, random_prices, Rng};

const DECIMALS: [u32; 3] = [6, 18, 24];

/// A question at `decimals` with 3 to 16 answers and a depth of 10 to 10,000 whole tokens, and a fund of `share` of L.
/// Returns it with its opening prices.
fn opened(r: &mut Rng, decimals: u32, share: f64, fee: bool) -> (Question, Vec<u128>) {
    let n = 3 + r.below(14);
    let prices = random_prices(r, n);
    let token = 10u128.pow(decimals);
    let depth = r.log_between(10, 10_000) * token;
    let l = Pool::open(depth, &prices).virtual_liquidity();
    let fee_rate = if fee { (r.unit() * 0.05 * 1e18) as u128 } else { 0 };
    let mut q = Question::new(n, fee_rate, 10, 10, 10, 0).unwrap();
    q.open(depth, mul_div(l, (share * 1e18) as u128, WAD), &prices, 0).unwrap();
    (q, prices)
}

fn depth(q: &Question) -> u128 {
    q.pool.as_ref().unwrap().b as u128
}

/// From one wei of ln, b / 1e18 units, up to three times the depth.
fn tickets(r: &mut Rng, q: &Question) -> u128 {
    r.log_between(depth(q) / WAD, 3 * depth(q))
}

#[test]
fn guarantee_1_round_trip_on_the_curve() {
    let mut r = Rng(0x1234_5678_9abc_def1);
    let mut trips = 0;
    for decimals in DECIMALS {
        for _ in 0..200 {
            let (mut q, _) = opened(&mut r, decimals, 1.0, true);
            for _ in 0..r.below(10) {
                let (i, yes, n) = (r.below(q.n()), r.unit() < 0.6, tickets(&mut r, &q));
                q.buy(i, yes, n, 0).unwrap();
            }
            let (i, yes, n) = (r.below(q.n()), r.unit() < 0.6, tickets(&mut r, &q));
            let pool = q.pool.clone().unwrap();
            let cost = pool.buy_cost(i, yes, n);
            let mut after = pool.clone();
            after.apply(i, yes, n as i128);
            let refund = after.sell_refund(i, yes, n);
            assert!(cost >= refund && cost - refund <= 2, "cost {cost} refund {refund}");

            // the same trip through the pot, where the fees come on top
            let pot = q.pot;
            let buy = q.buy(i, yes, n, 0).unwrap();
            assert_eq!(buy.amount, cost);
            match q.sell(i, yes, n, 0) {
                Ok(sale) => {
                    // the pot is full, so the cap is the tokens themselves, which only noise in C can reach
                    assert_eq!(sale.amount, refund.min(n));
                    assert_eq!(q.pot, pot + cost - sale.amount);
                    assert_eq!(sale.fee, mul_div_up(sale.amount, q.fee_rate, WAD));
                    trips += 1;
                }
                Err(e) => assert_eq!((e, cost), (SaleTooSmall, 1), "a sale is refused only where the rise was negative"),
            }
        }
    }
    assert!(trips > 400, "{trips} round trips");
}

#[test]
fn guarantee_2_no_profitable_sequence_at_the_true_chances() {
    // One trader alone, prices opening at the truth, no fee, a pot from empty to twice L, and sizes down to the noise. The
    // expected result, paid at the market's payout, is at most zero, give or take the noise in C.
    let mut r = Rng(0x0bad_cafe_1234_4321);
    let mut closest = [i128::MIN; 3];
    for (scale, decimals) in DECIMALS.into_iter().enumerate() {
        for run in 0..250 {
            let share = [0.0, 0.2, 1.0, 2.0][run % 4];
            let (mut q, truth) = opened(&mut r, decimals, share, false);
            let n = q.n();
            let mut yes_h = vec![0u128; n];
            let mut no_h = vec![0u128; n];
            let mut paid: i128 = 0;
            for _ in 0..1 + r.below(12) {
                let (i, yes) = (r.below(n), r.unit() < 0.5);
                let held = if yes { yes_h[i] } else { no_h[i] };
                if held > 0 && r.unit() < 0.4 {
                    let s = held / 2 + 1;
                    if let Ok(f) = q.sell(i, yes, s, 0) {
                        paid -= (f.amount - f.fee) as i128;
                        if yes { yes_h[i] -= s } else { no_h[i] -= s }
                    }
                } else {
                    let t = tickets(&mut r, &q);
                    let f = q.buy(i, yes, t, 0).unwrap();
                    paid += (f.amount + f.fee) as i128;
                    if yes { yes_h[i] += t } else { no_h[i] += t }
                }
            }
            let ev: u128 = (0..n)
                .map(|w| {
                    let toks: u128 = yes_h[w] + (0..n).filter(|&j| j != w).map(|j| no_h[j]).sum::<u128>();
                    mul_div(truth[w], mul_div(toks, q.payout_if(w).unwrap(), WAD), WAD)
                })
                .sum();
            let profit = ev as i128 - paid;
            assert!(profit <= noise(depth(&q)) as i128, "expected profit {profit} on {paid} paid at depth {}", depth(&q));
            closest[scale] = closest[scale].max(profit - noise(depth(&q)) as i128);
        }
    }
    // the test means something only if some run came near the bound: within 10 units at 6 decimals, where the noise is
    // below a unit, and within 1e-14 of a token above that
    for (scale, within) in [10, 10_000, 10_000_000_000].into_iter().enumerate() {
        assert!(closest[scale] > -within, "the closest run at {} decimals was {}", DECIMALS[scale], closest[scale]);
    }
}

#[test]
fn guarantee_3_a_sale_never_lowers_the_payout_on_what_it_pays_on() {
    let mut r = Rng(0xdead_beef_cafe_f00d);
    let (mut sales, mut capped) = (0, 0);
    for decimals in DECIMALS {
        for _ in 0..200 {
            let share = 0.1 * r.unit(); // a short fund, so the cap binds
            let (mut q, _) = opened(&mut r, decimals, share, true);
            let mut held: Vec<(usize, bool, u128)> = Vec::new();
            for _ in 0..30 {
                if !held.is_empty() && r.unit() < 0.4 {
                    let k = r.below(held.len());
                    let (i, yes, h) = held[k];
                    let n = ((h as f64 * (0.2 + 0.8 * r.unit())) as u128 + 1).min(h);
                    let pays_on: Vec<usize> = (0..q.n()).filter(|&w| (w == i) == yes).collect();
                    let before: Vec<u128> = pays_on.iter().map(|&w| q.payout_if(w).unwrap()).collect();
                    let curve = q.pool.as_ref().unwrap().sell_refund(i, yes, n);
                    match q.sell(i, yes, n, 0) {
                        Ok(f) => {
                            for (k2, &w) in pays_on.iter().enumerate() {
                                assert!(q.payout_if(w).unwrap() >= before[k2], "payout on {w} fell");
                            }
                            let least = *before.iter().min().unwrap();
                            assert_eq!(f.amount, curve.min(mul_div(n, least, WAD)));
                            capped += (f.amount < curve) as u32;
                            sales += 1;
                            held[k].2 -= n;
                            if held[k].2 == 0 {
                                held.swap_remove(k);
                            }
                        }
                        Err(e) => assert_eq!(e, SaleTooSmall),
                    }
                } else {
                    let (i, yes, n) = (r.below(q.n()), r.unit() < 0.6, tickets(&mut r, &q));
                    q.buy(i, yes, n, 0).unwrap();
                    held.push((i, yes, n));
                }
            }
        }
    }
    assert!(sales > 5000 && capped > 500, "{sales} sales, {capped} of them capped");
}

#[test]
fn the_accounting_holds_and_every_winning_token_is_paid_the_same() {
    let mut r = Rng(0x5eed_5eed_5eed_5eed);
    for decimals in DECIMALS {
        for _ in 0..120 {
            let share = 0.5 * r.unit();
            let (mut q, _) = opened(&mut r, decimals, share, true);
            let n = q.n();
            let fund = q.fund;
            let (mut costs, mut refunds, mut fees) = (0u128, 0u128, 0u128);
            let mut holders: Vec<(Vec<u128>, Vec<u128>)> = (0..3).map(|_| (vec![0; n], vec![0; n])).collect();
            for _ in 0..25 {
                let h = r.below(3);
                let (i, yes) = (r.below(n), r.unit() < 0.6);
                let held = if yes { holders[h].0[i] } else { holders[h].1[i] };
                if held > 0 && r.unit() < 0.3 {
                    if let Ok(f) = q.sell(i, yes, held, 0) {
                        refunds += f.amount;
                        fees += f.fee;
                        if yes { holders[h].0[i] = 0 } else { holders[h].1[i] = 0 }
                    }
                } else {
                    let t = tickets(&mut r, &q);
                    let f = q.buy(i, yes, t, 0).unwrap();
                    costs += f.amount;
                    fees += f.fee;
                    if yes { holders[h].0[i] += t } else { holders[h].1[i] += t }
                }
                assert_eq!(q.pot, fund + costs - refunds);
                assert_eq!(q.fees_paid, fees);
                for w in 0..n {
                    assert_eq!(q.winning_supply(w), q.pool.as_ref().unwrap().winning(w), "supply and curve disagree");
                }
                let price = q.prices().unwrap();
                assert!(price.iter().sum::<u128>() <= WAD && price.iter().sum::<u128>() + 16 >= WAD, "{price:?}");
                for i in 0..n {
                    let (lo, hi) = q.information_interval(i).unwrap();
                    assert!(lo <= price[i] && price[i] <= hi, "price outside its interval");
                }
            }
            let w = r.below(n);
            q.halt().unwrap();
            let pot = q.pot;
            let owed = q.winning_supply(w);
            let rate = q.resolve(w, false, 10).unwrap();
            let kept = mul_div_up(owed, rate, WAD);
            assert_eq!((q.pot, q.surplus), (kept, pot - kept));
            let mut paid = 0u128;
            for (y, no) in &holders {
                let toks: u128 = y[w] + (0..n).filter(|&j| j != w).map(|j| no[j]).sum::<u128>();
                if toks == 0 {
                    continue;
                }
                let (_, amount) = q.redeem(y, no).unwrap();
                assert_eq!(amount, mul_div(toks, rate, WAD), "a holder was paid at a different rate");
                paid += amount;
            }
            assert_eq!(q.winning_supply(w), 0, "every winning token was redeemed");
            let surplus = q.claim_surplus().unwrap();
            assert!(paid + surplus <= pot, "paid out more than the pot");
            assert!(pot - paid - surplus <= 4, "dust {}", pot - paid - surplus);
        }
    }
}

/// Drives the price of the cheapest answer up to near 1, by YES on it and NO on the others, with some sales on the way,
/// and calls `check` after each step. That answer is the tightest: it owes the most while its price is near 1, when C
/// has only just cleared what it owes.
fn drive_to_one(q: &mut Question, r: &mut Rng, mut check: impl FnMut(&Question)) -> usize {
    let w = (0..q.n()).min_by_key(|&i| q.pool.as_ref().unwrap().q0[i]).unwrap();
    let step = depth(q) / 2;
    let mut held = 0;
    while q.prices().unwrap()[w] < 999 * WAD / 1000 {
        if r.unit() < 0.25 && held > 0 {
            let s = r.log_between(1, held);
            if q.sell(w, true, s, 0).is_ok() {
                held -= s;
            }
        } else if r.unit() < 0.6 {
            q.buy(w, true, step, 0).unwrap();
            held += step;
        } else {
            let other = (w + 1 + r.below(q.n() - 1)) % q.n();
            q.buy(other, false, step, 0).unwrap();
        }
        check(q);
    }
    w
}

#[test]
fn a_full_fund_collapses_the_interval_to_the_price() {
    let mut r = Rng(42);
    for decimals in DECIMALS {
        for _ in 0..40 {
            let (mut q, _) = opened(&mut r, decimals, 1.0, false);
            let n = q.n();
            drive_to_one(&mut q, &mut r, |q| {
                let price = q.prices().unwrap();
                for i in 0..n {
                    assert_eq!(q.payout_if(i), Ok(WAD), "fund = L must pay every token in full");
                    assert_eq!(q.information_interval(i), Ok((price[i], price[i])), "fund = L must give a point interval");
                }
            });
        }
    }
}

#[test]
fn a_fund_below_l_does_not_collapse_it() {
    // the same drive with 5% less money: near a price of 1 the pot is short by about those 5%. So L is the right size
    // to the unit of a fund, and `a_full_fund_collapses_the_interval_to_the_price` is not passing for another reason
    let mut r = Rng(42);
    for decimals in DECIMALS {
        for _ in 0..40 {
            let (mut q, _) = opened(&mut r, decimals, 0.95, false);
            let w = drive_to_one(&mut q, &mut r, |_| {});
            let (lo, hi) = q.information_interval(w).unwrap();
            assert!(q.payout_if(w).unwrap() < WAD, "the pot should be short");
            assert!(hi - lo > 1_000_000, "the interval should be open: {lo} to {hi}");
        }
    }
}
