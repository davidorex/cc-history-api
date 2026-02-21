# Milestones: claude-history

## v1.0 - MVP

**Shipped:** 2026-02-21
**Stats:** 6 phases, 27 plans, 16,688 LOC (15,969 Rust + 719 SQL), 148 tests

- Exact serde modeling of Claude Code's JSONL records with overflow-driven schema drift detection
- Normalized SQLite store with incremental byte-offset sync and FTS5 full-text search
- axum HTTP API with 27 endpoints at /v1/, Unix domain socket, CLI-daemon integration
- Real-time file watcher with debounced ingestion and SSE event stream (7 event types)
- Artifact layer: file operation tracking, git extraction, content reconstruction via edit replay
- Version monitoring with persistent version_history and grouped drift analysis with promotion status

[Full archive](milestones/v1.0-ROADMAP.md)

---
