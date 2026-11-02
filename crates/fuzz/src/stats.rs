//! Optional counters for judging the generator: how many programs parse, finish, and how
//! many of their top-level statements complete.
//!
//! Enabled by `MONTY_FUZZ_STATS`: `1` prints a line to stderr every 10 seconds, any other
//! value is a file path to append the lines to. Use a file with libFuzzer's `-fork` mode,
//! where each worker's stderr goes to a log the parent does not show. Lines carry the
//! process id, and each worker counts only its own programs. Disabled, recording costs
//! one cached check.

use std::{
    env,
    fs::OpenOptions,
    io::Write,
    process,
    sync::{
        OnceLock,
        atomic::{AtomicU64, Ordering},
    },
    time::Instant,
};

use monty_types::ExcType;

use crate::harness::Outcome;

/// Seconds between two printed lines.
const REPORT_INTERVAL_SECS: u64 = 10;

/// Where enabled stats go.
enum Sink {
    Stderr,
    File(String),
}

/// Totals since the process started.
#[derive(Default)]
struct Counters {
    programs: AtomicU64,
    rejected: AtomicU64,
    completed: AtomicU64,
    timeout: AtomicU64,
    memory: AtomicU64,
    recursion: AtomicU64,
    other_error: AtomicU64,
    /// Top-level statements of completed programs, and how many of them completed.
    statements: AtomicU64,
    statements_ok: AtomicU64,
    source_bytes: AtomicU64,
}

static COUNTERS: OnceLock<Counters> = OnceLock::new();
static START: OnceLock<Instant> = OnceLock::new();
static NEXT_REPORT: AtomicU64 = AtomicU64::new(REPORT_INTERVAL_SECS);

/// Records one program's outcome, printing a summary when one is due.
pub fn record(outcome: &Outcome, top_level_statements: usize, source_len: usize) {
    let Some(sink) = sink() else { return };
    let c = COUNTERS.get_or_init(Counters::default);
    let start = START.get_or_init(Instant::now);
    c.programs.fetch_add(1, Ordering::Relaxed);
    c.source_bytes.fetch_add(source_len as u64, Ordering::Relaxed);
    match outcome {
        Outcome::Rejected(_) => c.rejected.fetch_add(1, Ordering::Relaxed),
        Outcome::Completed(value) => {
            c.statements.fetch_add(top_level_statements as u64, Ordering::Relaxed);
            let ok = i64::try_from(value)
                .ok()
                .and_then(|n| u64::try_from(n).ok())
                .unwrap_or(0);
            c.statements_ok.fetch_add(ok, Ordering::Relaxed);
            c.completed.fetch_add(1, Ordering::Relaxed)
        }
        Outcome::Raised(err) => match err.exc_type() {
            ExcType::TimeoutError => c.timeout.fetch_add(1, Ordering::Relaxed),
            ExcType::MemoryError => c.memory.fetch_add(1, Ordering::Relaxed),
            ExcType::RecursionError => c.recursion.fetch_add(1, Ordering::Relaxed),
            _ => c.other_error.fetch_add(1, Ordering::Relaxed),
        },
    };

    let elapsed = start.elapsed().as_secs();
    let due = NEXT_REPORT.load(Ordering::Relaxed);
    if elapsed >= due
        && NEXT_REPORT
            .compare_exchange(
                due,
                elapsed + REPORT_INTERVAL_SECS,
                Ordering::Relaxed,
                Ordering::Relaxed,
            )
            .is_ok()
    {
        report(sink, c, elapsed);
    }
}

/// The configured sink, or `None` when stats are off.
fn sink() -> Option<&'static Sink> {
    static SINK: OnceLock<Option<Sink>> = OnceLock::new();
    SINK.get_or_init(|| match env::var("MONTY_FUZZ_STATS") {
        Ok(v) if v == "1" => Some(Sink::Stderr),
        Ok(v) if !v.is_empty() && v != "0" => Some(Sink::File(v)),
        _ => None,
    })
    .as_ref()
}

/// Writes one summary line.
fn report(sink: &Sink, c: &Counters, elapsed: u64) {
    let programs = c.programs.load(Ordering::Relaxed).max(1);
    let pct = |n: &AtomicU64| 100.0 * n.load(Ordering::Relaxed) as f64 / programs as f64;
    let statements = c.statements.load(Ordering::Relaxed).max(1);
    let line = format!(
        "[ast_panic pid={} t={elapsed}s] programs={programs} ({}/s) parsed={:.2}% completed={:.1}% \
         timeout={:.1}% memory={:.1}% recursion={:.1}% other_err={:.1}% stmt_ok={:.1}% avg_src={}B\n",
        process::id(),
        programs / elapsed.max(1),
        100.0 - pct(&c.rejected),
        pct(&c.completed),
        pct(&c.timeout),
        pct(&c.memory),
        pct(&c.recursion),
        pct(&c.other_error),
        100.0 * c.statements_ok.load(Ordering::Relaxed) as f64 / statements as f64,
        c.source_bytes.load(Ordering::Relaxed) / programs,
    );
    match sink {
        Sink::Stderr => eprint!("{line}"),
        Sink::File(path) => {
            if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) {
                let _ = file.write_all(line.as_bytes());
            }
        }
    }
}
