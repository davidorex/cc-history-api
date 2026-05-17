#!/usr/bin/env python3
"""Verify lockstep versioning across the 4 release-bearing files.

Per issue #14's coupling rule (documented in CONTRIBUTING.md §Lockstep
workspace versioning): the 3 workspace crates and the MCPB bundle
manifest must all carry the same version string. Drift is a defect.

This script reads:
  - crates/core/Cargo.toml
  - crates/store/Cargo.toml
  - crates/server/Cargo.toml
  - mcpb/manifest.json

and exits 0 if all four versions match, 1 with a diagnostic if they
diverge. Invoked by the release-orchestration script (issue #16) at
pre-flight and again at post-bump verification. Suitable as a CI gate
or pre-commit hook.

Usage:
    python3 scripts/check_versions.py
    python3 scripts/check_versions.py --json    # machine-readable
"""
import json
import re
import subprocess
import sys
from pathlib import Path


def repo_root() -> Path:
    return Path(
        subprocess.check_output(['git', 'rev-parse', '--show-toplevel'])
        .decode()
        .strip()
    )


def read_cargo_version(path: Path) -> str:
    """Extract the [package] version line from a Cargo.toml."""
    for line in path.read_text().splitlines():
        m = re.match(r'^version\s*=\s*"([^"]+)"', line)
        if m:
            return m.group(1)
    raise ValueError(f'No [package] version found in {path}')


def read_manifest_version(path: Path) -> str:
    return json.loads(path.read_text())['version']


def main() -> int:
    json_mode = '--json' in sys.argv[1:]

    root = repo_root()
    files = {
        'crates/core/Cargo.toml': read_cargo_version,
        'crates/store/Cargo.toml': read_cargo_version,
        'crates/server/Cargo.toml': read_cargo_version,
        'mcpb/manifest.json': read_manifest_version,
    }

    versions: dict[str, str] = {}
    errors: list[str] = []
    for rel, reader in files.items():
        path = root / rel
        if not path.exists():
            errors.append(f'{rel}: missing')
            continue
        try:
            versions[rel] = reader(path)
        except Exception as e:
            errors.append(f'{rel}: {e}')

    if json_mode:
        print(json.dumps({'versions': versions, 'errors': errors}, indent=2))
        return 1 if errors or len(set(versions.values())) != 1 else 0

    if errors:
        for e in errors:
            print(f'ERROR: {e}', file=sys.stderr)
        return 1

    unique = set(versions.values())
    if len(unique) == 1:
        v = unique.pop()
        print(f'OK: all 4 files at version {v}')
        return 0

    print('DRIFT: version mismatch across release-bearing files', file=sys.stderr)
    for rel, v in versions.items():
        print(f'  {rel}: {v}', file=sys.stderr)
    print(
        '\nPer CONTRIBUTING.md §Lockstep workspace versioning, all four\n'
        'must agree. Resolve via scripts/release.py <bump-type> or by\n'
        'hand-correcting whichever file drifted from the intended baseline.',
        file=sys.stderr,
    )
    return 1


if __name__ == '__main__':
    sys.exit(main())
