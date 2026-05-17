#!/usr/bin/env python3
"""Regenerate mcpb/manifest.json's `tools` array from the MCP tool registry.

Source of truth: each `#[tool(description = "...")]` attribute in
crates/server/src/mcp/tools.rs, paired with the immediately-following
`async fn NAME(` declaration.

Per issue #17: the manifest's `tools` array is a generated artifact, not
hand-maintained. This script eliminates the parallel-registry drift that
left the bundled manifest declaring 10 tools while the daemon served 17.

Invoked by the release-orchestration script (#16) during the bundling step.
Standalone invocation regenerates the array in-place; the result is visible
via `git diff mcpb/manifest.json`.

Usage:
    python3 scripts/sync_manifest_tools.py           # write
    python3 scripts/sync_manifest_tools.py --check   # exit 1 if sync needed
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


def extract_tools(source: str) -> list[dict]:
    """Pair each #[tool(description = "...")] line with the next async fn name.

    Rust string literals may embed escaped quotes (\\"); the regex consumes
    those without terminating the outer match. The captured descriptions are
    then Rust-unescaped before being placed into JSON (the JSON serializer
    re-escapes per its own rules).
    """
    tools: list[dict] = []
    pending_desc: str | None = None

    desc_pattern = re.compile(r'^\s+#\[tool\(description = "((?:[^"\\]|\\.)*)"\)\]$')
    fn_pattern = re.compile(r'^\s+async fn ([a-z_][a-z_0-9]*)\(')

    for line in source.splitlines():
        m = desc_pattern.match(line)
        if m:
            pending_desc = (
                m.group(1)
                .replace('\\\\', '\x00')   # placeholder so the next two don't see it
                .replace('\\"', '"')
                .replace('\\n', '\n')
                .replace('\x00', '\\')
            )
            continue
        if pending_desc is not None:
            f = fn_pattern.match(line)
            if f:
                tools.append({'name': f.group(1), 'description': pending_desc})
                pending_desc = None

    return tools


def main() -> int:
    check_mode = '--check' in sys.argv[1:]

    root = repo_root()
    tools_rs = root / 'crates' / 'server' / 'src' / 'mcp' / 'tools.rs'
    manifest_path = root / 'mcpb' / 'manifest.json'

    if not tools_rs.exists():
        print(f'ERROR: {tools_rs} not found', file=sys.stderr)
        return 1
    if not manifest_path.exists():
        print(f'ERROR: {manifest_path} not found', file=sys.stderr)
        return 1

    tools = extract_tools(tools_rs.read_text())
    if not tools:
        print(
            'ERROR: extracted zero tools — parser-vs-source-format drift '
            'likely. Inspect crates/server/src/mcp/tools.rs for changes to '
            'the #[tool(description = "...")] attribute shape.',
            file=sys.stderr,
        )
        return 1

    rel_tools = tools_rs.relative_to(root)
    rel_manifest = manifest_path.relative_to(root)
    print(f'Extracted {len(tools)} tools from {rel_tools}')

    manifest = json.loads(manifest_path.read_text())
    existing_tools = manifest.get('tools', [])

    if existing_tools == tools:
        print(f'{rel_manifest} already in sync; no changes')
        return 0

    if check_mode:
        old_names = {t.get('name') for t in existing_tools}
        new_names = {t['name'] for t in tools}
        added = sorted(new_names - old_names)
        removed = sorted(old_names - new_names)
        print(
            f'OUT OF SYNC: {rel_manifest} declares {len(existing_tools)} tools, '
            f'registry has {len(tools)}',
            file=sys.stderr,
        )
        if added:
            print(f'  Missing from manifest: {", ".join(added)}', file=sys.stderr)
        if removed:
            print(f'  Stale in manifest:    {", ".join(removed)}', file=sys.stderr)
        return 1

    manifest['tools'] = tools
    manifest_path.write_text(
        json.dumps(manifest, indent=2, ensure_ascii=False) + '\n'
    )
    print(f'Wrote {len(tools)} tools to {rel_manifest}')
    return 0


if __name__ == '__main__':
    sys.exit(main())
