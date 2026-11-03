//! AIMM's curve for a question with several answers, exactly one of which happens: Hanson's,
//! C(q) = b ln sum_i exp(q_i / b), with a fixed depth b (SPEC.md, section 1).
//!
//! q_i counts the tokens that pay if answer i happens: YES on i and NO on every other answer. The curve opens at
//! q0_i = b ln p0_i, so C(q0) = 0 and the prices exp(q_i / b) / sum_k exp(q_k / b) are the opening prices. Nobody
//! holds the opening tokens: the winning tokens on i are q_i - q0_i. The cost depends only on q, so a trade and its
//! reverse return the same amount. The cost of a move is floor(C after) - floor(C before) + 1 and a refund the same
//! difference less 1, so rounding C to a unit uses up the margin and the error in ln and exp is left over (SPEC.md,
//! section 4). Tokens and money are in the collateral's smallest unit, prices in WAD.

use crate::math::{exp_wad, ln_wad, mul_div, s_div_wad, s_mul_wad, EXP_FLOOR, IWAD, WAD};

/// The curve's state: depth b and the book q, with the opening book q0.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "near", near_sdk::near(serializers = [borsh]))]
pub struct Pool {
    /// The depth b, in the collateral's smallest unit.
    pub b: i128,
    /// The book: tokens owed on each answer, signed, in the same unit.
    pub q: Vec<i128>,
    /// The opening book.
    pub q0: Vec<i128>,
}

impl Pool {
    /// Opens the curve with depth `b` at `prices` (WAD, each in (0, 1]). The caller checks that they add up to 1.
    pub fn open(b: u128, prices: &[u128]) -> Pool {
        let b = i128::try_from(b).expect("depth too large");
        assert!(b > 0, "depth must be positive");
        let q0: Vec<i128> = prices
            .iter()
            .map(|&p| {
                assert!(p > 0 && p <= WAD, "opening price out of range");
                s_mul_wad(ln_wad(p as i128), b)
            })
            .collect();
        Pool { b, q: q0.clone(), q0 }
    }

    pub fn n(&self) -> usize {
        self.q.len()
    }

    /// The virtual liquidity L = b ln(1 / min p0): the most the curve can ever owe beyond what it takes in.
    pub fn virtual_liquidity(&self) -> u128 {
        (-*self.q0.iter().min().unwrap()).max(0) as u128
    }

    /// exp((x - m) / b) in WAD, with x <= m.
    fn term(&self, x: i128, m: i128) -> u128 {
        // past 41 b the term is below EXP_FLOOR; returning here also keeps the division below in range
        if m - x >= 41 * self.b {
            return 0;
        }
        let e = s_div_wad(x - m, self.b);
        if e <= EXP_FLOOR { 0 } else { exp_wad(e) as u128 }
    }

    /// C(q): everything the curve has taken in at the book `q`.
    pub fn cost_at(&self, q: &[i128]) -> i128 {
        let m = *q.iter().max().unwrap();
        let s: u128 = q.iter().map(|&x| self.term(x, m)).sum();
        m + s_mul_wad(self.b, ln_wad(s as i128))
    }

    /// Every answer's price (WAD) at the book `q`.
    pub fn prices_at(&self, q: &[i128]) -> Vec<u128> {
        let m = *q.iter().max().unwrap();
        let t: Vec<u128> = q.iter().map(|&x| self.term(x, m)).collect();
        let s: u128 = t.iter().sum();
        t.iter().map(|&x| mul_div(x, WAD, s)).collect()
    }

    pub fn prices(&self) -> Vec<u128> {
        self.prices_at(&self.q)
    }

    /// The book after buying (`n` > 0) or selling (`n` < 0) `n` YES on `i`, or NO if `!yes`: a NO moves every other answer.
    pub fn book_after(&self, i: usize, yes: bool, n: i128) -> Vec<i128> {
        self.q.iter().enumerate().map(|(j, &x)| if (j == i) == yes { x + n } else { x }).collect()
    }

    /// The rise in C from buying `tickets`, plus one unit.
    pub fn buy_cost(&self, i: usize, yes: bool, tickets: u128) -> u128 {
        let d = self.cost_at(&self.book_after(i, yes, tickets as i128)) - self.cost_at(&self.q);
        d.max(0) as u128 + 1
    }

    /// The fall in C from selling `tickets`, signed: rounding can make it negative on a dust sale.
    pub fn sell_fall(&self, i: usize, yes: bool, tickets: u128) -> i128 {
        self.cost_at(&self.q) - self.cost_at(&self.book_after(i, yes, -(tickets as i128)))
    }

    /// The fall in C from selling `tickets`, less one unit.
    pub fn sell_refund(&self, i: usize, yes: bool, tickets: u128) -> u128 {
        let d = self.sell_fall(i, yes, tickets);
        if d <= 1 { 0 } else { (d - 1) as u128 }
    }

    /// Moves the book by a purchase (`n` > 0) or a sale (`n` < 0).
    pub fn apply(&mut self, i: usize, yes: bool, n: i128) {
        self.q = self.book_after(i, yes, n);
    }

    /// Tokens held that pay if `i` happens: q_i - q0_i.
    pub fn winning(&self, i: usize) -> u128 {
        (self.q[i] - self.q0[i]).max(0) as u128
    }
}

#[allow(dead_code)]
const _: i128 = IWAD;
