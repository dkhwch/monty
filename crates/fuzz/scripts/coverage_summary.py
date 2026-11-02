"""Summarize an `llvm-cov report` by source directory.

Usage: llvm-cov report ... | python3 -I coverage_summary.py

Groups line coverage of `crates/monty/src` by its top-level directory
(`bytecode`, `builtins`, `types`, `modules`, ...) and the other crates by
crate, which is easier to compare than the per-file report.
"""

import re
import sys
from collections import defaultdict


def main() -> None:
    totals: dict[str, list[int]] = defaultdict(lambda: [0, 0])
    for row in sys.stdin:
        parts = row.split()
        # Columns: Filename Regions Missed Cover Functions Missed Executed Lines Missed Cover ...
        if len(parts) < 10 or not parts[0].endswith('.rs'):
            continue
        match = re.match(r'([^/]+)/src/(.*)', parts[0])
        if not match:
            continue
        crate, rest = match.groups()
        if crate == 'monty':
            key = 'monty/' + (rest.split('/')[0] if '/' in rest else '(top-level files)')
        else:
            key = crate
        lines, missed = int(parts[7]), int(parts[8])
        totals[key][0] += lines
        totals[key][1] += lines - missed
    print(f'{"area":30s} {"covered":>8s} {"lines":>8s} {"cover":>7s}')
    for key, (lines, covered) in sorted(totals.items(), key=lambda kv: -kv[1][0]):
        print(f'{key:30s} {covered:8d} {lines:8d} {100 * covered / max(lines, 1):6.1f}%')


if __name__ == '__main__':
    main()
