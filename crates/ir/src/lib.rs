//! # Intermediate Representation (IR)
//!
//! Defines the IR types and provides the `IrBuilder` which converts
//! the C AST into a Rust-friendly intermediate representation.
//!
//! ## Module layout
//! - **types** — IR type definitions (`IrProgram`, `IrStatement`, `IrExpr`, …)
//! - **builder** — `IrBuilder` that converts C AST → IR

mod types;
mod builder;

pub use types::*;
pub use builder::IrBuilder;
