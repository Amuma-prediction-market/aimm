//! What the test files share.
#![allow(dead_code)]
use aimm_core::math::WAD;
use aimm_core::question::MIN_OPENING_PRICE;

pub struct Rng(pub u64);
impl Rng {
    pub fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    pub fn unit(&mut self) -> f64 {
        (self.next() >> 11) as f64 / (1u64 << 53) as f64
    }
    pub fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
    pub fn wide(&mut self) -> u128 {
        (self.next() as u128) << 64 | self.next() as u128
    }
    /// Uniform in the logarithm, from `lo` to `hi`.
    pub fn log_between(&mut self, lo: u128, hi: u128) -> u128 {
        let (a, b) = ((lo.max(1) as f64).ln(), (hi.max(1) as f64).ln());
        ((a + (b - a) * self.unit()).exp() as u128).clamp(lo.max(1), hi.max(1))
    }
}

/// Opening prices for `n` answers, adding up to exactly 1. About one question in four has answers at the lowest price
/// allowed.
pub fn random_prices(r: &mut Rng, n: usize) -> Vec<u128> {
    let w: Vec<f64> = (0..n).map(|_| 0.05 + r.unit()).collect();
    let s: f64 = w.iter().sum();
    let mut p: Vec<u128> = w.iter().map(|x| ((x / s) * 0.999e18) as u128).collect();
    if r.unit() < 0.25 {
        for i in 1..1 + r.below(n - 1) {
            p[i] = MIN_OPENING_PRICE;
        }
    }
    p[0] = WAD - p[1..].iter().sum::<u128>();
    p
}

/// How far computed C can sit from the true C. One wei of ln or exp is b / 1e18 units of money, and the worst found,
/// on books chosen to sit just under a power of two, is 26 of those. Measured, not proved.
pub fn noise(depth: u128) -> u128 {
    32 * depth / WAD + 1
}
