//! The Fortis core runtime, implemented once (Delivery Programme 5.3): the exact semantics of the primitive
//! values that both the development VM and native code use, so that the two cannot drift apart.

pub mod big;
pub mod decimal;
pub mod failure;
pub mod float;
pub mod telemetry;
pub mod time;

pub use decimal::{DecError, Decimal, Rounding};
