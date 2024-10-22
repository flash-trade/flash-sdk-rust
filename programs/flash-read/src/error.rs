//! Error types

use anchor_lang::prelude::*;

#[error_code]
pub enum CompError {
    #[msg("Overflow in arithmetic operation")]
    MathOverflow,
    #[msg("Exponent mismatch in arithmetic operation")]
    ExponentMismatch,
}
