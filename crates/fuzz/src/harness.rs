//! Runs a program in Monty the way the `ast_panic` fuzz target does.
//!
//! Settings come from environment variables, read once:
//! - `MONTY_FUZZ_FEED_MS`: execution time limit per program (default 100).
//! - `MONTY_FUZZ_MEMORY_MB`: soft memory limit per program (default 16).
//! - `MONTY_FUZZ_STRICT=1`: panic when Monty rejects a generated program. A rejection
//!   means a generator bug (or Monty refusing valid code), so this turns them into
//!   crashes libFuzzer can find. Only for checking the generator, not for bug hunting.
//! - `MONTY_FUZZ_STATS`: see [`crate::stats`].
//!
//! `max_memory` is only enforced when `monty_alloc::LimitedAllocator` is the global
//! allocator (see `limitations/resource_limits.md`); binaries using this module must
//! install it. No hard ceiling is armed: crossing one aborts the process, which
//! libFuzzer would report as a crash that is not a panic. libFuzzer's `-rss_limit_mb`
//! is the hard backstop instead and reports separately (`oom-*` files).

use std::{
    env,
    sync::{
        OnceLock,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use monty::MontyRun;
use monty_types::{
    BASELINE_MEMORY, CompileOptions, DateTimeSource, LIVE_MEMORY, MontyException, MontyObject, OsPolicy, PrintWriter,
    RandomSeed, RandomStart, ResourceLimits, ResourceTracker, SleepMode,
};

use crate::{ast::Program, stats};

/// Script name used for every program; it appears in tracebacks.
pub const SCRIPT_NAME: &str = "fuzz.py";

/// Harness settings, read once from the environment.
#[derive(Debug)]
pub struct Settings {
    pub feed_duration: Duration,
    pub max_memory: usize,
    pub strict: bool,
}

/// The settings for this process.
pub fn settings() -> &'static Settings {
    static SETTINGS: OnceLock<Settings> = OnceLock::new();
    SETTINGS.get_or_init(|| Settings {
        feed_duration: Duration::from_millis(env_number("MONTY_FUZZ_FEED_MS", 100)),
        max_memory: usize::try_from(env_number("MONTY_FUZZ_MEMORY_MB", 16)).unwrap_or(16) * 1024 * 1024,
        strict: env::var("MONTY_FUZZ_STRICT").is_ok_and(|v| v == "1"),
    })
}

/// What happened to one program.
#[derive(Debug)]
pub enum Outcome {
    /// `MontyRun::new` failed: a parse, compile or nesting error.
    Rejected(MontyException),
    /// The program ran to the end; holds its result (`_ok` for generated programs).
    Completed(MontyObject),
    /// An exception escaped: an uncatchable resource error, or something `except Exception` misses.
    Raised(MontyException),
}

/// One fuzzing iteration: print the program, run it, record stats, enforce strict mode.
///
/// # Panics
/// In strict mode, when Monty rejects the program. Otherwise only if Monty itself panics,
/// which is exactly what the fuzzer is looking for.
pub fn fuzz_one(program: &Program) {
    let source = program.to_source();
    let source_len = source.len();
    let strict_copy = settings().strict.then(|| source.clone());
    let outcome = run_source(source);
    stats::record(&outcome, program.body.len(), source_len);
    if let (Outcome::Rejected(err), Some(source)) = (&outcome, strict_copy) {
        panic!("MONTY_FUZZ_STRICT: generated program rejected: {err}\n--- source ---\n{source}");
    }
}

/// Compiles and runs `source` under the fuzzing limits and a deterministic OS policy.
#[must_use]
pub fn run_source(source: String) -> Outcome {
    let runner = match MontyRun::new(source, SCRIPT_NAME, vec![], CompileOptions::default()) {
        Ok(runner) => runner.with_os_policy(os_policy()),
        Err(err) => return Outcome::Rejected(err),
    };
    let mut runner = runner;
    start_memory_accounting();
    match runner.run(vec![], tracker(), PrintWriter::Disabled) {
        Ok(value) => Outcome::Completed(value),
        Err(err) => Outcome::Raised(err),
    }
}

/// Fresh resource limits for one run.
fn tracker() -> ResourceTracker {
    let settings = settings();
    ResourceTracker::new(
        ResourceLimits::default()
            .max_feed_duration(settings.feed_duration)
            .max_memory(settings.max_memory),
    )
}

/// Makes runs reproducible: a fixed clock and random seed, and sleeps that return at once
/// (`MontyRun::run` would otherwise really sleep, uncounted by the time limit).
fn os_policy() -> OsPolicy {
    OsPolicy {
        datetime: DateTimeSource::Fixed {
            unix_seconds: 1_700_000_000,
            microsecond: 123_456,
        },
        sleep: SleepMode::Zero,
        random_start: RandomStart::Seed(RandomSeed::Str("monty-fuzz".to_owned())),
        ..OsPolicy::default()
    }
}

/// Starts this run's memory budget at zero.
///
/// Monty measures usage as live allocator bytes minus `BASELINE_MEMORY`. Resetting the
/// baseline before each run charges a program only for what it allocates, whatever the
/// harness or earlier runs left allocated. Warns once if the allocator is missing.
fn start_memory_accounting() {
    let live = LIVE_MEMORY.load(Ordering::Relaxed);
    if live == 0 {
        static WARNED: AtomicBool = AtomicBool::new(false);
        if !WARNED.swap(true, Ordering::Relaxed) {
            eprintln!("monty-fuzz: monty_alloc is not the global allocator; max_memory is not enforced");
        }
    } else {
        BASELINE_MEMORY.store(live, Ordering::Relaxed);
    }
}

/// An unsigned number from the environment, or `default`.
fn env_number(name: &str, default: u64) -> u64 {
    env::var(name).ok().and_then(|v| v.parse().ok()).unwrap_or(default)
}
