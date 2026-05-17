#!/usr/bin/env python3
"""Release-orchestration for cc-history-api.

Mechanizes the full release cut as a single command. Closes issue #16.

Operates on the four release-bearing files in lockstep per issue #14:
  - crates/core/Cargo.toml
  - crates/store/Cargo.toml
  - crates/server/Cargo.toml
  - mcpb/manifest.json

Promotes CHANGELOG.md's [Unreleased] section to a versioned block per
issue #12 + #15 conventions. Regenerates the MCPB manifest's tools array
per issue #17. Builds the release binary, rebundles the MCPB archive,
commits, tags, and prepares the GH Release artifacts. Stops short of
`git push` and `gh release create` — those are user-authorized actions
the script prints as next-step instructions.

Usage:
    python3 scripts/release.py <patch|minor|major>
    python3 scripts/release.py <patch|minor|major> --dry-run

Bump-type semantics: see CONTRIBUTING.md §Bump triggers.

The script's only side effects in --dry-run mode are reading files and
printing what would happen. Real-mode side effects: file edits, cargo
build, mcpb pack, git add/commit/tag. No remote-affecting actions
(push / GH Release create) are taken by the script itself.
"""
import json
import re
import shutil
import subprocess
import sys
from datetime import date
from pathlib import Path


# -----------------------------------------------------------------------------
# Repo + version utilities
# -----------------------------------------------------------------------------

def repo_root() -> Path:
    return Path(
        subprocess.check_output(['git', 'rev-parse', '--show-toplevel'])
        .decode()
        .strip()
    )


def bump(version: str, kind: str) -> str:
    """Compute the next version per major / minor / patch."""
    m = re.fullmatch(r'(\d+)\.(\d+)\.(\d+)', version)
    if not m:
        raise ValueError(f'Not a bare semver: {version!r}')
    major, minor, patch = (int(x) for x in m.groups())
    if kind == 'major':
        return f'{major + 1}.0.0'
    if kind == 'minor':
        return f'{major}.{minor + 1}.0'
    if kind == 'patch':
        return f'{major}.{minor}.{patch + 1}'
    raise ValueError(f'Unknown bump kind: {kind}')


def read_cargo_version(path: Path) -> str:
    for line in path.read_text().splitlines():
        m = re.match(r'^version\s*=\s*"([^"]+)"', line)
        if m:
            return m.group(1)
    raise ValueError(f'No [package] version in {path}')


def write_cargo_version(path: Path, new_version: str) -> None:
    out: list[str] = []
    replaced = False
    for line in path.read_text().splitlines():
        if not replaced and re.match(r'^version\s*=\s*"', line):
            out.append(f'version = "{new_version}"')
            replaced = True
        else:
            out.append(line)
    if not replaced:
        raise ValueError(f'No [package] version in {path}')
    path.write_text('\n'.join(out) + '\n')


def read_manifest_version(path: Path) -> str:
    return json.loads(path.read_text())['version']


def write_manifest_version(path: Path, new_version: str) -> None:
    data = json.loads(path.read_text())
    data['version'] = new_version
    path.write_text(json.dumps(data, indent=2, ensure_ascii=False) + '\n')


# -----------------------------------------------------------------------------
# CHANGELOG handling
# -----------------------------------------------------------------------------

def promote_changelog(changelog: Path, version: str, today: str) -> str:
    """Replace `## [Unreleased]` with `## [<version>] - <today>`.

    Returns the body of the promoted section (everything between the new
    `## [<version>] - <today>` heading and the next `## ` heading or EOF).
    The body is suitable as `gh release create --notes-file` content.
    """
    text = changelog.read_text()
    if '## [Unreleased]' not in text:
        raise ValueError(
            'CHANGELOG.md has no `## [Unreleased]` section — nothing to promote'
        )
    new_heading = f'## [{version}] - {today}'
    text = text.replace('## [Unreleased]', new_heading, 1)
    changelog.write_text(text)

    # Extract the promoted section body for GH Release notes
    lines = text.splitlines()
    body: list[str] = []
    in_section = False
    for line in lines:
        if line.startswith(new_heading):
            in_section = True
            continue
        if in_section and line.startswith('## '):
            break
        if in_section:
            body.append(line)
    # Trim trailing blank lines
    while body and not body[-1].strip():
        body.pop()
    return '\n'.join(body).strip() + '\n'


def unreleased_has_content(changelog: Path) -> bool:
    """Check that [Unreleased] has at least one non-blank line of content."""
    in_section = False
    for line in changelog.read_text().splitlines():
        if line.startswith('## [Unreleased]'):
            in_section = True
            continue
        if in_section and line.startswith('## '):
            return False
        if in_section and line.strip():
            return True
    return False


# -----------------------------------------------------------------------------
# Shell helpers
# -----------------------------------------------------------------------------

def run(cmd: list[str], cwd: Path | None = None, capture: bool = False) -> str:
    print(f'  $ {" ".join(cmd)}')
    if capture:
        r = subprocess.run(cmd, cwd=cwd, check=True, capture_output=True, text=True)
        return r.stdout
    subprocess.run(cmd, cwd=cwd, check=True)
    return ''


def have_tool(name: str) -> bool:
    return shutil.which(name) is not None


# -----------------------------------------------------------------------------
# Main flow
# -----------------------------------------------------------------------------

def main() -> int:
    args = sys.argv[1:]
    dry_run = '--dry-run' in args
    args = [a for a in args if a != '--dry-run']
    if len(args) != 1 or args[0] not in ('patch', 'minor', 'major'):
        print('Usage: release.py <patch|minor|major> [--dry-run]', file=sys.stderr)
        return 2
    bump_kind = args[0]

    root = repo_root()
    changelog = root / 'CHANGELOG.md'
    manifest = root / 'mcpb' / 'manifest.json'
    cargo_files = [
        root / 'crates' / 'core' / 'Cargo.toml',
        root / 'crates' / 'store' / 'Cargo.toml',
        root / 'crates' / 'server' / 'Cargo.toml',
    ]
    bin_release = root / 'target' / 'release' / 'claude-history'
    bin_in_bundle = root / 'mcpb' / 'bin' / 'claude-history'
    sync_tools = root / 'scripts' / 'sync_manifest_tools.py'
    check_versions = root / 'scripts' / 'check_versions.py'

    mode = 'DRY-RUN' if dry_run else 'LIVE'
    print(f'\n=== cc-history-api release ({mode}, bump={bump_kind}) ===\n')

    # -- 1. Pre-flight --
    print('[1/9] Pre-flight checks')
    for tool in ('cargo', 'mcpb', 'gh', 'git', 'python3'):
        if not have_tool(tool):
            print(f'  ERROR: required tool not found: {tool}', file=sys.stderr)
            return 1
    print('  All required tools present: cargo, mcpb, gh, git, python3')

    status = run(['git', 'status', '--porcelain'], capture=True)
    if status.strip():
        print('  ERROR: working tree not clean:', file=sys.stderr)
        print(status, file=sys.stderr)
        return 1
    print('  Working tree clean')

    branch = run(['git', 'rev-parse', '--abbrev-ref', 'HEAD'], capture=True).strip()
    if branch != 'main':
        print(
            f'  WARNING: not on main branch (on {branch!r}). Continuing — '
            f'review tag placement before pushing.'
        )

    subprocess.run([sys.executable, str(check_versions)], check=True)

    if not unreleased_has_content(changelog):
        print(
            '  ERROR: CHANGELOG.md [Unreleased] section is empty — nothing '
            'to release. Add entries under appropriate subsections before '
            'cutting.',
            file=sys.stderr,
        )
        return 1
    print('  CHANGELOG.md [Unreleased] has content')

    # -- 2. Compute new version --
    print('\n[2/9] Compute new version')
    current = read_cargo_version(cargo_files[0])
    new = bump(current, bump_kind)
    print(f'  {current} → {new}')

    if dry_run:
        print('\n[DRY-RUN] Would bump 4 files to', new)
        print('[DRY-RUN] Would promote CHANGELOG [Unreleased] → [%s]' % new)
        print('[DRY-RUN] Would build, rebundle, commit, tag')
        print('[DRY-RUN] No changes written')
        return 0

    # -- 3. Bump versions in lockstep --
    print(f'\n[3/9] Bump versions to {new}')
    for f in cargo_files:
        write_cargo_version(f, new)
        print(f'  {f.relative_to(root)}: {new}')
    write_manifest_version(manifest, new)
    print(f'  {manifest.relative_to(root)}: {new}')
    subprocess.run([sys.executable, str(check_versions)], check=True)

    # -- 4. Build release binary (also updates Cargo.lock) --
    print('\n[4/9] cargo build --release')
    run(['cargo', 'build', '--release'])

    # -- 5. Promote CHANGELOG section + extract notes --
    print(f'\n[5/9] Promote CHANGELOG [Unreleased] → [{new}]')
    today = date.today().isoformat()
    notes_body = promote_changelog(changelog, new, today)
    notes_file = root / 'target' / f'release-notes-v{new}.md'
    notes_file.parent.mkdir(parents=True, exist_ok=True)
    notes_file.write_text(notes_body)
    print(f'  Extracted release notes to {notes_file.relative_to(root)}')
    print(f'  ({len(notes_body.splitlines())} lines, {len(notes_body)} chars)')

    # -- 6. Sync MCPB manifest tools array (per #17) --
    print('\n[6/9] Sync MCPB manifest tools array from daemon registry')
    run([sys.executable, str(sync_tools)])

    # -- 7. Copy fresh binary + pack MCPB bundle --
    print('\n[7/9] Stage binary + pack MCPB bundle')
    bin_in_bundle.parent.mkdir(parents=True, exist_ok=True)
    shutil.copy2(bin_release, bin_in_bundle)
    print(f'  Copied {bin_release.relative_to(root)} → {bin_in_bundle.relative_to(root)}')
    run(['mcpb', 'pack'], cwd=manifest.parent)
    bundle = manifest.parent / 'mcpb.mcpb'
    if not bundle.exists():
        bundles = list(manifest.parent.glob('*.mcpb'))
        if not bundles:
            print('  ERROR: mcpb pack produced no .mcpb file', file=sys.stderr)
            return 1
        bundle = bundles[0]
    size_mb = bundle.stat().st_size / 1_000_000
    print(f'  Packed bundle: {bundle.relative_to(root)} ({size_mb:.2f} MB)')

    # -- 8. Commit + tag --
    print(f'\n[8/9] Commit + tag v{new}')
    # Non-gitignored files
    run(['git', 'add'] + [str(f) for f in cargo_files] +
        [str(root / 'Cargo.lock'), str(manifest), str(changelog)])
    # Gitignored release assets (mcpb/bin/ and mcpb/*.mcpb are .gitignored
    # for dev-iteration sanity, but the cut bundle is a tagged artifact
    # whose hash is part of the release identity per #16 design rationale).
    # `git add -f` overrides the gitignore for the release-asset paths.
    run(['git', 'add', '-f', str(bin_in_bundle), str(bundle)])
    run(['git', 'commit', '-m', f'Release v{new}'])
    run(['git', 'tag', f'v{new}'])

    # -- 9. Next steps --
    print(f'\n[9/9] Local cut complete: v{new}')
    print('\n=== Next steps (user-authorized) ===')
    print(f'  1. Push commit + tag:')
    print(f'     git push origin main && git push origin v{new}')
    print(f'')
    print(f'  2. Create GH Release with the extracted notes + bundle asset:')
    print(f'     gh release create v{new} \\')
    print(f'       --title "v{new}" \\')
    print(f'       --notes-file "{notes_file.relative_to(root)}" \\')
    print(f'       "{bundle.relative_to(root)}"')
    print(f'')
    print(f'The release notes file is preserved at {notes_file.relative_to(root)} ')
    print(f'until the next cut; it is also reproducible from the [{new}] section ')
    print(f'in CHANGELOG.md.')
    return 0


if __name__ == '__main__':
    try:
        sys.exit(main())
    except subprocess.CalledProcessError as e:
        print(f'\nERROR: subprocess failed: {e}', file=sys.stderr)
        sys.exit(1)
