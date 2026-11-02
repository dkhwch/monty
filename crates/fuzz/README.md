# monty-fuzz

Fuzz targets for Monty, built with [cargo-fuzz](https://github.com/rust-fuzz/cargo-fuzz) (libFuzzer).
All three hunt Rust panics: Python exceptions and resource-limit errors are expected and ignored.

| Target | Input | Mostly exercises |
|---|---|---|
| `string_input_panic` | raw bytes as Python source | the parser (almost every input is invalid syntax) |
| `tokens_input_panic` | random sequences of Python tokens | the parser |
| `ast_panic` | a generated syntax tree, printed as valid Python | the interpreter: VM, builtins, types, stdlib modules |

## How `ast_panic` works

```text
libFuzzer bytes ──arbitrary──▶ Program tree ──printer──▶ Python source ──▶ MontyRun::new + run
  (mutated by        (src/generate.rs)       (src/printer.rs)              (src/harness.rs)
   coverage feedback)
```

The bytes are not Python: they are a recipe, every byte picks something (a statement kind, an operator, a builtin, a literal).
Any byte string decodes to some program, and every program is valid Python inside Monty's subset, so libFuzzer's mutations change *what the program does* instead of breaking its syntax.

Validity is built in at three levels:

- **Grammar**: the tree types can only express valid constructs; the printer handles indentation, `pass` for empty blocks, escaping (output is pure ASCII) and precedence-correct parentheses.
- **Compile rules**: the generator tracks context, so `break`/`continue` only appear in loops, `return` only in functions, `await` only in `async def`, `global`/`nonlocal` only as a function's first statement, no walrus in comprehensions or lambdas, only the last `match` case may be irrefutable, and so on.
- **Monty's subset**: constructs Monty rejects at compile time (`del`, `yield`, class bases, method decorators, `async for`/`with`, `@=`, ...) cannot be represented, and nesting stays under Monty's limit of 30 levels in debug builds (`MAX_NESTING_DEPTH` in `crates/monty/src/parse.rs`).

To get past the first line, every program starts with a prologue that imports the modules it uses and defines `v0`..`v6` with different types (`1`, `'ab'`, `[1, 2]`, `{'a': 1}`, `(1, 'x')`, `b'xy'`, an instance), plus placeholder functions `f0`..`f2` and classes `C0`, `C1` that accept any arguments.
Each top-level statement is wrapped in `try: ... except Exception: pass`, so one error only skips that statement (resource errors stay uncatchable).
Builtins, methods and module functions come from `src/catalog.rs`, extracted from Monty's source; anything that would suspend to the host (`open`, filesystem, environment) is left out, because `MontyRun::run` cannot serve it.
Loops run at most 8 iterations (a hidden `_gN` counter), sizes and nesting are capped, and `eval`/`exec` nest at most twice.

Runs are deterministic: a fixed clock, a fixed random seed, and sleeps that return at once (`OsPolicy` in `src/harness.rs`).
The one exception is the execution-time limit, which is wall-clock: a program near the limit can finish in one run and time out in another.

## Setup

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh   # Rust
rustup toolchain install nightly --component llvm-tools-preview   # llvm-tools: coverage reports
sudo apt install build-essential                                  # g++, to compile libFuzzer
cargo install cargo-fuzz
```

## Build

From the repository root:

```bash
cargo +nightly fuzz build --fuzz-dir crates/fuzz ast_panic --sanitizer none -a
```

- `--sanitizer none`: no AddressSanitizer. We hunt panics, not memory errors; libfuzzer-sys turns every panic into an abort that libFuzzer records. Coverage instrumentation is always on regardless.
- `-a`: debug assertions and integer overflow checks on, in an optimized build. Without it, `debug_assert!`s and arithmetic overflows (which wrap silently in release) would never panic.

## Run

```bash
cargo +nightly fuzz run --fuzz-dir crates/fuzz ast_panic --sanitizer none -a -- \
    -timeout=5 -rss_limit_mb=2048 -fork=7 -ignore_crashes=1 -ignore_timeouts=1 -ignore_ooms=1
```

Everything after `--` goes to libFuzzer:

| Flag | Meaning |
|---|---|
| `-timeout=5` | an input running over 5 s is saved as `timeout-*`. Monty's own limit is 100 ms, so this means Monty's limit failed to stop something. |
| `-rss_limit_mb=2048` | a process above 2 GB resident memory is saved as `oom-*` (the hard memory backstop, see below). |
| `-fork=7` | 7 worker processes sharing one corpus; leave a core for the parent. |
| `-ignore_crashes=1` | keep fuzzing after a crash (timeouts and OOMs are ignored by default in fork mode). |
| `-max_total_time=N` | stop after N seconds (optional). |
| `-len_control=0 -max_len=4096` | use full-size inputs from the start instead of letting libFuzzer grow them slowly (optional; bigger programs per run, fewer runs per second). |

Run it inside `tmux` so it survives a dropped SSH session.
Ctrl-C stops it; running the same command again continues from the saved corpus (`crates/fuzz/corpus/ast_panic/`).
Findings land in `crates/fuzz/artifacts/ast_panic/` as `crash-*` (panics), `timeout-*` and `oom-*`.

Reading the status lines: `cov` is the number of covered code edges (it rises while the fuzzer finds new code, a long plateau means it is stuck), `ft` counts finer-grained features (edge plus hit-count bucket), `corp` is the corpus size, `exec/s` the speed.
`cov` includes the generator's own code, not only Monty's.

### Environment variables

| Variable | Default | Effect |
|---|---|---|
| `MONTY_FUZZ_STATS` | off | `1`: print stats to stderr every 10 s; a file path: append them there (use this with `-fork`, whose workers' stderr is not shown) |
| `MONTY_FUZZ_STRICT` | off | `1`: panic when Monty rejects a generated program (generator self-check, not for bug hunting) |
| `MONTY_FUZZ_FEED_MS` | `100` | execution time limit per program |
| `MONTY_FUZZ_MEMORY_MB` | `16` | soft memory limit per program |

A stats line looks like this (each fork worker writes its own):

```text
[ast_panic pid=85867 t=150s] programs=55612 (370/s) parsed=100.00% completed=99.6% timeout=0.0% memory=0.3% recursion=0.0% other_err=0.0% stmt_ok=68.9% avg_src=1265B
```

`parsed` is the share `MontyRun::new` accepts, `completed` the share that ran to the end, `timeout`/`memory`/`recursion`/`other_err` how the rest ended, and `stmt_ok` the share of top-level statements that finished without an exception (the program's final `_ok` counter).

### Resource limits

The existing targets set `max_memory`, but it is only enforced when `monty-alloc` is the global allocator (`limitations/resource_limits.md`): Monty measures live allocator bytes minus a baseline, and without the allocator both read zero.
`ast_panic` installs `monty_alloc::LimitedAllocator` and resets the baseline before every run, so each program gets its own 16 MiB soft limit and Monty's `MemoryError` paths are actually exercised.
It deliberately arms no hard ceiling: crossing one aborts the process, which libFuzzer would record as a crash that is not a panic.
libFuzzer's `-rss_limit_mb` is the hard backstop instead, and it reports separately (`oom-*`).
Note that `-malloc_limit_mb` does nothing here: libFuzzer only installs its malloc hooks when a sanitizer runtime is present.

## Checking the generator

`examples/fuzz_tool.rs` runs the generator without libFuzzer:

```bash
cargo build -p monty-fuzz --example fuzz_tool
./target/debug/examples/fuzz_tool gen --count 5000 --out /tmp/progs   # decode random bytes, run, summarize
python3 -I crates/fuzz/scripts/check_syntax.py /tmp/progs               # CPython compile() on every program
```

`gen` reports how many programs Monty rejected (should be 0), completed or raised, and catches panics (it doubles as a plain random tester).
`check_syntax.py` is an independent check that the programs are valid Python.
For a check under libFuzzer itself, run a few minutes with `MONTY_FUZZ_STRICT=1`: any rejected program becomes a crash.

Results at the time of writing: 5000 random programs, 0 rejected, 99.6% completed, 0 CPython syntax errors; under libFuzzer, 100.00% parsed and about 70% of top-level statements complete (failures are mostly type mismatches, which the generator allows on purpose since error paths are code worth fuzzing too).

## Coverage report

```bash
cargo +nightly fuzz coverage --fuzz-dir crates/fuzz ast_panic --sanitizer none -a
LLVM_COV=$(rustc +nightly --print sysroot)/lib/rustlib/x86_64-unknown-linux-gnu/bin/llvm-cov
BIN=target/x86_64-unknown-linux-gnu/coverage/x86_64-unknown-linux-gnu/release/ast_panic
PROF=crates/fuzz/coverage/ast_panic/coverage.profdata
$LLVM_COV report $BIN -instr-profile=$PROF -ignore-filename-regex='/.cargo/|/rustc/|crates/fuzz/' | tail -40
$LLVM_COV show $BIN -instr-profile=$PROF -ignore-filename-regex='/.cargo/|/rustc/|crates/fuzz/' \
    -format=html -output-dir=crates/fuzz/coverage/html
```

`cargo fuzz coverage` rebuilds the target with source-line instrumentation and replays the corpus (the first build takes several minutes).
The HTML report shows line by line which parts of `crates/monty/src` the corpus reaches; unreached areas are candidates for extending `src/catalog.rs` or the generator.
For a per-directory table, pipe the report through the summary script:

```bash
$LLVM_COV report $BIN -instr-profile=$PROF -ignore-filename-regex='/.cargo/|/rustc/|crates/fuzz/' \
    | python3 -I crates/fuzz/scripts/coverage_summary.py
```

After a 150-second single-worker run, for example, the corpus already covered 65% of the lines in `bytecode/`, 64% in `builtins/`, 44% in `modules/` and 38% in `types/`.

## Triage

```bash
crates/fuzz/scripts/triage.sh            # group crash-* files by panic location
crates/fuzz/scripts/triage.sh --tmin     # minimize each crash first (slow)
```

For each unique panic location it writes `crates/fuzz/triage/<location>/` with the input, the decoded `program.py` and the full log, and prints a summary table.
Crashes inside the Ruff parser (a known upstream issue) and strict-mode harness checks are labelled.
Reproduce one outside libFuzzer, then shrink `program.py` by hand:

```bash
cargo run -p monty-fuzz --example fuzz_tool -- run crates/fuzz/triage/<location>/program.py
```

`cargo fuzz fmt --fuzz-dir crates/fuzz ast_panic <input>` and `fuzz_tool show <input>` both print the program behind any corpus or crash file.

## Leak hunting with `memory-model-checks`

Monty's `memory-model-checks` feature makes dropping a heap reference without `drop_with` panic, turning reference-count leaks into findings, and runs the garbage collector on every allocation.
It is much slower; run it as a second phase, seeded with the normal corpus and in its own target directory:

```bash
cp -r crates/fuzz/corpus/ast_panic crates/fuzz/corpus/ast_panic_mmc
mkdir -p crates/fuzz/artifacts/ast_panic_mmc
cargo +nightly fuzz run --fuzz-dir crates/fuzz ast_panic --sanitizer none -a \
    --features memory-model-checks --target-dir target/fuzz-mmc crates/fuzz/corpus/ast_panic_mmc -- \
    -timeout=5 -rss_limit_mb=2048 -fork=7 -ignore_crashes=1 -artifact_prefix=crates/fuzz/artifacts/ast_panic_mmc/
```

All leak panics share one location (the `Drop` impl in `value.rs`); `triage.sh crates/fuzz/artifacts/ast_panic_mmc` groups them by the first backtrace frame outside it instead.
Monty makes no reference-count guarantees after a resource-limit error, so check whether a leak still reproduces with larger limits (`MONTY_FUZZ_FEED_MS`, `MONTY_FUZZ_MEMORY_MB`) before reporting it.

## Not generated

Things the generator deliberately leaves out: host calls (`open`, filesystem, environment), `del`/`yield`/class inheritance and the other compile-time rejections, exception constructors with anything but zero or one string argument (an uncatchable internal error in Monty, see `limitations/exceptions.md`), keyword-only and positional-only parameters, annotations outside dataclass fields, and async comprehensions.
