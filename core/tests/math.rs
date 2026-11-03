//! The fixed-point arithmetic against references that share no code with it: a big-integer multiply and divide built
//! here, and ln and exp worked to 120 digits in Python's `decimal` and rounded to the nearest wei.
mod common;
use aimm_core::math::{exp_wad, ln_wad, mul_div, mul_div_up, s_div_wad, s_mul_wad, EXP_FLOOR, IWAD, WAD};
use common::Rng;
use std::cmp::Ordering;
use std::panic::catch_unwind;

/// A natural number in 32-bit limbs, least significant first, with no zero limb on top.
#[derive(Clone, Debug, PartialEq)]
struct Big(Vec<u32>);

impl Big {
    fn of(x: u128) -> Big {
        Big((0..4).map(|k| (x >> (32 * k)) as u32).collect()).trimmed()
    }
    fn trimmed(mut self) -> Big {
        while self.0.last() == Some(&0) {
            self.0.pop();
        }
        self
    }
    fn times(&self, o: &Big) -> Big {
        let mut v = vec![0u32; self.0.len() + o.0.len() + 1];
        for (i, &a) in self.0.iter().enumerate() {
            let mut carry = 0u64;
            for (j, &b) in o.0.iter().enumerate() {
                let t = a as u64 * b as u64 + v[i + j] as u64 + carry;
                v[i + j] = t as u32;
                carry = t >> 32;
            }
            let mut k = i + o.0.len();
            while carry > 0 {
                let t = v[k] as u64 + carry;
                v[k] = t as u32;
                carry = t >> 32;
                k += 1;
            }
        }
        Big(v).trimmed()
    }
    fn cmp_to(&self, o: &Big) -> Ordering {
        self.0.len().cmp(&o.0.len()).then_with(|| self.0.iter().rev().cmp(o.0.iter().rev()))
    }
    fn less(&self, o: &Big) -> Big {
        let mut v = self.0.clone();
        let mut borrow = 0i64;
        for (i, x) in v.iter_mut().enumerate() {
            let t = *x as i64 - borrow - *o.0.get(i).unwrap_or(&0) as i64;
            *x = t as u32;
            borrow = (t < 0) as i64;
        }
        Big(v).trimmed()
    }
    fn plus_one(&self) -> Big {
        let mut v = self.0.clone();
        v.push(0);
        for x in v.iter_mut() {
            *x = x.wrapping_add(1);
            if *x != 0 {
                break;
            }
        }
        Big(v).trimmed()
    }
    fn doubled_plus(&self, bit: u32) -> Big {
        let mut v = Vec::with_capacity(self.0.len() + 1);
        let mut carry = bit;
        for &x in &self.0 {
            v.push(x << 1 | carry);
            carry = x >> 31;
        }
        v.push(carry);
        Big(v).trimmed()
    }
    /// Quotient and remainder, one bit at a time.
    fn over(&self, d: &Big) -> (Big, Big) {
        let (mut q, mut r) = (Big(vec![]), Big(vec![]));
        for k in (0..32 * self.0.len()).rev() {
            r = r.doubled_plus(self.0[k / 32] >> (k % 32) & 1);
            let bit = r.cmp_to(d) != Ordering::Less;
            if bit {
                r = r.less(d);
            }
            q = q.doubled_plus(bit as u32);
        }
        (q, r)
    }
    fn to_u128(&self) -> Option<u128> {
        (self.0.len() <= 4).then(|| self.0.iter().rev().fold(0, |x, &l| x << 32 | l as u128))
    }
}

/// What floor and ceil of a * b / d are, by the reference; None if the result does not fit in 128 bits.
fn reference(a: u128, b: u128, d: u128) -> (Option<u128>, Option<u128>) {
    let (q, r) = Big::of(a).times(&Big::of(b)).over(&Big::of(d));
    let up = if r.0.is_empty() { q.clone() } else { q.plus_one() };
    (q.to_u128(), up.to_u128())
}

#[test]
fn the_reference_is_right_on_sums_worked_by_hand() {
    // (2^64 + 1)(2^64 - 1) = 2^128 - 1, and that over 2^64 + 1 is 2^64 - 1 with nothing left
    assert_eq!(Big::of((1 << 64) + 1).times(&Big::of((1 << 64) - 1)), Big::of(u128::MAX));
    assert_eq!(reference((1 << 64) + 1, (1 << 64) - 1, (1 << 64) + 1), (Some((1 << 64) - 1), Some((1 << 64) - 1)));
    // 7 * 9 / 4 is 15.75
    assert_eq!(reference(7, 9, 4), (Some(15), Some(16)));
    // (2^128 - 1)^2 = 2^256 - 2^129 + 1, in limbs 1, 0, 0, 0, 0xfffffffe, 0xffffffff, 0xffffffff, 0xffffffff
    let square = Big::of(u128::MAX).times(&Big::of(u128::MAX));
    assert_eq!(square.0, vec![1, 0, 0, 0, 0xffff_fffe, u32::MAX, u32::MAX, u32::MAX]);
    assert_eq!(reference(u128::MAX, u128::MAX, 1), (None, None));
}

#[test]
fn mul_div_matches_the_reference_where_the_product_passes_128_bits() {
    let mut r = Rng(0x7a3f_5e11_c0de_0001);
    let (mut wide, mut top_bit) = (0, 0);
    for _ in 0..6000 {
        let (a, b) = (r.wide() >> r.below(40), r.wide() >> r.below(40));
        let hi = Big::of(a).times(&Big::of(b)).over(&Big::of(u128::MAX)).0.to_u128().unwrap();
        // the quotient fits exactly when d is above the high half of the product
        let d = match r.below(3) {
            0 => r.wide() | 1 << 127,
            1 => hi + 2 + r.wide() % (u128::MAX - hi - 2),
            _ => (hi + 2).max(r.wide() >> r.below(100)),
        };
        let (down, up) = reference(a, b, d);
        assert_eq!(Some(mul_div(a, b, d)), down, "{a} {b} {d}");
        assert_eq!(Some(mul_div_up(a, b, d)), up, "{a} {b} {d}");
        wide += (Big::of(a).times(&Big::of(b)).0.len() > 4) as u32;
        top_bit += (d >> 127 == 1) as u32;
    }
    assert!(wide > 5000 && top_bit > 1500, "{wide} products past 128 bits, {top_bit} divisors with the top bit set");
}

#[test]
fn mul_div_matches_the_reference_or_refuses_on_every_edge() {
    let edges: Vec<u128> = vec![0, 1, 2, 3, 7, 1 << 32, (1 << 63) - 1, 1 << 63, (1 << 64) - 1, 1 << 64, (1 << 64) + 1, WAD, 1 << 100,
        (1 << 127) - 1, 1 << 127, (1 << 127) + 1, u128::MAX - 1, u128::MAX];
    let (mut fit, mut refused) = (0, 0);
    for &a in &edges {
        for &b in &edges {
            for &d in &edges {
                let down = catch_unwind(|| mul_div(a, b, d)).ok();
                let up = catch_unwind(|| mul_div_up(a, b, d)).ok();
                let want = if d == 0 { (None, None) } else { reference(a, b, d) };
                assert_eq!((down, up), want, "{a} {b} {d}");
                fit += down.is_some() as u32;
                refused += down.is_none() as u32;
            }
        }
    }
    assert!(fit > 1000 && refused > 1000, "{fit} fit, {refused} refused");
}

#[test]
fn mul_div_up_adds_one_only_when_something_was_left_over() {
    assert_eq!(mul_div_up(10, 10, 5), 20);
    assert_eq!(mul_div_up(10, 10, 3), 34);
    assert_eq!(mul_div_up(0, 10, 3), 0);
    assert_eq!(mul_div_up(1, 1, u128::MAX), 1);
    assert_eq!(mul_div_up(1 << 127, 2, 1 << 127), 2);
    assert_eq!(mul_div_up((1 << 127) + 1, 2, 1 << 127), 3);
}

#[test]
#[should_panic(expected = "mul_div overflow")]
fn mul_div_refuses_a_quotient_of_exactly_2_to_the_128() {
    mul_div(1 << 127, 1 << 127, 1 << 126);
}

#[test]
fn mul_div_takes_the_largest_quotient_there_is() {
    assert_eq!(mul_div(u128::MAX, 1 << 64, 1 << 64), u128::MAX);
    // (2^43 - 1)(2^86 + 2^43 + 1) = 2^129 - 1, so a half of it is 2^128 - 1 and a half left over
    assert_eq!(mul_div((1 << 43) - 1, (1 << 86) + (1 << 43) + 1, 2), u128::MAX);
}

#[test]
#[should_panic(expected = "mul_div overflow")]
fn mul_div_up_refuses_to_round_past_the_largest_quotient() {
    mul_div_up((1 << 43) - 1, (1 << 86) + (1 << 43) + 1, 2);
}

#[test]
#[should_panic(expected = "division by zero")]
fn mul_div_refuses_a_zero_divisor() {
    mul_div(1, 1, 0);
}

#[test]
#[should_panic(expected = "division by zero")]
fn mul_div_refuses_a_zero_divisor_past_128_bits() {
    mul_div(u128::MAX, u128::MAX, 0);
}

#[test]
fn signed_products_round_toward_zero_with_all_four_signs() {
    let half = IWAD / 2;
    assert_eq!(s_mul_wad(3, half), 1);
    assert_eq!(s_mul_wad(-3, half), -1);
    assert_eq!(s_mul_wad(3, -half), -1);
    assert_eq!(s_mul_wad(-3, -half), 1);
    assert_eq!(s_mul_wad(-1, 1), 0);
    assert_eq!(s_mul_wad(0, -IWAD), 0);
    assert_eq!(s_div_wad(7, 2 * IWAD), 3);
    assert_eq!(s_div_wad(-7, 2 * IWAD), -3);
    assert_eq!(s_div_wad(7, -2 * IWAD), -3);
    assert_eq!(s_div_wad(-7, -2 * IWAD), 3);
    assert_eq!(s_div_wad(0, -1), 0);
}

#[test]
#[should_panic(expected = "signed overflow")]
fn s_mul_wad_refuses_a_product_past_i128() {
    s_mul_wad(i128::MAX, 2 * IWAD);
}

#[test]
#[should_panic(expected = "signed overflow")]
fn s_div_wad_refuses_a_quotient_past_i128() {
    s_div_wad(i128::MAX, IWAD / 2);
}

#[test]
#[should_panic(expected = "division by zero")]
fn s_div_wad_refuses_a_zero_divisor() {
    s_div_wad(1, 0);
}

/// x and ln(x / 1e18) * 1e18. The first half is every kind of number the code meets, the rest random sums of terms, the
/// argument of ln in C, from 1 to 16.
const LN: &[(i128, i128)] = &[
    (1, -41446531673892822312),
    (2, -40753384493332877003),
    (3, -40347919385224712621),
    (1000, -34538776394910685260),
    (1000000, -27631021115928548208),
    (1000000000, -20723265836946411156),
    (1000000000000, -13815510557964274104),
    (1000000000001, -13815510557963274104),
    (1000000000000000, -6907755278982137052),
    (10000000000000000, -4605170185988091368),
    (50000000000000000, -2995732273553990993),
    (100000000000000000, -2302585092994045684),
    (200000000000000000, -1609437912434100375),
    (250000000000000000, -1386294361119890619),
    (300000000000000000, -1203972804325935993),
    (333333333333333333, -1098612288668109692),
    (400000000000000000, -916290731874155065),
    (500000000000000000, -693147180559945309),
    (499999999999999999, -693147180559945311),
    (500000000000000001, -693147180559945307),
    (600000000000000000, -510825623765990683),
    (700000000000000000, -356674943938732379),
    (750000000000000000, -287682072451780927),
    (800000000000000000, -223143551314209756),
    (900000000000000000, -105360515657826301),
    (990000000000000000, -10050335853501441),
    (999999000000000000, -1000000500000),
    (999999999999999999, -1),
    (1000000000000000000, 0),
    (1000000000000000001, 1),
    (1000001000000000000, 999999500000),
    (1010000000000000000, 9950330853168083),
    (1100000000000000000, 95310179804324860),
    (1500000000000000000, 405465108108164382),
    (1999999999999999999, 693147180559945309),
    (2000000000000000000, 693147180559945309),
    (2000000000000000001, 693147180559945310),
    (2500000000000000000, 916290731874155065),
    (2718281828459045235, 1000000000000000000),
    (3000000000000000000, 1098612288668109691),
    (3700000000000000000, 1308332819650178760),
    (4000000000000000000, 1386294361119890619),
    (7000000000000000000, 1945910149055313305),
    (8000000000000000000, 2079441541679835928),
    (12500000000000000000, 2525728644308255440),
    (15990000000000000000, 2771963526845862863),
    (16000000000000000000, 2772588722239781238),
    (1000000000000000000000, 6907755278982137052),
    (1000000000000000000000000, 13815510557964274104),
    (1000000000000000000000000000000, 27631021115928548208),
    (1000000000000000000000000000000000000, 41446531673892822312),
    (100000000000000000000000000000000000000, 46051701859880913680),
    (170141183460469231731687303715884105727, 46583160257220231984),
    (2223725163901051235, 799183791369401224),
    (12624963402158238667, 2535676076318270488),
    (3940616870255189056, 1371337277109273378),
    (8648719065386770608, 2157411225051747763),
    (15062307532474499100, 2712195433242231144),
    (12985304782249288717, 2563818316710586993),
    (15202904163762105680, 2721486473007344556),
    (8475801933202892818, 2137215272176775695),
    (10517153576612642625, 2353007598132864923),
    (9612372148048991724, 2263051034150225904),
    (4556055152294049773, 1516457150931781980),
    (4434845031770573912, 1489492673047462289),
    (11316275349366517802, 2426241985890413797),
    (11422836768063608776, 2435614576889017868),
    (9162548674753354274, 2215124379586474125),
    (6975838771724778106, 1942452574488216267),
    (4675687398236461922, 1542376188987689622),
    (13386200305461311800, 2594224348369072086),
    (10243245782528516451, 2326618540336733469),
    (12683724974360666708, 2540319673561858520),
    (2904126003900908794, 1066132485794280900),
    (12812024966798915455, 2550384180449268020),
    (10709052196042961774, 2371089383442060369),
    (13642324870040011890, 2613177082908810559),
    (1070046905058839375, 67702484016611561),
    (9412994824392494807, 2242091162728800585),
    (11052757402701799829, 2402679935540202735),
    (8926644496217767009, 2189040567971716848),
    (12601813088066107956, 2533840699483676022),
    (15062333945133203056, 2712197186800607837),
    (13233516277839226923, 2582752723451333155),
    (13507267065552351830, 2603227841764317880),
    (12000627119340788730, 2484958908367562682),
    (6420963230146858567, 1859568142296037918),
    (11393601735184535986, 2433051946540380248),
    (10826593195021984843, 2382005440428263572),
    (15569747766876231863, 2745329785770963390),
    (6283648684806118328, 1837950812529814230),
    (12725522176982754459, 2543609597113925987),
    (6006340529020393069, 1792815666092453411),
];

/// x and exp(x / 1e18) * 1e18.
const EXP: &[(i128, i128)] = &[
    (0, 1000000000000000000),
    (-1, 999999999999999999),
    (-2, 999999999999999998),
    (-1000, 999999999999999000),
    (-1000000, 999999999999000000),
    (-1000000000, 999999999000000000),
    (-1000000000000, 999999000000500000),
    (-1000000000000000, 999000499833374992),
    (-10000000000000000, 990049833749168054),
    (-100000000000000000, 904837418035959573),
    (-300000000000000000, 740818220681717866),
    (-340000000000000000, 711770322762609715),
    (-350000000000000000, 704688089718713434),
    (-500000000000000000, 606530659712633424),
    (-690000000000000000, 501576069066055534),
    (-693147180559945309, 500000000000000000),
    (-693147180559945310, 500000000000000000),
    (-700000000000000000, 496585303791409515),
    (-1000000000000000000, 367879441171442322),
    (-1039720770839917964, 353553390593273762),
    (-2000000000000000000, 135335283236612692),
    (-2500000000000000000, 82084998623898795),
    (-5000000000000000000, 6737946999085467),
    (-10000000000000000000, 45399929762485),
    (-20000000000000000000, 2061153622),
    (-30000000000000000000, 93576),
    (-40000000000000000000, 4),
    (-40900000000000000000, 2),
    (-40999999999999999999, 2),
    (-15356026935343820829, 214270428453),
    (-35700774286309958158, 313),
    (-24868929552603068436, 15832924),
    (-20978337253133018471, 774861158),
    (-22681714671279242342, 141077085),
    (-35116747010217031457, 561),
    (-18512248025533172934, 9124999196),
    (-1724829412479174268, 178203448464652752),
    (-26903814759747854502, 2069292),
    (-34813037829987439496, 760),
    (-38804541031649358381, 14),
    (-15419613757870462523, 201069793489),
    (-21702452554716417437, 375616457),
    (-39851835748818503691, 5),
    (-7040606024688903065, 875595768784130),
    (-26442269350976649847, 3282981),
    (-35048311114199294377, 601),
    (-28601831263738212176, 378776),
    (-9331199604472067419, 88615869316094),
    (-36443437603842704802, 149),
    (-1154741402023379053, 315139020680291567),
    (-33445384639080553451, 2984),
    (-18200178525585418684, 12467026904),
    (-7053182471563560929, 864652840923593),
    (-447648890155205694, 639129049142459636),
    (-17302022476481341505, 30607447462),
    (-33359397490505904896, 3252),
    (-36598238971995378781, 128),
    (-35300278562321941296, 467),
    (-9461908324486470063, 77758061725958),
    (-30292934302883028329, 69815),
    (-38695883699454658237, 16),
    (-12826817533308926348, 2687719392809),
    (-31154963731358549999, 29483),
    (-34665306009454913438, 881),
    (-14018981737448418344, 815893718462),
    (-15243795965106171959, 239719591577),
    (-3686275531397177245, 25065182896701130),
    (-3319672592651175065, 36164670394766757),
    (-6388062212459229881, 1681511458326057),
];

#[test]
fn ln_is_within_30_wei_of_the_exact_value() {
    // SPEC section 5: a few parts in 1e17
    for &(x, want) in LN {
        assert!(ln_wad(x).abs_diff(want) <= 30, "ln of {x}: got {}, want {want}", ln_wad(x));
    }
}

#[test]
fn exp_is_within_5_wei_of_the_exact_value() {
    for &(x, want) in EXP {
        assert!(exp_wad(x).abs_diff(want) <= 5, "exp of {x}: got {}, want {want}", exp_wad(x));
    }
}

#[test]
fn ln_is_exact_at_one_and_changes_sign_there() {
    assert_eq!(ln_wad(IWAD), 0);
    assert!(ln_wad(IWAD - 1) < 0);
    assert!(ln_wad(IWAD + 1) >= 0);
}

#[test]
fn ln_never_falls_as_x_rises() {
    let mut r = Rng(0x1357_9bdf_0246_8ace);
    for _ in 0..3000 {
        let x = (r.log_between(1, 1 << 126)) as i128;
        let step = 1 + r.below(1000) as i128;
        assert!(ln_wad(x + step) >= ln_wad(x), "ln fell between {x} and {}", x + step);
    }
}

#[test]
fn exp_at_the_ends_of_its_range() {
    assert_eq!(exp_wad(0), IWAD);
    assert_eq!(exp_wad(EXP_FLOOR), 0);
    assert_eq!(exp_wad(EXP_FLOOR - 1), 0);
    assert_eq!(exp_wad(i128::MIN / 2), 0);
    assert!(exp_wad(EXP_FLOOR + 1) > 0, "a term just above the floor still counts");
}

#[test]
fn ln_undoes_exp() {
    for k in 1..=40 {
        let x = -(k as i128) * IWAD;
        let e = exp_wad(x);
        // one wei of a term of size e is 1e18 / e wei of its ln
        let allowed = 40 + WAD as i128 / e;
        assert!((ln_wad(e) - x).abs() <= allowed, "ln(exp({x})) = {}", ln_wad(e));
    }
}

#[test]
#[should_panic(expected = "ln of a non-positive number")]
fn ln_refuses_zero() {
    ln_wad(0);
}

#[test]
#[should_panic(expected = "ln of a non-positive number")]
fn ln_refuses_a_negative_number() {
    ln_wad(-1);
}

#[test]
#[should_panic(expected = "exp_wad takes a non-positive exponent")]
fn exp_refuses_a_positive_exponent() {
    exp_wad(1);
}
