//! Structure-aware fuzz target: generated, always-valid Python programs, hunting Rust panics.
//!
//! libFuzzer's bytes are decoded by `arbitrary` into a `Program` tree (see
//! `src/generate.rs`), printed as Python (`src/printer.rs`) and run in Monty
//! (`src/harness.rs`). Because every program is valid syntax inside Monty's subset,
//! the fuzzer's time goes into the interpreter, not the parser. Python exceptions and
//! resource errors are expected; only panics are findings.
//!
//! Run with `cargo fuzz run --fuzz-dir crates/fuzz ast_panic --sanitizer none -a`;
//! see `crates/fuzz/README.md`.
#![no_main]

use libfuzzer_sys::fuzz_target;
use monty_fuzz::{ast::Program, harness};

/// Counts allocated bytes so Monty's `max_memory` limit is actually enforced.
#[global_allocator]
static ALLOC: monty_alloc::LimitedAllocator = monty_alloc::LimitedAllocator;

fuzz_target!(|program: Program| {
    harness::fuzz_one(&program);
});
