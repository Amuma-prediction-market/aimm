//! AIMM, the Adaptive Information Market Maker, as plain Rust: the curve, the pot, the payout and the sell cap.
//! No dependencies and no floating point. The NEAR contract in `../contract` wraps this crate.
pub mod lmsr;
pub mod math;
pub mod question;

pub use lmsr::Pool;
pub use math::WAD;
pub use question::{AimmError, Fill, Question};
