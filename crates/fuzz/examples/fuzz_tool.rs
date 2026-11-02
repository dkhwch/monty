//! Helper for the `ast_panic` fuzz target, usable without libFuzzer.
//!
//! ```text
//! cargo run -p monty-fuzz --example fuzz_tool -- gen [--count N] [--max-size BYTES] [--seed S] [--out DIR]
//! cargo run -p monty-fuzz --example fuzz_tool -- run FILE.py...
//! cargo run -p monty-fuzz --example fuzz_tool -- show INPUT...
//! ```
//!
//! - `gen` decodes pseudo-random byte strings into programs, runs each in Monty and
//!   prints how many were rejected, completed or raised. With `--out` it also writes
//!   the programs as `.py` files (e.g. to check them with CPython). Panics are caught,
//!   counted and saved, so this doubles as a plain random tester for comparison.
//! - `run` runs Python files under the same limits as the fuzz target. A panic is not
//!   caught: this is how to reproduce a finding outside the fuzzer.
//! - `show` prints the Python program a raw fuzzer input (corpus or crash file) decodes to.

use std::{
    collections::BTreeMap,
    env, fs,
    panic::{self, AssertUnwindSafe},
    path::{Path, PathBuf},
    process,
    time::Instant,
};

use arbitrary::{Arbitrary, Unstructured};
use monty_fuzz::{
    ast::Program,
    harness::{self, Outcome},
};

/// Same allocator as the fuzz target, so `max_memory` behaves identically.
#[global_allocator]
static ALLOC: monty_alloc::LimitedAllocator = monty_alloc::LimitedAllocator;

/// Dispatches to the subcommand.
fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("gen") => gen_command(&args[1..]),
        Some("run") => run_command(&args[1..]),
        Some("show") => show_command(&args[1..]),
        _ => Err("usage: fuzz_tool gen|run|show ... (see the doc comment in examples/fuzz_tool.rs)".to_owned()),
    };
    if let Err(message) = result {
        eprintln!("{message}");
        process::exit(2);
    }
}

/// Options of `gen`.
struct GenOptions {
    count: u64,
    max_size: usize,
    seed: u64,
    out: Option<PathBuf>,
}

/// Generates and runs random programs, then prints a summary.
fn gen_command(args: &[String]) -> Result<(), String> {
    let options = parse_gen_options(args)?;
    if let Some(out) = &options.out {
        fs::create_dir_all(out).map_err(|e| format!("cannot create {}: {e}", out.display()))?;
    }
    let mut rng = SplitMix64(options.seed);
    let mut summary = Summary::default();
    let started = Instant::now();
    for i in 0..options.count {
        let len = usize::try_from(rng.next() % options.max_size as u64).unwrap_or(0) + 1;
        let bytes: Vec<u8> = (0..len).map(|_| rng.next().to_le_bytes()[0]).collect();
        let Ok(program) = Program::arbitrary_take_rest(Unstructured::new(&bytes)) else {
            summary.undecodable += 1;
            continue;
        };
        let source = program.to_source();
        if let Some(out) = &options.out {
            let _ = fs::write(out.join(format!("prog_{i:06}.py")), &source);
        }
        summary.source_bytes += source.len() as u64;
        summary.max_source = summary.max_source.max(source.len());
        let run = panic::catch_unwind(AssertUnwindSafe(|| harness::run_source(source.clone())));
        if let Ok(outcome) = run {
            summary.add(i, &outcome, program.body.len());
        } else {
            summary.panics += 1;
            let path = options
                .out
                .clone()
                .unwrap_or_else(|| PathBuf::from("."))
                .join(format!("panic_{i:06}.py"));
            let _ = fs::write(&path, &source);
            eprintln!("PANIC on program {i}; source saved to {}", path.display());
        }
    }
    summary.print(options.count, started.elapsed().as_secs_f64());
    Ok(())
}

/// Parses `--count`, `--max-size`, `--seed` and `--out`.
fn parse_gen_options(args: &[String]) -> Result<GenOptions, String> {
    let mut options = GenOptions {
        count: 1000,
        max_size: 4096,
        seed: 1,
        out: None,
    };
    let mut iter = args.iter();
    while let Some(flag) = iter.next() {
        let value = iter.next().ok_or_else(|| format!("{flag} needs a value"))?;
        let number = || value.parse::<u64>().map_err(|e| format!("{flag}: {e}"));
        match flag.as_str() {
            "--count" => options.count = number()?,
            "--max-size" => options.max_size = usize::try_from(number()?.max(1)).unwrap_or(4096),
            "--seed" => options.seed = number()?,
            "--out" => options.out = Some(PathBuf::from(value)),
            _ => return Err(format!("unknown option {flag}")),
        }
    }
    Ok(options)
}

/// Runs `.py` files exactly as the fuzz target runs generated programs.
fn run_command(files: &[String]) -> Result<(), String> {
    if files.is_empty() {
        return Err("run: give at least one .py file".to_owned());
    }
    for file in files {
        let source = fs::read_to_string(file).map_err(|e| format!("{file}: {e}"))?;
        let outcome = harness::run_source(source);
        println!("{file}: {}", describe(&outcome));
    }
    Ok(())
}

/// Prints the program behind raw fuzzer inputs.
fn show_command(files: &[String]) -> Result<(), String> {
    if files.is_empty() {
        return Err("show: give at least one input file".to_owned());
    }
    for file in files {
        let bytes = fs::read(Path::new(file)).map_err(|e| format!("{file}: {e}"))?;
        match Program::arbitrary_take_rest(Unstructured::new(&bytes)) {
            Ok(program) => println!("# {file}\n{}", program.to_source()),
            Err(e) => println!("# {file}: does not decode ({e})"),
        }
    }
    Ok(())
}

/// One-line description of an outcome.
fn describe(outcome: &Outcome) -> String {
    match outcome {
        Outcome::Rejected(err) => format!("REJECTED {}", err.summary()),
        Outcome::Completed(value) => format!("completed, result {value:?}"),
        Outcome::Raised(err) => format!("raised {}", err.summary()),
    }
}

/// Aggregated results of `gen`.
#[derive(Default)]
struct Summary {
    undecodable: u64,
    rejected: u64,
    completed: u64,
    panics: u64,
    raised: BTreeMap<String, u64>,
    statements: u64,
    statements_ok: u64,
    source_bytes: u64,
    max_source: usize,
    first_rejections: Vec<String>,
}

impl Summary {
    /// Counts one program's outcome.
    fn add(&mut self, index: u64, outcome: &Outcome, top_level: usize) {
        match outcome {
            Outcome::Rejected(err) => {
                self.rejected += 1;
                if self.first_rejections.len() < 5 {
                    self.first_rejections
                        .push(format!("program {index}: {}", err.summary()));
                }
            }
            Outcome::Completed(value) => {
                self.completed += 1;
                self.statements += top_level as u64;
                self.statements_ok += i64::try_from(value)
                    .ok()
                    .and_then(|n| u64::try_from(n).ok())
                    .unwrap_or(0);
            }
            Outcome::Raised(err) => *self.raised.entry(format!("{:?}", err.exc_type())).or_default() += 1,
        }
    }

    /// Prints the summary table.
    fn print(&self, total: u64, seconds: f64) {
        let pct = |n: u64| 100.0 * n as f64 / total.max(1) as f64;
        println!(
            "programs:      {total} in {seconds:.1}s ({:.0}/s)",
            total as f64 / seconds.max(1e-9)
        );
        println!("undecodable:   {} ({:.2}%)", self.undecodable, pct(self.undecodable));
        println!("rejected:      {} ({:.2}%)", self.rejected, pct(self.rejected));
        println!("completed:     {} ({:.2}%)", self.completed, pct(self.completed));
        for (exc, n) in &self.raised {
            println!("raised {exc:<14} {n} ({:.2}%)", pct(*n));
        }
        println!("panics:        {}", self.panics);
        println!(
            "statements ok: {:.1}% of top-level statements in completed programs",
            100.0 * self.statements_ok as f64 / self.statements.max(1) as f64
        );
        println!(
            "source size:   avg {} B, max {} B",
            self.source_bytes / total.max(1),
            self.max_source
        );
        for rejection in &self.first_rejections {
            println!("  {rejection}");
        }
    }
}

/// Tiny deterministic PRNG, so `gen --seed N` always produces the same programs.
struct SplitMix64(u64);

impl SplitMix64 {
    /// The next pseudo-random number.
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
}
