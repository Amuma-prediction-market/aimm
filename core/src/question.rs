//! One question with several answers, exactly one of which happens, priced by AIMM: the curve of `lmsr`, a pot that
//! holds the operator's real money, and a payout of min(1, pot / winning tokens) on every winning token.
//!
//! This is the arithmetic of SPEC.md, sections 2 and 6,
//! with no accounts, no transfers and no clock of its own: the caller passes `now` and checks who is calling.
//! Holders' token balances are the caller's business too; the question keeps the total supply of every token.

use crate::lmsr::Pool;
use crate::math::{mul_div, mul_div_up, WAD};

pub const MIN_RANGES: usize = 3;
pub const MAX_RANGES: usize = 16;
/// The fee is at most a tenth of the curve's cost.
pub const MAX_FEE_RATE: u128 = WAD / 10;
/// Every opening price is at least one in a million.
pub const MIN_OPENING_PRICE: u128 = 1_000_000_000_000;
/// The most tokens one trade can move, and the most winning tokens an answer can carry, in the collateral's smallest
/// unit: 2^100, about 1.27e12 whole tokens at 18 decimals. (The book is i128.)
pub const MAX_TICKETS: u128 = 1 << 100;
/// The largest depth b, in the same unit: 2^100.
pub const MAX_DEPTH: u128 = 1 << 100;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AimmError {
    BadRange,
    BadConfig,
    BadPrices,
    BadDepth,
    AlreadyOpened,
    NotOpen,
    TradingClosed,
    StillTrading,
    TooEarly,
    AlreadyResolved,
    NotResolved,
    ZeroAmount,
    SaleTooSmall,
    TooManyTickets,
    SupplyShort,
    PotShort,
    AlreadyClaimed,
}

impl core::fmt::Display for AimmError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{:?}", self)
    }
}

pub type Res<T> = Result<T, AimmError>;

/// What a trade did: the curve's amount (cost of a buy, capped refund of a sale) and the fee.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Fill {
    pub amount: u128,
    pub fee: u128,
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "near", near_sdk::near(serializers = [borsh]))]
pub struct Question {
    pub ranges: u8,
    /// The fee as a share of the curve's cost or refund (WAD), at most 0.1. The operator's revenue.
    pub fee_rate: u128,
    /// Trading stops at this time.
    pub close_time: u64,
    /// The earliest time the resolver may report, once trading has stopped.
    pub resolve_after: u64,
    /// The earliest time the fallback resolver may report.
    pub fallback_after: u64,
    pub halted: bool,
    pub resolved: bool,
    pub winner: u8,
    pub pool: Option<Pool>,
    /// The operator's real money, put in at the opening.
    pub fund: u128,
    /// The fund, plus every curve cost paid in, less every refund and redemption paid out and the surplus set aside
    /// at the result.
    pub pot: u128,
    pub fees_paid: u128,
    pub fees_owed: u128,
    /// Supply of YES and NO on each answer.
    pub yes: Vec<u128>,
    pub no: Vec<u128>,
    pub no_total: u128,
    /// The payout per winning token (WAD), fixed at the result.
    pub payout: u128,
    /// What the result left over for the operator, until claimed.
    pub surplus: u128,
    pub surplus_claimed: bool,
}

/// min(1, money / owed) in WAD; 1 if nothing is owed.
pub fn payout_rate(money: u128, owed: u128) -> u128 {
    if owed == 0 || money >= owed { WAD } else { mul_div(money, WAD, owed) }
}

impl Question {
    pub fn new(ranges: usize, fee_rate: u128, close_time: u64, resolve_after: u64, fallback_after: u64, now: u64) -> Res<Question> {
        if !(MIN_RANGES..=MAX_RANGES).contains(&ranges) {
            return Err(AimmError::BadRange);
        }
        if close_time <= now || resolve_after < close_time || fallback_after < resolve_after || fee_rate > MAX_FEE_RATE {
            return Err(AimmError::BadConfig);
        }
        Ok(Question {
            ranges: ranges as u8,
            fee_rate,
            close_time,
            resolve_after,
            fallback_after,
            halted: false,
            resolved: false,
            winner: 0,
            pool: None,
            fund: 0,
            pot: 0,
            fees_paid: 0,
            fees_owed: 0,
            yes: vec![0; ranges],
            no: vec![0; ranges],
            no_total: 0,
            payout: 0,
            surplus: 0,
            surplus_claimed: false,
        })
    }

    pub fn n(&self) -> usize {
        self.ranges as usize
    }

    fn pool(&self) -> Res<&Pool> {
        self.pool.as_ref().ok_or(AimmError::NotOpen)
    }

    /// Opens the curve with depth `depth` at `prices` (WAD, adding up to exactly 1, each at least 1e-6) and puts
    /// the operator's `fund` into the pot. Once.
    pub fn open(&mut self, depth: u128, fund: u128, prices: &[u128], now: u64) -> Res<()> {
        if self.pool.is_some() {
            return Err(AimmError::AlreadyOpened);
        }
        if self.halted || now >= self.close_time {
            return Err(AimmError::TradingClosed);
        }
        if prices.len() != self.n() {
            return Err(AimmError::BadRange);
        }
        if prices.iter().any(|&p| p < MIN_OPENING_PRICE || p > WAD) || prices.iter().sum::<u128>() != WAD {
            return Err(AimmError::BadPrices);
        }
        if depth == 0 || depth > MAX_DEPTH {
            return Err(AimmError::BadDepth);
        }
        self.pool = Some(Pool::open(depth, prices));
        self.fund = fund;
        self.pot = fund;
        Ok(())
    }

    /// Supply of tokens that pay if `w` happens: YES on w and NO on every other answer.
    pub fn winning_supply(&self, w: usize) -> u128 {
        self.yes[w] + self.no_total - self.no[w]
    }

    /// Before the result: what each winning token would be paid if `w` happened now. After it: the payout for the
    /// winner and 0 for any other answer.
    pub fn payout_if(&self, w: usize) -> Res<u128> {
        if w >= self.n() {
            return Err(AimmError::BadRange);
        }
        if self.resolved {
            return Ok(if w == self.winner as usize { self.payout } else { 0 });
        }
        Ok(payout_rate(self.pot, self.winning_supply(w)))
    }

    fn check_trade(&self, range: usize, tickets: u128, now: u64) -> Res<()> {
        if self.pool.is_none() || self.resolved {
            return Err(AimmError::NotOpen);
        }
        if self.halted || now >= self.close_time {
            return Err(AimmError::TradingClosed);
        }
        if range >= self.n() {
            return Err(AimmError::BadRange);
        }
        if tickets == 0 {
            return Err(AimmError::ZeroAmount);
        }
        if tickets > MAX_TICKETS {
            return Err(AimmError::TooManyTickets);
        }
        Ok(())
    }

    /// What a purchase must pass: `check_trade`, and no answer ends up with more winning tokens than the limit.
    fn check_buy(&self, range: usize, yes: bool, tickets: u128, now: u64) -> Res<()> {
        self.check_trade(range, tickets, now)?;
        let pool = self.pool()?;
        for i in 0..self.n() {
            if (i == range) == yes && pool.winning(i) + tickets > MAX_TICKETS {
                return Err(AimmError::TooManyTickets);
            }
        }
        Ok(())
    }

    fn fee_on(&self, amount: u128) -> u128 {
        mul_div_up(amount, self.fee_rate, WAD)
    }

    /// The cost of `tickets` YES (or NO) on `range`, its fee, and every price after.
    pub fn quote_buy(&self, range: usize, yes: bool, tickets: u128, now: u64) -> Res<(Fill, Vec<u128>)> {
        self.check_buy(range, yes, tickets, now)?;
        let pool = self.pool()?;
        let cost = pool.buy_cost(range, yes, tickets);
        let after = pool.prices_at(&pool.book_after(range, yes, tickets as i128));
        Ok((Fill { amount: cost, fee: self.fee_on(cost) }, after))
    }

    /// Buys: the cost goes to the pot, the fee to the operator, and the supply of the token grows.
    pub fn buy(&mut self, range: usize, yes: bool, tickets: u128, now: u64) -> Res<Fill> {
        self.check_buy(range, yes, tickets, now)?;
        let pool = self.pool()?;
        let cost = pool.buy_cost(range, yes, tickets);
        let fee = self.fee_on(cost);
        self.pool.as_mut().unwrap().apply(range, yes, tickets as i128);
        self.pot += cost;
        if yes {
            self.yes[range] += tickets;
        } else {
            self.no[range] += tickets;
            self.no_total += tickets;
        }
        self.fees_paid += fee;
        self.fees_owed += fee;
        Ok(Fill { amount: cost, fee })
    }

    /// The curve's refund for a sale, capped at the seller's share of the pot as it stands: tickets times the payout
    /// on the sold answer (for a NO, the least payout among the answers it pays on).
    fn capped_refund(&self, range: usize, yes: bool, tickets: u128) -> Res<u128> {
        // a dust sale whose rounding raises C would let a buy back cost less than the sale left in the pot
        if self.pool()?.sell_fall(range, yes, tickets) < 0 {
            return Err(AimmError::SaleTooSmall);
        }
        let curve = self.pool()?.sell_refund(range, yes, tickets);
        let r = if yes {
            self.payout_if(range)?
        } else {
            (0..self.n()).filter(|&j| j != range).map(|j| payout_rate(self.pot, self.winning_supply(j))).min().unwrap()
        };
        Ok(curve.min(mul_div(tickets, r, WAD)))
    }

    /// What a sale must pass, and its refund.
    fn check_sell(&self, range: usize, yes: bool, tickets: u128, now: u64) -> Res<u128> {
        self.check_trade(range, tickets, now)?;
        let supply = if yes { self.yes[range] } else { self.no[range] };
        if supply < tickets {
            return Err(AimmError::SupplyShort);
        }
        let refund = self.capped_refund(range, yes, tickets)?;
        if refund > self.pot {
            return Err(AimmError::PotShort);
        }
        Ok(refund)
    }

    /// The refund of selling `tickets`, the fee off it, and every price after. A view cannot know the caller, so the
    /// quote holds for tokens the market has issued, not for what one seller holds.
    pub fn quote_sell(&self, range: usize, yes: bool, tickets: u128, now: u64) -> Res<(Fill, Vec<u128>)> {
        let refund = self.check_sell(range, yes, tickets, now)?;
        let pool = self.pool()?;
        let after = pool.prices_at(&pool.book_after(range, yes, -(tickets as i128)));
        Ok((Fill { amount: refund, fee: self.fee_on(refund) }, after))
    }

    /// Sells: the curve moves back, the supply shrinks, the refund leaves the pot. The seller receives refund - fee;
    /// the caller must check the seller holds the tokens.
    pub fn sell(&mut self, range: usize, yes: bool, tickets: u128, now: u64) -> Res<Fill> {
        let refund = self.check_sell(range, yes, tickets, now)?;
        let fee = self.fee_on(refund);
        self.pool.as_mut().unwrap().apply(range, yes, -(tickets as i128));
        if yes {
            self.yes[range] -= tickets;
        } else {
            self.no[range] -= tickets;
            self.no_total -= tickets;
        }
        self.pot -= refund;
        self.fees_paid += fee;
        self.fees_owed += fee;
        Ok(Fill { amount: refund, fee })
    }

    /// The operator takes the fees collected so far.
    pub fn claim_fees(&mut self) -> u128 {
        core::mem::take(&mut self.fees_owed)
    }

    /// Stops trading now, saying nothing about the result.
    pub fn halt(&mut self) -> Res<()> {
        if self.pool.is_none() {
            return Err(AimmError::NotOpen);
        }
        if self.resolved {
            return Err(AimmError::AlreadyResolved);
        }
        self.halted = true;
        Ok(())
    }

    /// Reports the winning answer: fixes the payout r = min(1, pot / W_w), rounded down, keeps ceil(r W_w) for the
    /// winners and records the rest of the pot for the operator. `by_fallback` is true when the fallback resolver
    /// reports. Final.
    pub fn resolve(&mut self, w: usize, by_fallback: bool, now: u64) -> Res<u128> {
        if by_fallback {
            if now < self.fallback_after {
                return Err(AimmError::TooEarly);
            }
        } else {
            if !self.halted && now < self.close_time {
                return Err(AimmError::StillTrading);
            }
            if now < self.resolve_after {
                return Err(AimmError::TooEarly);
            }
        }
        if self.pool.is_none() {
            return Err(AimmError::NotOpen);
        }
        if self.resolved {
            return Err(AimmError::AlreadyResolved);
        }
        if w >= self.n() {
            return Err(AimmError::BadRange);
        }
        let owed = self.winning_supply(w);
        let r = payout_rate(self.pot, owed);
        let keep = mul_div_up(owed, r, WAD).min(self.pot);
        self.resolved = true;
        self.winner = w as u8;
        self.payout = r;
        self.surplus = self.pot - keep;
        self.pot = keep;
        Ok(r)
    }

    /// The operator's share of the pot after the result. Once.
    pub fn claim_surplus(&mut self) -> Res<u128> {
        if !self.resolved {
            return Err(AimmError::NotResolved);
        }
        if self.surplus_claimed {
            return Err(AimmError::AlreadyClaimed);
        }
        self.surplus_claimed = true;
        Ok(core::mem::take(&mut self.surplus))
    }

    /// Redeems a holder's winning tokens, given what the holder has of YES and NO on every answer. Returns the
    /// tokens burned on each side, so the caller can zero them, and the amount paid: floor(tokens x payout).
    pub fn redeem(&mut self, held_yes: &[u128], held_no: &[u128]) -> Res<(Vec<(usize, bool, u128)>, u128)> {
        if !self.resolved {
            return Err(AimmError::NotResolved);
        }
        let w = self.winner as usize;
        let mut burned = Vec::new();
        let mut tickets: u128 = 0;
        for i in 0..self.n() {
            let (yes, b) = if i == w { (true, held_yes[i]) } else { (false, held_no[i]) };
            if b == 0 {
                continue;
            }
            if yes {
                self.yes[i] -= b;
            } else {
                self.no[i] -= b;
                self.no_total -= b;
            }
            tickets += b;
            burned.push((i, yes, b));
        }
        if tickets == 0 {
            return Err(AimmError::ZeroAmount);
        }
        let amount = mul_div(tickets, self.payout, WAD);
        self.pot -= amount;
        Ok((burned, amount))
    }

    /// After the result: takes a holder's losing tokens out of the supply, so their record can go. It pays nothing.
    /// The winning supply and the pot do not change.
    pub fn clear_losing(&mut self, held_yes: &[u128], held_no: &[u128]) -> Res<()> {
        if !self.resolved {
            return Err(AimmError::NotResolved);
        }
        let w = self.winner as usize;
        for i in 0..self.n() {
            if i == w {
                self.no[i] -= held_no[i];
                self.no_total -= held_no[i];
            } else {
                self.yes[i] -= held_yes[i];
            }
        }
        Ok(())
    }

    pub fn prices(&self) -> Res<Vec<u128>> {
        if self.resolved {
            return Err(AimmError::NotOpen);
        }
        Ok(self.pool()?.prices())
    }

    /// The whitepaper's information interval for answer `i` (Definition 5), in WAD: the upper probability
    /// min(1, p / r_yes) and the lower probability max(0, 1 - (1 - p) / r_no), where r_yes is what a YES on i would
    /// be paid as the pot stands and r_no the least of what a NO on i could be paid. With two answers a risk-neutral
    /// trader buys YES exactly when their belief is above the upper end and NO exactly when it is below the lower end
    /// (Proposition 3). With three or more answers r_no is the worst case, so the upper end still decides a YES, and a
    /// belief below the lower end is enough to buy NO, but not necessary. All of this is for the next token, with no fee
    /// and the pot as it stands. Rounded outward.
    pub fn information_interval(&self, i: usize) -> Res<(u128, u128)> {
        if i >= self.n() {
            return Err(AimmError::BadRange);
        }
        let p = self.prices()?[i];
        let r_yes = self.payout_if(i)?;
        let r_no = (0..self.n()).filter(|&j| j != i).map(|j| payout_rate(self.pot, self.winning_supply(j))).min().unwrap();
        let upper = if r_yes == 0 { WAD } else { mul_div_up(p, WAD, r_yes).min(WAD) };
        let lower = if r_no == 0 { 0 } else { WAD.saturating_sub(mul_div_up(WAD - p, WAD, r_no)) };
        Ok((lower, upper))
    }

    pub fn virtual_liquidity(&self) -> Res<u128> {
        Ok(self.pool()?.virtual_liquidity())
    }
}
