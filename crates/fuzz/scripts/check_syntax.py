"""Check that generated programs are valid Python, independently of Monty.

Usage: python3 -I check_syntax.py DIR

Compiles every `*.py` file in DIR with CPython's `compile()`, which runs the
parser and the symbol-table checks (`return` outside a function, misplaced
`nonlocal`, duplicate arguments, ...). Warnings are ignored; only
`SyntaxError`s count. Exits non-zero if any file fails.
"""

import sys
import warnings
from pathlib import Path


def main() -> int:
    directory = Path(sys.argv[1])
    files = sorted(directory.glob('*.py'))
    failures = []
    with warnings.catch_warnings():
        warnings.simplefilter('ignore')
        for path in files:
            try:
                compile(path.read_text(), str(path), 'exec')
            except SyntaxError as exc:
                failures.append(f'{path.name}:{exc.lineno}: {exc.msg}')
    print(f'{len(files)} files, {len(failures)} with SyntaxError')
    for failure in failures[:20]:
        print('  ' + failure)
    return 1 if failures else 0


if __name__ == '__main__':
    sys.exit(main())
