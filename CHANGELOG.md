# Changelog

All notable changes to cc-history-api are recorded here. Format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/); versioning follows
the semver bump policy documented in [CONTRIBUTING.md](CONTRIBUTING.md) (see
issue #13). The 3 workspace crates (`claude-history-core`,
`claude-history-store`, `claude-history`) and the MCPB bundle manifest
(`mcpb/manifest.json`) are versioned in lockstep — one entry per release
covers all four (see issue #14 for the coupling rule).

Pre-`0.1.0-prep` history is not backfilled here; the git log carries the
forensic record of work prior to this file. The `[Unreleased]` section
accumulates user-visible changes from the close of issue #12 onward.

## [0.1.0] - 2026-05-18

### Added
- Release infrastructure: `CHANGELOG.md` adopting Keep-a-Changelog format
  (closes #12).
- `CONTRIBUTING.md` with semver bump policy: lockstep workspace
  versioning across 3 crates + MCPB manifest; bump-trigger table
  (major / minor / patch); pre-1.0 semver convention (breaking changes
  land in minor with explicit `### Changed` / `### Removed` callout)
  (closes #13).
- MCPB↔crate version-coupling rule documented in CONTRIBUTING.md:
  manifest version tracks workspace crate version 1:1; re-packages
  without code change do not bump; only crate-version bumps trigger
  manifest bumps + rebundle. Forward pointer to issue #17 covers the
  generated-`tools`-array convention (closes #14).
- Tag and GitHub Release convention documented in CONTRIBUTING.md:
  tag format `v<semver>`; thin tag annotation message; every tag
  promoted to a GH Release; Release body extracted verbatim from
  `CHANGELOG.md` `## [<version>] - <date>` section; MCPB bundle
  attached as Release asset; pre-release tag suffix convention
  reserved but not yet implemented (closes #15).
- `scripts/sync_manifest_tools.py` derives `mcpb/manifest.json`'s
  `tools` array from `#[tool(description = "...")]` attributes in
  `crates/server/src/mcp/tools.rs`. Eliminates the parallel-registry
  drift that left the prior bundle declaring 10 tools while the daemon
  served 17. `--check` mode reports drift without writing; default
  mode writes in place. Invoked by the release-orchestration script
  (issue #16) during the bundling step (closes #17).
- `scripts/check_versions.py` verifies lockstep versioning across the
  3 workspace crates and `mcpb/manifest.json`. Exits non-zero on drift
  with per-file diagnostics. Suitable as a CI gate or pre-commit hook;
  invoked by `release.py` at pre-flight and post-bump (closes #16 in
  part).
- `scripts/release.py <patch|minor|major> [--dry-run]` orchestrates
  the full release cut: pre-flight (working-tree clean, required tools
  present, version lockstep, CHANGELOG [Unreleased] non-empty); bump
  all 4 version-bearing files in lockstep; `cargo build --release`;
  promote CHANGELOG [Unreleased] → [version] - date; extract the
  promoted section as GH Release notes; sync MCPB manifest tools array
  via `sync_manifest_tools.py`; stage fresh binary into bundle; pack
  `.mcpb` archive; commit + tag locally. Stops short of remote-
  affecting actions; prints `git push` and `gh release create`
  commands as next-step instructions for user authorization
  (closes #16).

### Changed
- `mcpb/manifest.json`'s `tools` array regenerated from the live
  daemon registry: 10 → 17 tools. Newly advertised in the bundle:
  `list_bookmarks`, `search_bookmarks`, `get_bookmark`,
  `list_attachments`, `get_hook_executions`, `list_plans`, `get_plan`.
  Per the new convention, this array is no longer hand-maintained.

### Changed
- `mcpb/manifest.json` version rolled back from `0.1.1` to `0.1.0` to
  align with the workspace crate baseline at the `0.1.0` release cut.
  The prior `0.1.1` was set during a one-off Feb 2026 rebundle while
  crate versions remained at `0.1.0`; the new coupling rule treats
  that drift as a defect and the rollback closes it (closes #14).
