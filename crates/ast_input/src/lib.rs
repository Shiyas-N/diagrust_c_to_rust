//! # ast_input — C source → Clang-AST types
//!
//! This crate provides the C Abstract Syntax Tree types and a Clang-based
//! parser that converts C source code into the internal representation.
//!
//! ## Module layout
//! - **types** — Core AST type definitions (`CProgram`, `CStatement`, …)
//! - **parser** — `ClangParser` which drives libclang
//! - **validator** — Lightweight structural validation of C source code

mod types;
mod parser;
mod validator;

// Re-export everything that dependent crates need
pub use types::*;
pub use parser::{ClangParser, SimpleParser, read_c_file};
pub use validator::InputValidator;
