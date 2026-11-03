//! Replays a real Polymarket question (prepared by demo/prepare.py) on three markets run by the contract's own
//! arithmetic, with the same real money ℓ in the first two:
//!   AIMM      fund ℓ, virtual liquidity L = ℓ / 0.3        (the whitepaper's setting, ℓ = 0.3 L)
//!   AMM       fund ℓ, virtual liquidity L = ℓ              (fully backed: an AMM with the same money)
//!   AMM full  fund L, virtual liquidity L = ℓ / 0.3        (fully backed at AIMM's depth: what AIMM's curve would cost)
//! Every fill kept by demo/prepare.py runs on each market. All real traders are one account, and so is the informed
//! trader. Either one moves the market by selling what it holds of the opposite token, then buying the rest. Slippage
//! is what a real trade pays above the market's own price just before it, with the book carrying on from that trade
//! (the whitepaper's figures price each trade on a book of its own). Volume is at Polymarket's prices. Every minute the
//! informed trader, who believes the prices of demo/prepare.py, trades as the whitepaper's Proposition 3 says: YES on a
//! range while its price is above the range's upper probability, NO while it is below the lower one. With no fee
//! (FEE_BPS unset) only the mechanism differs.
//!
//!     cargo run --release -p aimm-core --example replay -- ../aimm-data/london.replay ../aimm-data/london
//! writes <out>.csv (one row per minute per market) and <out>.ops (every trade executed, for the on-chain replay).
use aimm_core::math::WAD;
use aimm_core::Question;
use std::fmt::Write as _;
use std::io::Write as _;

const UNIT: f64 = 1e6; // the collateral's smallest unit: a 6-decimal stablecoin
const ELL: f64 = 1_000.0; // real money, in dollars
const SHARE: f64 = 0.3; // ℓ / L for AIMM
const TOL: u128 = WAD / 1000; // the informed trader ignores gaps under 0.1 point

/// One transaction of a move: a sale of the opposite token, or a purchase.
struct Leg {
    sell: bool,
    yes: bool,
    tickets: u128,
    amount: u128,
    fee: u128,
    capped: bool,
}

/// Moves the market toward (i, yes) by `t` tokens: sells what `held` has of the opposite token, then buys the rest. Both
/// legs move the curve the same way. A sale the curve refuses as too small is bought instead.
fn step(q: &mut Question, held: &mut [u128], i: usize, yes: bool, t: u128) -> Result<Vec<Leg>, aimm_core::AimmError> {
    let n = q.n();
    let (mine, opposite) = if yes { (i, n + i) } else { (n + i, i) };
    let mut legs = Vec::new();
    let mut left = t;
    let s = t.min(held[opposite]);
    if s > 0 {
        let curve = q.pool.as_ref().unwrap().sell_refund(i, !yes, s);
        if let Ok(f) = q.sell(i, !yes, s, 0) {
            held[opposite] -= s;
            left -= s;
            legs.push(Leg { sell: true, yes: !yes, tickets: s, amount: f.amount, fee: f.fee, capped: f.amount < curve });
        }
    }
    if left > 0 {
        let f = q.buy(i, yes, left, 0)?;
        held[mine] += left;
        legs.push(Leg { sell: false, yes, tickets: left, amount: f.amount, fee: f.fee, capped: false });
    }
    Ok(legs)
}

struct Market {
    name: &'static str,
    q: Question,
    slippage: f64,
    volume: f64,
    arb_trades: u64,
    arb_volume: f64,
    sales: u64,
    capped: u64,
    outside: u64,
    /// tokens held, per actor (0 real traders, 1 informed trader), YES then NO on every answer, and money paid in
    held: [Vec<u128>; 2],
    paid: [f64; 2],
}

fn interval(q: &Question, i: usize) -> (u128, u128) {
    q.information_interval(i).unwrap()
}

/// The interval widened by the fee: a trader who pays fee f on top of the price gains from YES only if their belief
/// is above upper (1 + f), and from NO only if it is below 1 - (1 - lower)(1 + f).
fn edges(q: &Question, i: usize) -> (u128, u128) {
    let (lo, hi) = interval(q, i);
    let f = q.fee_rate;
    let hi_f = aimm_core::math::mul_div_up(hi, WAD + f, WAD);
    let lo_f = WAD.saturating_sub(aimm_core::math::mul_div_up(WAD - lo, WAD + f, WAD));
    (lo_f, hi_f)
}

/// The most tickets of (range, yes) the informed trader buys: the least n after which the edge reaches the belief.
fn tickets_to_reach(q: &Question, held: &[u128], i: usize, yes: bool, belief: u128) -> u128 {
    let reached = |n: u128| -> bool {
        let mut c = q.clone();
        if step(&mut c, &mut held.to_vec(), i, yes, n).is_err() {
            return true;
        }
        let (lo, hi) = edges(&c, i);
        if yes { hi >= belief } else { lo <= belief }
    };
    let mut hi: u128 = 1_000_000; // one dollar's worth of tokens
    while !reached(hi) && hi < (1u128 << 99) {
        hi *= 2;
    }
    let mut lo: u128 = 0;
    while hi - lo > 1_000 {
        let mid = lo + (hi - lo) / 2;
        if reached(mid) { hi = mid } else { lo = mid }
    }
    hi
}

impl Market {
    /// Runs a move for an actor (0 real traders, 1 informed trader), books it and writes it for the on-chain replay:
    /// <market> trade|arb|trade_sell|arb_sell <time> <range> <yes> <tickets> <cost or refund> <fee>
    fn trade(&mut self, ops: &mut String, k: usize, actor: usize, time: &str, i: usize, yes: bool, t: u128) -> Vec<Leg> {
        let legs = step(&mut self.q, &mut self.held[actor], i, yes, t).unwrap();
        for l in &legs {
            let who = ["trade", "arb"][actor];
            let kind = if l.sell { format!("{who}_sell") } else { who.to_string() };
            let _ = writeln!(ops, "{k} {kind} {time} {i} {} {} {} {}", l.yes as u8, l.tickets, l.amount, l.fee);
            if l.sell {
                self.paid[actor] -= (l.amount - l.fee) as f64 / UNIT;
                self.sales += 1;
                self.capped += l.capped as u64;
            } else {
                self.paid[actor] += (l.amount + l.fee) as f64 / UNIT;
            }
        }
        legs
    }
}

fn ms_index(name: &str) -> usize {
    match name { "AIMM" => 0, "AMM" => 1, _ => 2 }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let text = std::fs::read_to_string(&args[1]).expect("replay file");
    let out = args.get(2).cloned().unwrap_or_else(|| "replay".into());
    let header = text.lines().next().unwrap().to_string();
    let winner: usize = header.rsplit("winner: ").next().unwrap().trim().parse().unwrap();
    let events: Vec<Vec<String>> = text.lines().skip(1).map(|l| l.split_whitespace().map(String::from).collect()).collect();
    let first = events.iter().find(|e| e[0] == "T").unwrap();
    let p0f: Vec<f64> = first[2..].iter().map(|x| x.parse().unwrap()).collect();
    let mut p0: Vec<u128> = p0f.iter().map(|&p| (p * 1e18) as u128).collect();
    let big = (0..p0.len()).max_by_key(|&i| p0[i]).unwrap();
    let sum: u128 = p0.iter().sum();
    p0[big] = p0[big] + WAD - sum;
    let n = p0.len();
    let lnmin = (1e18 / *p0.iter().min().unwrap() as f64).ln();
    let make = |name, l: f64, fund: f64| -> Market {
        let fee: u128 = std::env::var("FEE_BPS").ok().map(|b| b.parse::<u128>().unwrap() * WAD / 10_000).unwrap_or(0);
        let mut q = Question::new(n, fee, u64::MAX - 2, u64::MAX - 1, u64::MAX, 0).unwrap();
        q.open((l / lnmin * UNIT) as u128, (fund * UNIT) as u128, &p0, 0).unwrap();
        Market { name, q, slippage: 0.0, volume: 0.0, arb_trades: 0, arb_volume: 0.0, sales: 0, capped: 0, outside: 0, held: [vec![0; 2 * n], vec![0; 2 * n]], paid: [0.0; 2] }
    };
    let mut ms = vec![make("AIMM", ELL / SHARE, ELL), make("AMM", ELL, ELL), make("AMM full", ELL / SHARE, ELL / SHARE)];
    let mut csv = String::from("time,market,range,target,price,lower,upper,pot,payout,slippage\n");
    let mut ops = String::new();
    // how each market was opened, so the on-chain replay opens it identically: open <market> <depth> <fund> <fee> <prices...>
    for (k, m) in ms.iter().enumerate() {
        let b = m.q.pool.as_ref().unwrap().b;
        let p: Vec<String> = p0.iter().map(|x| x.to_string()).collect();
        let _ = writeln!(ops, "open {k} {b} {} {} {}", m.q.fund, m.q.fee_rate, p.join(" "));
    }
    let mut minutes = 0u64;
    for e in &events {
        match e[0].as_str() {
            "X" => {
                let (i, yes): (usize, bool) = (e[2].parse().unwrap(), e[3] == "1");
                let shares: f64 = e[4].parse().unwrap();
                let price: f64 = e[5].parse().unwrap();
                let tickets = (shares * UNIT) as u128;
                if tickets == 0 {
                    continue;
                }
                for (k, m) in ms.iter_mut().enumerate() {
                    let p = m.q.prices().unwrap()[i] as f64 / 1e18;
                    let before = if yes { p } else { 1.0 - p }; // the market's own price just before the trade
                    let legs = m.trade(&mut ops, k, 0, &e[1], i, yes, tickets);
                    // s tokens sold for R are as good as s bought for s - R: a pair of opposites is worth 1 together
                    let net: f64 = legs.iter().map(|l| if l.sell { l.tickets as f64 - l.amount as f64 } else { l.amount as f64 }).sum();
                    m.slippage += net / UNIT - shares * before;
                    m.volume += shares * price;
                }
            }
            "T" => {
                minutes += 1;
                let target: Vec<u128> = e[2..].iter().map(|x| (x.parse::<f64>().unwrap() * 1e18) as u128).collect();
                for (k, m) in ms.iter_mut().enumerate() {
                    for _pass in 0..6 {
                        let mut traded = false;
                        for i in 0..n {
                            let (lo, hi) = edges(&m.q, i);
                            let (yes, gap) = if target[i] > hi + TOL { (true, true) } else if target[i] + TOL < lo { (false, true) } else { (true, false) };
                            if !gap {
                                continue;
                            }
                            let t = tickets_to_reach(&m.q, &m.held[1], i, yes, target[i]);
                            let legs = m.trade(&mut ops, k, 1, &e[1], i, yes, t);
                            m.arb_trades += 1;
                            m.arb_volume += legs.iter().map(|l| l.amount as f64).sum::<f64>() / UNIT;
                            traded = true;
                        }
                        if !traded {
                            break;
                        }
                    }
                    let p = m.q.prices().unwrap();
                    for i in 0..n {
                        let (lo, hi) = interval(&m.q, i);
                        if target[i] + TOL < lo || target[i] > hi + TOL {
                            m.outside += 1;
                        }
                        let _ = writeln!(csv, "{},{},{i},{},{},{},{},{},{},{:.4}", e[1], m.name, target[i] as f64 / 1e18, p[i] as f64 / 1e18,
                            lo as f64 / 1e18, hi as f64 / 1e18, m.q.pot as f64 / UNIT, m.q.payout_if(i).unwrap() as f64 / 1e18, m.slippage);
                    }
                }
            }
            _ => {}
        }
    }
    std::fs::File::create(format!("{out}.csv")).unwrap().write_all(csv.as_bytes()).unwrap();
    println!("{header}");
    println!("{minutes} minutes; real money {ELL} USDC in AIMM and AMM; AIMM's L = {:.0}", ELL / SHARE);
    println!("{:<9} {:>10} {:>12} {:>10} {:>11} {:>12} {:>7} {:>7} {:>14} {:>16}", "market", "fund", "real trades", "slippage", "slip/volume", "arb trades", "sales", "capped", "outside int.", "payout winner");
    for m in ms.iter_mut() {
        let fund = m.q.fund as f64 / UNIT;
        m.q.halt().unwrap();
        let r = m.q.resolve(winner, false, u64::MAX).unwrap() as f64 / 1e18;
        println!("{:<9} {:>10.0} {:>12.0} {:>10.2} {:>10.2}% {:>12} {:>7} {:>7} {:>14} {:>16.4}", m.name, fund, m.volume, m.slippage,
            100.0 * m.slippage / m.volume.max(1e-9), m.arb_trades, m.sales, m.capped, m.outside, r);
    }
    println!("\nsettlement (USDC): winners are paid from the pot; the operator gets the rest of the pot plus the fees");
    println!("{:<9} {:>8} {:>12} {:>14} {:>9} {:>9} {:>14} {:>20} {:>20}", "market", "fund", "pot at end", "paid winners", "surplus", "fees", "operator gets", "real traders net", "informed trader net");
    for m in ms.iter_mut() {
        let fund = m.q.fund as f64 / UNIT;
        let pot = m.q.pot as f64 / UNIT + m.q.surplus as f64 / UNIT;
        let mut won = [0.0f64; 2];
        let mut won_units = [0u128; 2];
        for a in 0..2 {
            let h = m.held[a].clone();
            if let Ok((_, amount)) = m.q.redeem(&h[..n], &h[n..]) { won[a] = amount as f64 / UNIT; won_units[a] = amount; }
        }
        let surplus_units = m.q.claim_surplus().unwrap();
        let fee_units = m.q.claim_fees();
        let surplus = surplus_units as f64 / UNIT;
        let fees = fee_units as f64 / UNIT;
        let k = ms_index(m.name);
        // what the on-chain replay must reproduce at settlement: settle <market> <winner> <payout> <redeem real> <redeem informed> <surplus> <fees> <dust left>
        let _ = writeln!(ops, "settle {k} {winner} {} {} {} {surplus_units} {fee_units} {}", m.q.payout, won_units[0], won_units[1], m.q.pot);
        println!("{:<9} {:>8.0} {:>12.2} {:>14.2} {:>9.2} {:>9.2} {:>14.2} {:>20.2} {:>20.2}", m.name, fund, pot, won[0] + won[1], surplus, fees,
            surplus + fees, won[0] - m.paid[0], won[1] - m.paid[1]);
    }
    std::fs::File::create(format!("{out}.ops")).unwrap().write_all(ops.as_bytes()).unwrap();
}
