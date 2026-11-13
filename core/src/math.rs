//! Fixed-point arithmetic in WAD (1e18), with no floating point and no dependencies.
//!
//! `mul_div` takes a 256-bit intermediate, so `a * b / d` never overflows when the result fits in 128 bits.
//! `ln_wad` and `exp_wad` are accurate to a few parts in 1e17, far inside the one part in 1e9 to which the core must
//! match the vectors in `tests/vectors/`. A Python reference model, which is not in this repository, produced them, and
//! they are kept as fixed data.

pub const WAD: u128 = 1_000_000_000_000_000_000;
pub const IWAD: i128 = WAD as i128;
/// ln 2 at scale 1e36, so that k * ln 2 stays exact to a wei for any shift k the code uses.
const LN2_E36: i128 = 693_147_180_559_945_309_417_232_121_458_176_568;
/// ln 2 in WAD, rounded to the nearest wei.
const LN2: i128 = 693_147_180_559_945_309;
/// Below this exponent `exp_wad` is taken as 0.
pub const EXP_FLOOR: i128 = -41 * IWAD;

/// The full 256-bit product of two 128-bit numbers, as (high, low).
fn full_mul(a: u128, b: u128) -> (u128, u128) {
    let (a1, a0) = (a >> 64, a & u64::MAX as u128);
    let (b1, b0) = (b >> 64, b & u64::MAX as u128);
    let p00 = a0 * b0;
    let p01 = a0 * b1;
    let p10 = a1 * b0;
    let p11 = a1 * b1;
    let mid = (p00 >> 64) + (p01 & u64::MAX as u128) + (p10 & u64::MAX as u128);
    let lo = (p00 & u64::MAX as u128) | (mid << 64);
    let hi = p11 + (p01 >> 64) + (p10 >> 64) + (mid >> 64);
    (hi, lo)
}

/// (hi * 2^128 + lo) / d and the remainder. Panics if d is 0 or the quotient does not fit in 128 bits.
fn div_256(hi: u128, lo: u128, d: u128) -> (u128, u128) {
    assert!(d != 0, "division by zero");
    if hi == 0 {
        return (lo / d, lo % d);
    }
    assert!(hi < d, "mul_div overflow");
    let mut q: u128 = 0;
    let mut r: u128 = hi;
    for i in (0..128).rev() {
        let carry = r >> 127;
        r = (r << 1) | ((lo >> i) & 1);
        if carry == 1 || r >= d {
            r = r.wrapping_sub(d);
            q |= 1 << i;
        }
    }
    (q, r)
}

/// floor(a * b / d).
pub fn mul_div(a: u128, b: u128, d: u128) -> u128 {
    let (hi, lo) = full_mul(a, b);
    div_256(hi, lo, d).0
}

/// ceil(a * b / d).
pub fn mul_div_up(a: u128, b: u128, d: u128) -> u128 {
    let (hi, lo) = full_mul(a, b);
    let (q, r) = div_256(hi, lo, d);
    if r == 0 { q } else { q.checked_add(1).expect("mul_div overflow") }
}

/// a * b / d for signed a, b and positive d, rounded toward zero.
fn s_mul_div(a: i128, b: i128, d: u128) -> i128 {
    let neg = (a < 0) != (b < 0);
    let m = mul_div(a.unsigned_abs(), b.unsigned_abs(), d);
    let m = i128::try_from(m).expect("signed overflow");
    if neg { -m } else { m }
}

/// a * b / WAD, rounded toward zero (solady's `sMulWad`).
pub fn s_mul_wad(a: i128, b: i128) -> i128 {
    s_mul_div(a, b, WAD)
}

/// a * WAD / b, rounded toward zero (solady's `sDivWad`). b must not be 0.
pub fn s_div_wad(a: i128, b: i128) -> i128 {
    assert!(b != 0, "division by zero");
    let neg = (a < 0) != (b < 0);
    let m = mul_div(a.unsigned_abs(), WAD, b.unsigned_abs());
    let m = i128::try_from(m).expect("signed overflow");
    if neg { -m } else { m }
}

/// ln(x / WAD) in WAD, for x > 0.
pub fn ln_wad(x: i128) -> i128 {
    assert!(x > 0, "ln of a non-positive number");
    let x = x as u128;
    // Write x = 2^k * y with y in [WAD, 2 WAD).
    let mut k: i32 = 0;
    let mut y = x;
    if y >= 2 * WAD {
        let shift = (127 - y.leading_zeros()) as i32 - (127 - WAD.leading_zeros()) as i32;
        k = shift;
        y >>= shift;
        while y >= 2 * WAD {
            y >>= 1;
            k += 1;
        }
    }
    while y < WAD {
        y <<= 1;
        k -= 1;
    }
    // ln y = 2 atanh(z), z = (y - 1) / (y + 1), z in [0, 1/3).
    let z = mul_div(y - WAD, WAD, y + WAD);
    let z2 = mul_div(z, z, WAD);
    let mut term = z;
    let mut sum = z;
    let mut n: u128 = 1;
    while term > 0 {
        term = mul_div(term, z2, WAD);
        n += 2;
        sum += term / n;
    }
    let ln_y = 2 * sum as i128;
    let k_ln2 = (k as i128 * LN2_E36) / IWAD; // k ln 2 in WAD, rounded toward zero
    k_ln2 + ln_y
}

/// exp(x / WAD) in WAD, for x <= 0. Returns 0 at or below `EXP_FLOOR`.
pub fn exp_wad(x: i128) -> i128 {
    assert!(x <= 0, "exp_wad takes a non-positive exponent");
    if x <= EXP_FLOOR {
        return 0;
    }
    // x = k ln 2 + r with k = round(x / ln 2) <= 0 and |r| <= ln 2 / 2.
    let k = (2 * x + LN2).div_euclid(2 * LN2);
    let r = x - (k * LN2_E36) / IWAD;
    // exp(r) by its Taylor series; |r| < 0.35, so 25 terms are far more than enough.
    let mut term: i128 = IWAD;
    let mut sum: i128 = IWAD;
    let mut i: i128 = 1;
    while term != 0 && i < 40 {
        term = s_mul_wad(term, r) / i;
        sum += term;
        i += 1;
    }
    let shift = (-k) as u32;
    if shift >= 127 { 0 } else { sum >> shift }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn to_f(x: i128) -> f64 {
        x as f64 / 1e18
    }

    #[test]
    fn mul_div_matches_exact_arithmetic() {
        // products that overflow 128 bits, checked against exact values computed by hand-split arithmetic
        let a: u128 = 340_282_366_920_938_463_463_374_607_431_768_211_455; // 2^128 - 1
        assert_eq!(mul_div(a, a, a), a);
        assert_eq!(mul_div(a, 3, 3), a);
        assert_eq!(mul_div(1 << 100, 1 << 100, 1 << 90), 1 << 110);
        assert_eq!(mul_div_up(10, 10, 3), 34);
        assert_eq!(mul_div(10, 10, 3), 33);
        assert_eq!(mul_div(WAD * 7, WAD * 11, WAD), WAD * 77);
        let mut s: u128 = 0x9e37_79b9_7f4a_7c15;
        for _ in 0..20_000 {
            s ^= s << 13; s ^= s >> 7; s ^= s << 17;
            let a = s >> (s % 64);
            let b = (s.rotate_left(29)) >> 70;
            let d = ((s.rotate_left(51)) >> 66) | 1;
            if let Some(prod) = a.checked_mul(b) { assert_eq!(mul_div(a, b, d), prod / d, "{a} {b} {d}"); }
        }
    }

    #[test]
    fn ln_is_accurate() {
        for &v in &[1e-12, 1e-6, 0.01, 0.1, 0.5, 0.999999, 1.0, 1.000001, 1.5, 2.0, 3.0, 16.0, 1e6, 1e12, 1e18] {
            let x = (v * 1e18) as i128;
            let got = to_f(ln_wad(x));
            let want = (x as f64 / 1e18).ln();
            assert!((got - want).abs() <= 1e-15 * want.abs().max(1.0), "ln {v}: {got} vs {want}");
        }
        assert_eq!(ln_wad(IWAD), 0);
    }

    #[test]
    fn exp_is_accurate() {
        for &v in &[0.0, -1e-9, -0.001, -0.3, -0.5, -0.69, -1.0, -2.5, -10.0, -20.0, -30.0, -40.9] {
            let x = (v * 1e18) as i128;
            let got = exp_wad(x) as f64;
            let want = (x as f64 / 1e18).exp() * 1e18;
            assert!((got - want).abs() <= 1e-15 * want + 2.0, "exp {v}: {got} vs {want}");
        }
        assert_eq!(exp_wad(0), IWAD);
        assert_eq!(exp_wad(EXP_FLOOR), 0);
    }

    #[test]
    fn ln_and_exp_invert() {
        for &v in &[-0.01, -1.0, -5.0, -15.0] {
            let x = (v * 1e18) as i128;
            let back = ln_wad(exp_wad(x));
            assert!((back - x).abs() < 1_000 * (1.0 / (v as f64).exp()) as i128 + 1_000, "{v}");
        }
    }
}
