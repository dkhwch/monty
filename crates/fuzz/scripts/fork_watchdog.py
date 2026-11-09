"""Kill hung libFuzzer fork-mode workers so the parent can start new jobs in their place.

Usage (from the repository root, next to a running `cargo fuzz run ... -fork=N`):
    python3 -I crates/fuzz/scripts/fork_watchdog.py --target ast_panic                    # watch, check every 15 s
    python3 -I crates/fuzz/scripts/fork_watchdog.py --target ast_panic --dry-run --once   # report only

Why it is needed: when a worker hits `-timeout` (or crashes, or runs out of memory), libFuzzer saves
the input from a signal handler that allocates memory. If the signal lands while the target holds
the allocator lock, the worker deadlocks: it sleeps forever with an empty `timeout-<sha1>` file
open, and the input survives only in the job log. The fork-mode parent never kills a job, so
every such hang costs one of the N workers for the rest of the run.

A worker counts as hung when it has run longer than its own `-max_total_time` (the parent gives
each job at most 300 s) plus --grace, or when its CPU time has not moved for --stall seconds (a
healthy worker never sleeps). For each hung worker the watchdog:

1. copies the tail of the job log to `<artifact dir>/hung-logs/`, because the parent deletes its
   temp dir when it exits;
2. recovers the input libFuzzer printed just before freezing (it prints inputs up to 256 bytes).
   When its SHA-1 matches the name of the empty artifact the worker had open, the input is written
   into that file, which then works like any other artifact; otherwise it is saved as
   `recovered-<kind>-<sha1>`;
3. sends SIGKILL to the worker only, never to its `sh` wrapper or the parent. With
   `-ignore_crashes=1` the parent counts it as a crash and starts a new job.
"""

import argparse
import hashlib
import os
import re
import signal
import sys
import time
from dataclasses import dataclass
from pathlib import Path

CLK_TCK = os.sysconf('SC_CLK_TCK')
ARTIFACT_RE = re.compile(r'(crash|timeout|oom|leak|slow-unit)-([0-9a-f]{40})')
HEX_LINE_RE = re.compile(r'(0x[0-9a-f]{1,2},)+')
LOG_TAIL_LINES = 1000
INDENT = ' ' * 9

# A process identity that survives PID reuse: (pid, start time in clock ticks).
Key = tuple[int, int]


@dataclass
class Worker:
    """One fork-mode worker, as read from /proc at one check."""

    pid: int
    start_ticks: int
    state: str
    cpu_ticks: int
    argv: list[str]

    @property
    def key(self) -> Key:
        return self.pid, self.start_ticks

    @property
    def target(self) -> str:
        return os.path.basename(self.argv[0])

    def flag(self, name: str) -> str | None:
        """Value of the libFuzzer flag `-name=value` on this worker's command line."""
        prefix = f'-{name}='
        return next((a[len(prefix) :] for a in self.argv if a.startswith(prefix)), None)


def main() -> int:
    parser = argparse.ArgumentParser(
        description=__doc__.split('\n\n')[0], formatter_class=argparse.RawDescriptionHelpFormatter
    )
    parser.add_argument('--target', help='only watch workers of this fuzz target, e.g. ast_panic (default: all)')
    parser.add_argument('--interval', type=float, default=15, help='seconds between checks (default: 15)')
    parser.add_argument(
        '--stall', type=float, default=60, help='kill a worker whose CPU time has not moved for this long (default: 60)'
    )
    parser.add_argument(
        '--grace', type=float, default=60, help='kill a worker this many seconds past its -max_total_time (default: 60)'
    )
    parser.add_argument('--dry-run', action='store_true', help='report hung workers, change nothing')
    parser.add_argument('--once', action='store_true', help='check once and exit (only the --grace rule can fire)')
    args = parser.parse_args()

    mode = ' (dry run)' if args.dry_run else ''
    say(f'watching {args.target or "all"} fork-mode workers every {args.interval:g}s{mode}')
    cpu_seen: dict[Key, tuple[int, float]] = {}
    handled: set[Key] = set()
    try:
        while True:
            check(args, cpu_seen, handled)
            if args.once:
                return 0
            time.sleep(args.interval)
    except KeyboardInterrupt:
        return 0


def check(args: argparse.Namespace, cpu_seen: dict[Key, tuple[int, float]], handled: set[Key]) -> None:
    """One pass: find hung workers and handle each once.

    `cpu_seen` maps each worker to its CPU time and when that last changed, across passes;
    `handled` holds workers already dealt with, so a dry run or a failed kill reports only once.
    """
    now = time.monotonic()
    uptime = float(Path('/proc/uptime').read_text().split()[0])
    alive = set()
    for w in find_workers(args.target):
        alive.add(w.key)
        last = cpu_seen.get(w.key)
        if last is None or last[0] != w.cpu_ticks:
            cpu_seen[w.key] = (w.cpu_ticks, now)
        if w.key in handled or w.state in 'tTZX':  # done already, stopped by a debugger, or dead
            continue
        age = uptime - w.start_ticks / CLK_TCK
        limit = int(w.flag('max_total_time') or 0)
        idle = now - cpu_seen[w.key][1]
        if limit and age > limit + args.grace:
            reason = f'running for {duration(age)}, its -max_total_time is {limit}s'
        elif idle >= args.stall:
            reason = f'no CPU time used for {duration(idle)}'
        else:
            continue
        handled.add(w.key)
        try:
            handle(w, reason, args.dry_run)
        except (FileNotFoundError, ProcessLookupError):
            say(f'{INDENT}exited while being handled')
    for key in cpu_seen.keys() - alive:
        del cpu_seen[key]
    handled.intersection_update(alive)


def handle(w: Worker, reason: str, dry_run: bool) -> None:
    """Save the hung worker's log tail and input, then SIGKILL it."""
    log_path = Path(os.readlink(f'/proc/{w.pid}/fd/2'))
    artifact = open_artifact(w.pid)
    artifact_dir = artifact.parent if artifact else configured_artifact_dir(w)
    job = log_path.stem if log_path.suffix == '.log' else '?'
    would = 'would ' if dry_run else ''
    say(f'hung: pid {w.pid}, {w.target} job {job} ({reason})')

    # The parent removes its temp dir, and the job log with it, when the run ends.
    log_text = ''
    if log_path.suffix == '.log' and log_path.is_file():
        log_text = log_path.read_text(errors='replace')
        saved_log = artifact_dir / 'hung-logs' / f'{w.target}-job{job}-{time.strftime("%Y%m%d-%H%M%S")}.log'
        if not dry_run:
            saved_log.parent.mkdir(exist_ok=True)
            saved_log.write_text('\n'.join(log_text.split('\n')[-LOG_TAIL_LINES:]))
        say(f'{INDENT}{would}save log tail -> {show(saved_log)}')

    data = recover_input(log_text)
    input_dest = None
    if data is None:
        say(f'{INDENT}input not recoverable: not printed in the log (inputs over 256 bytes are not)')
    else:
        input_dest, message = plan_input(data, artifact, artifact_dir)
        say(f'{INDENT}{would if input_dest else ""}{message}')

    if dry_run:
        say(f'{INDENT}not killed (dry run)')
        return
    killed = kill_worker(w)
    say(f'{INDENT}killed; the parent will start a new job' if killed else f'{INDENT}already gone, nothing killed')
    # Written after the kill, so a worker that wakes up after all cannot race us.
    if input_dest is not None:
        input_dest.write_bytes(data)


def find_workers(target: str | None) -> list[Worker]:
    """This user's fork-mode workers, optionally only those running the fuzz target `target`."""
    uid = os.getuid()
    workers = []
    for entry in Path('/proc').iterdir():
        if not entry.name.isdigit():
            continue
        try:
            if entry.stat().st_uid != uid:
                continue
            argv = (entry / 'cmdline').read_bytes().decode(errors='replace').split('\0')[:-1]
            # Fork-mode workers get `-stop_file=<parent temp dir>/STOP` as a separate argument. The
            # `sh -c` wrapper has the whole command line in one argument, so it never matches.
            if not any(a.startswith('-stop_file=') and 'libFuzzerTemp.FuzzWithFork' in a for a in argv):
                continue
            if target and os.path.basename(argv[0]) != target:
                continue
            state, cpu_ticks, start_ticks = read_stat(int(entry.name))
        except (FileNotFoundError, ProcessLookupError, PermissionError):
            continue  # exited while we looked
        workers.append(Worker(int(entry.name), start_ticks, state, cpu_ticks, argv))
    return workers


def read_stat(pid: int) -> tuple[str, int, int]:
    """Return (state, utime + stime, start time) from /proc/PID/stat, times in clock ticks."""
    text = Path(f'/proc/{pid}/stat').read_text()
    # Skip "pid (comm) ", comm may contain spaces. fields[0] is field 3 of proc(5), the state;
    # utime, stime and starttime are fields 14, 15 and 22.
    fields = text[text.rindex(')') + 2 :].split()
    return fields[0], int(fields[11]) + int(fields[12]), int(fields[19])


def open_artifact(pid: int) -> Path | None:
    """The `timeout-<sha1>`-style artifact the worker has open, i.e. was in the middle of writing."""
    for fd in Path(f'/proc/{pid}/fd').iterdir():
        try:
            path = Path(os.readlink(fd))
        except OSError:
            continue
        if ARTIFACT_RE.fullmatch(path.name):
            return path
    return None


def configured_artifact_dir(w: Worker) -> Path:
    """Directory of the worker's `-artifact_prefix`, resolved against its working directory."""
    cwd = os.readlink(f'/proc/{w.pid}/cwd')
    # A prefix is a directory ending in `/` or a file name prefix; either way its dirname is the directory.
    return Path(os.path.dirname(os.path.join(cwd, w.flag('artifact_prefix') or '')) or cwd)


def recover_input(log_text: str) -> bytes | None:
    """The input libFuzzer printed as hex after its last `...; base unit: <sha1>` line, if any."""
    lines = log_text.split('\n')
    for i in range(len(lines) - 1, -1, -1):
        if '; base unit: ' in lines[i]:
            hex_line = lines[i + 1] if i + 1 < len(lines) else ''
            if HEX_LINE_RE.fullmatch(hex_line):
                return bytes(int(h, 16) for h in hex_line.split(',') if h)
            return None  # over 256 bytes: libFuzzer does not print it
    return None


def plan_input(data: bytes, artifact: Path | None, artifact_dir: Path) -> tuple[Path | None, str]:
    """Where to write a recovered input (None: nowhere), and a message describing it.

    The empty artifact itself when the input's SHA-1 matches its name, so it becomes a normal
    artifact; otherwise a separate `recovered-*` file, never overwriting anything.
    """
    sha = hashlib.sha1(data).hexdigest()
    if artifact and artifact.name.endswith(sha):
        if artifact.stat().st_size:
            return None, f'input already saved in {show(artifact)}'
        return artifact, f'write input -> {show(artifact)} ({len(data)} bytes, SHA-1 matches its name)'
    kind = ARTIFACT_RE.fullmatch(artifact.name)[1] if artifact else 'hung'
    dest = artifact_dir / f'recovered-{kind}-{sha}'
    if dest.exists():
        return None, f'input already saved in {show(dest)}'
    return dest, f'save input -> {show(dest)} ({len(data)} bytes)'


def kill_worker(w: Worker) -> bool:
    """SIGKILL the worker, unless its PID now belongs to another process. True if killed."""
    try:
        pidfd = os.pidfd_open(w.pid)
    except ProcessLookupError:
        return False
    try:
        # The pidfd pins whichever process has this PID now: check it is still the one judged hung.
        if read_stat(w.pid)[2] != w.start_ticks:
            return False
        signal.pidfd_send_signal(pidfd, signal.SIGKILL)
        return True
    except (FileNotFoundError, ProcessLookupError):
        return False
    finally:
        os.close(pidfd)


def show(path: Path) -> str:
    """`path` relative to the current directory when it is inside it, for shorter messages."""
    rel = os.path.relpath(path)
    return str(path) if rel.startswith('..') else rel


def duration(seconds: float) -> str:
    """`42s`, `4m05s` or `2h07m`."""
    s = int(seconds)
    if s < 60:
        return f'{s}s'
    if s < 3600:
        return f'{s // 60}m{s % 60:02d}s'
    return f'{s // 3600}h{s % 3600 // 60:02d}m'


def say(message: str) -> None:
    """Print a timestamped line, flushed so it shows up promptly when redirected to a file."""
    print(f'{time.strftime("%H:%M:%S")} {message}', flush=True)


if __name__ == '__main__':
    sys.exit(main())
