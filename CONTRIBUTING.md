# Contributing to cc-history-api

This document is the contributor-facing complement to README.md (which is
user-facing). It currently covers the release/versioning policy that the
release-orchestration script (issue #16) and the tag/release convention
(issue #15) operate against. Other contributor concerns (PR process,
review expectations, test conventions) are not yet documented here.

## Versioning and release policy

### Lockstep workspace versioning

The 3 workspace crates and the MCPB bundle manifest are versioned in
lockstep. One bump decision applies to all four files as a unit:

- `crates/core/Cargo.toml`
- `crates/store/Cargo.toml`
- `crates/server/Cargo.toml`
- `mcpb/manifest.json`

A standalone version drift across these four files is treated as a
defect; the release-orchestration script (issue #16) enforces the
coupling, and a `scripts/check-versions.sh` verifier (also under issue
#16) fails CI / pre-flight if any drift is observed. The coupling rule
itself is documented in issue #14's resolution.

### Semver interpretation

cc-history-api follows [semver 2.0.0](https://semver.org/) with the
trigger table below. The version range is currently `0.x`; the
pre-1.0-release semver convention adopted here treats `0.x` bumps with
the same trigger semantics as `1.x` would — that is, breaking changes
land in **minor** bumps pre-1.0 (per strict semver §4) but are still
called out under `### Changed` / `### Removed` in `CHANGELOG.md` so
downstream consumers can detect them without re-reading the trigger
table. A `1.0.0` cut would shift breaking changes to require **major**
bumps; that transition is out of scope for this document.

### Bump triggers

| Bump type | Triggers |
|---|---|
| **major** | Incompatible REST API change (removed endpoint, breaking response-shape change, breaking query-parameter rename); incompatible MCP-tool surface change (removed tool, breaking parameter rename, breaking result-shape change); incompatible CLI surface change (removed subcommand, breaking flag rename, breaking exit-code semantics); SQLite migration that requires manual recovery or destroys data; semantics change to `~/.claude/.claude-history.db` storage that breaks backward read compatibility. |
| **minor** | New REST endpoint; new MCP tool; new CLI subcommand; new CLI flag (additive, optional); new SQLite migration that is purely additive (new table, new column with `IF NOT EXISTS`, new index, new view); new canned query shipped with the binary; new typed-record variant; new analytical view; new search index; new attachment-subtype handling. Within the `0.x` range, breaking changes also land here per the convention above, but must be documented in CHANGELOG `### Changed` / `### Removed`. |
| **patch** | Bug fix that does not change a surface contract; internal refactor with no behavior change; documentation; dependency bump with no surface change; performance improvement; test-only change; CI / build-script change. |

### Bump decision flow

When preparing a release cut, the human running `scripts/release.sh`
(issue #16) reads the `[Unreleased]` section of `CHANGELOG.md`,
identifies the highest-tier entry under that section per the trigger
table above, and invokes the script with that bump type:

```
scripts/release.sh patch   # bug-fix-only release
scripts/release.sh minor   # any additive surface entry
scripts/release.sh major   # any breaking entry (post-1.0)
```

The script's pre-flight does not auto-derive the bump type from the
CHANGELOG content (parsing intent from English is unreliable); the
choice is human-authorized, the script mechanizes everything downstream
of the choice.

### Tag and GitHub Release convention

Documented in issue #15's resolution. In summary: tag format
`v<semver>` (e.g., `v0.1.0`); every tag is promoted to a GitHub Release
whose body is the extracted `## [<version>] - <date>` section from
`CHANGELOG.md`; tag annotation message stays thin (`Release v<version>`)
since the notes live in the GH Release body.

### History

The `v1.0` tag (Feb 21 2026) predates the OSS-release framing and the
policy above; it is retained as a historical pre-OSS marker. The
post-`0.1.0-prep` cut produces `v0.1.0` regardless of the prior `v1.0`,
per the decision recorded at the establishment of the `0.1.0-prep`
milestone.
