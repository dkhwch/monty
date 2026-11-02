//! Structure-aware fuzzing support for Monty.
//!
//! The `ast_panic` fuzz target turns libFuzzer's bytes into a [`ast::Program`]
//! (via `arbitrary`), prints it as Python that is valid by construction, and
//! runs it in Monty. Only Rust panics count as findings; every Python-level or
//! resource error is expected and ignored.
//!
//! The code lives in this library rather than in the target file so the
//! `fuzz_tool` example can reuse it outside libFuzzer: for generating sample
//! programs, checking them, and reproducing crashes.

pub mod ast;
pub mod catalog;
mod generate;
pub mod harness;
pub mod printer;
pub mod stats;
