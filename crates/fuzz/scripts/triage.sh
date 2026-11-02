#!/usr/bin/env bash
# Triage the crashes found by the ast_panic fuzz target.
#
# Usage (from anywhere in the repo):
#   crates/fuzz/scripts/triage.sh [--tmin] [ARTIFACT_DIR]
#
# For every crash-* file in ARTIFACT_DIR (default crates/fuzz/artifacts/ast_panic):
#   1. optionally minimizes it with `cargo fuzz tmin` (--tmin; slow but gives small reproducers),
#   2. reruns it once with RUST_BACKTRACE=1 and extracts the panic location and message,
#   3. decodes it into the Python program it represents (fuzz_tool show),
# then groups crashes by panic location, so several inputs hitting the same line count once.
#
# Results go to crates/fuzz/triage/<location>/{input,program.py,log.txt}, plus a summary
# table on stdout. Locations inside value.rs (memory-model-checks leak panics all panic in
# the same Drop impl) are replaced by the first backtrace frame elsewhere in crates/monty.
# Crashes from the Ruff parser and harness self-checks (MONTY_FUZZ_STRICT) are flagged.
#
# Needs the fuzz target and the tool built first:
#   cargo +nightly fuzz build --fuzz-dir crates/fuzz ast_panic --sanitizer none -a
#   cargo build -p monty-fuzz --example fuzz_tool

set -euo pipefail

REPO=$(git rev-parse --show-toplevel)
TMIN=0
if [[ "${1:-}" == "--tmin" ]]; then
    TMIN=1
    shift
fi
ARTIFACTS=${1:-$REPO/crates/fuzz/artifacts/ast_panic}
BIN=$REPO/target/x86_64-unknown-linux-gnu/release/ast_panic
TOOL=$REPO/target/debug/examples/fuzz_tool
OUT=$REPO/crates/fuzz/triage

for exe in "$BIN" "$TOOL"; do
    [[ -x "$exe" ]] || { echo "missing $exe (see the build commands at the top of this script)" >&2; exit 1; }
done
shopt -s nullglob
crashes=("$ARTIFACTS"/crash-*)
[[ ${#crashes[@]} -gt 0 ]] || { echo "no crash-* files in $ARTIFACTS"; exit 0; }
mkdir -p "$OUT"

declare -A count message
for crash in "${crashes[@]}"; do
    input=$crash
    if [[ $TMIN -eq 1 ]]; then
        # Every minimization round rewrites the same exact path; keep going if tmin fails.
        minimized="$OUT/minimized-$(basename "$crash")"
        (cd "$REPO" && cargo +nightly fuzz tmin --fuzz-dir crates/fuzz ast_panic --sanitizer none -a \
            --runs 2000 "$crash" -- -exact_artifact_path="$minimized" >/dev/null 2>&1) || true
        [[ -f "$minimized" ]] && input=$minimized
    fi

    log=$(RUST_BACKTRACE=1 timeout 60 "$BIN" "$input" 2>&1 || true)
    # Rust prints: thread '...' panicked at <file>:<line>:<col>:\n<message>
    location=$(grep -m1 -oP "panicked at \K[^ ]+(?=:$)" <<<"$log" || true)
    msg=$(grep -m1 -A1 "panicked at" <<<"$log" | tail -n1 || true)
    if [[ "$location" == *"/value.rs:"* ]]; then
        frame=$(grep -oP "at \K\S*crates/monty/src/\S+" <<<"$log" | grep -v "/value.rs:" | head -n1 || true)
        [[ -n "$frame" ]] && location="leak via ${frame#./}"
    fi
    if [[ -z "$location" ]]; then
        location="no-panic:$(grep -m1 -oE "ERROR: libFuzzer: [a-z-]+|has overflowed its stack" <<<"$log" || echo unknown)"
    fi
    [[ "$location" == *ruff_* ]] && location="RUFF $location"
    [[ "$msg" == MONTY_FUZZ_STRICT* ]] && location="HARNESS(strict) $location"

    key=$(tr -c 'A-Za-z0-9_.-' '_' <<<"$location")
    dir="$OUT/$key"
    if [[ ! -d "$dir" ]]; then
        mkdir -p "$dir"
        cp "$input" "$dir/input"
        "$TOOL" show "$input" | tail -n +2 >"$dir/program.py"
        printf '%s\n' "$log" >"$dir/log.txt"
    fi
    [[ -n "${message[$location]+set}" ]] || message[$location]=$msg
    count[$location]=$(( ${count[$location]:-0} + 1 ))
done

printf '%-6s  %-60s  %s\n' COUNT LOCATION MESSAGE
for location in "${!count[@]}"; do
    printf '%-6s  %-60s  %s\n' "${count[$location]}" "$location" "${message[$location]:-}"
done | sort -rn
echo
echo "Details (input, program.py, log.txt) per location in $OUT/"
echo "Reproduce one outside libFuzzer: cargo run -p monty-fuzz --example fuzz_tool -- run <dir>/program.py"
