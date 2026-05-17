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

## [Unreleased]

### Added
- Release infrastructure: `CHANGELOG.md` adopting Keep-a-Changelog format
  (closes #12).
- `CONTRIBUTING.md` with semver bump policy: lockstep workspace
  versioning across 3 crates + MCPB manifest; bump-trigger table
  (major / minor / patch); pre-1.0 semver convention (breaking changes
  land in minor with explicit `### Changed` / `### Removed` callout)
  (closes #13).
