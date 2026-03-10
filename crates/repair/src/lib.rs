//! # Automated Repair Engine
//!
//! Comprehensive rule-based repair driven by rustc diagnostics.
//!
//! ## Module layout
//! - **types** — Diagnostic types, repair actions, and the diagnostic parser
//! - **engine** — `RepairEngine` with all repair rules and helpers

pub mod types;
pub mod engine;

pub use types::*;
pub use engine::RepairEngine;
