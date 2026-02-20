# Stack Research

**Domain:** Rust CLI daemon — JSONL ingestion, SQLite store, HTTP/UDS/CLI API
**Project:** claude-history
**Researched:** 2026-02-20
**Confidence:** HIGH

## Recommended Stack

### Core Technologies

| Technology | Version | Purpose | Why Recommended | Confidence |
|------------|---------|---------|-----------------|------------|
| Rust (edition 2024) | 1.85+ | Language | Memory safety without GC, single static binary, zero runtime deps. Edition 2024 brings resolver v3 by default. | HIGH |
| tokio | 1.49.0 | Async runtime | De facto Rust async runtime. LTS 1.47.x until Sep 2026. Required by axum, notify, and every async crate in the ecosystem. Use `features = ["full"]` for file I/O + signals + net. | HIGH |
| serde | 1.0.228 | Serialization framework | The only serious serialization framework in Rust. Internally tagged enums (`#[serde(tag = "type")]`) map directly to Claude's JSONL discriminated union format. `flatten` with `HashMap<String, Value>` captures overflow/unknown fields. Since serde 1.0.46, flattened internally tagged enums deserialize correctly. | HIGH |
| serde_json | 1.0.149 | JSON (de)serialization | Required companion to serde for JSON. Supports `raw_value` feature for zero-copy preservation of unknown payloads. `Value` type serves as the overflow capture sink. | HIGH |
| rusqlite | 0.38.0 | SQLite bindings | Ergonomic synchronous SQLite access. The `bundled` feature compiles SQLite 3.51.1 from source, which unconditionally enables FTS5, FTS3, JSON1, RTREE, and STAT4 via compile flags in libsqlite3-sys/build.rs. No separate FTS5 feature flag needed — bundled implies it. | HIGH |
| axum | 0.8.8 | HTTP framework | Tokio team's own web framework. Built on tower + hyper. First-class SSE support (`axum::response::sse`). Official UDS example via `tokio::net::UnixListener`. 0.8.x is stable; 0.9 is in development on main branch. | HIGH |
| clap | 4.5.59 | CLI argument parsing | Dominant CLI parser. Derive macro (`#[derive(Parser)]`) generates help, completions, and subcommands from struct definitions. Stable 4.x line with frequent patch releases. | HIGH |
| notify | 8.2.0 | Filesystem watching | Cross-platform file watcher (FSEvents on macOS, inotify on Linux, ReadDirectoryChanges on Windows). v8 replaced `instant` with `web-time`. Pair with `notify-debouncer-full` 0.7.0 for debounced events with file rename tracking. | HIGH |
| tracing | 0.1.44 | Structured logging | Tokio ecosystem's structured diagnostics. Spans with temporal context, not just log lines. Integrates with axum via tower-http tracing layer. | HIGH |

### Supporting Libraries

| Library | Version | Purpose | When to Use | Confidence |
|---------|---------|---------|-------------|------------|
| tracing-subscriber | 0.3.22 | Log output formatting | Always. Provides `fmt` layer with JSON output, `EnvFilter` for RUST_LOG control, and `Registry` for composing layers. | HIGH |
| tower-http | 0.6.8 | HTTP middleware | Always with axum. Use features: `trace`, `cors`, `compression-gzip`, `request-id`, `timeout`. Provides `TraceLayer` that integrates with tracing. | HIGH |
| tokio-stream | 0.1.18 | Async stream utilities | SSE endpoints. Wraps tokio channels into `Stream` for axum SSE responses. `wrappers` feature for `BroadcastStream`. | HIGH |
| hyper-util | 0.1.20 | Hyper utilities | UDS serving. Required for unix domain socket listener plumbing with axum `serve`. | MEDIUM |
| axum-extra | 0.12.5 | Axum extensions | Optional. Typed headers, query parameter extraction, `TypedPath` for compile-time-checked routes. | MEDIUM |
| chrono | 0.4.43 | Date/time handling | Timestamp parsing/formatting. Claude JSONL uses ISO 8601 timestamps. Rusqlite has a `chrono` feature for direct SQLite datetime interop. | HIGH |
| uuid | 1.21.0 | UUID generation/parsing | Session and conversation IDs. `v4` feature for random UUIDs, `serde` feature for serialization. | HIGH |
| thiserror | 2.0.18 | Error type derivation | Library crates (core, store). Derive `Display` + `Error` impls on domain error enums. v2 is the current major. | HIGH |
| anyhow | 1.0.101 | Opaque error handling | Binary crate (server/CLI). Ergonomic `Result<T>` for main entrypoints where error type enumeration is not needed. | HIGH |
| notify-debouncer-full | 0.7.0 | Event debouncing | File watcher integration. Aggregates rapid filesystem events, tracks file renames, provides cache of filesystem state. | HIGH |
| tempfile | 3.25.0 | Temporary files | Testing. Temp databases and temp JSONL fixtures. | HIGH |

### Development Tools

| Tool | Purpose | Notes |
|------|---------|-------|
| cargo-watch | Rebuild on change | `cargo install cargo-watch` then `cargo watch -x check -x test` |
| cargo-nextest | Fast test runner | Parallel test execution, better output. `cargo install cargo-nextest` |
| cargo-deny | Dependency auditing | License and vulnerability checking. `cargo install cargo-deny` |
| sqlx-cli (optional) | SQL migration tooling | Only if you later migrate from rusqlite to sqlx. Not needed initially. |

## Cargo Workspace Structure

```
claude-history/
  Cargo.toml          # workspace root
  crates/
    core/             # JSONL models, serde types, domain logic
      Cargo.toml
      src/lib.rs
    store/            # SQLite schema, queries, FTS5, migrations
      Cargo.toml
      src/lib.rs
    server/           # axum HTTP + UDS + SSE + CLI binary
      Cargo.toml
      src/main.rs
```

**Workspace Cargo.toml pattern:**
```toml
[workspace]
resolver = "3"
members = ["crates/*"]

[workspace.dependencies]
serde = { version = "1.0", features = ["derive"] }
serde_json = "1.0"
tokio = { version = "1.49", features = ["full"] }
tracing = "0.1"
# ... all shared deps declared here, members use `workspace = true`
```

**Crate dependency flow:** `server` depends on `store` + `core`. `store` depends on `core`. `core` has no internal deps.

## Installation

```toml
# crates/core/Cargo.toml
[dependencies]
serde = { workspace = true }
serde_json = { workspace = true }
chrono = { version = "0.4", features = ["serde"] }
uuid = { version = "1.21", features = ["v4", "serde"] }
thiserror = "2.0"

# crates/store/Cargo.toml
[dependencies]
claude-history-core = { path = "../core" }
rusqlite = { version = "0.38", features = ["bundled", "serde_json", "trace"] }
tracing = { workspace = true }
thiserror = "2.0"
chrono = { version = "0.4", features = ["serde"] }

# crates/server/Cargo.toml
[dependencies]
claude-history-core = { path = "../core" }
claude-history-store = { path = "../store" }
axum = "0.8"
axum-extra = { version = "0.12", features = ["typed-header"] }
tokio = { workspace = true }
tokio-stream = { version = "0.1", features = ["sync"] }
tower-http = { version = "0.6", features = ["trace", "cors", "compression-gzip", "request-id"] }
hyper-util = { version = "0.1", features = ["tokio"] }
notify = "8.2"
notify-debouncer-full = "0.7"
clap = { version = "4.5", features = ["derive"] }
tracing = { workspace = true }
tracing-subscriber = { version = "0.3", features = ["env-filter", "json"] }
anyhow = "1.0"
serde = { workspace = true }
serde_json = { workspace = true }

[dev-dependencies]
tempfile = "3.25"
```

## Key Architectural Patterns

### JSONL Modeling with Discriminated Unions + Overflow

Claude Code's JSONL uses a `type` field as a discriminator. The standard pattern:

```rust
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum Message {
    #[serde(rename = "human")]
    Human(HumanMessage),
    #[serde(rename = "assistant")]
    Assistant(AssistantMessage),
    #[serde(rename = "system")]
    System(SystemMessage),
    // ... other variants
}

#[derive(Debug, Serialize, Deserialize)]
pub struct HumanMessage {
    pub message: MessageContent,
    // Known fields...

    /// Captures any fields not explicitly modeled.
    /// Critical for schema drift tolerance.
    #[serde(flatten)]
    pub overflow: HashMap<String, serde_json::Value>,
}
```

**Why this works:** `#[serde(tag = "type")]` + `#[serde(flatten)]` is supported since serde 1.0.46. The overflow `HashMap` captures unknown fields without losing data — essential for a tool that must survive Claude Code schema changes without code updates.

### SQLite with FTS5

rusqlite `bundled` feature compiles SQLite with `-DSQLITE_ENABLE_FTS5` unconditionally. No additional feature flags needed. Use `rusqlite::Connection` synchronously but offload to `tokio::task::spawn_blocking` for async integration:

```rust
let result = tokio::task::spawn_blocking(move || {
    conn.query_row("SELECT ...", params, |row| { ... })
}).await??;
```

### Axum SSE

axum 0.8 has built-in SSE via `axum::response::sse::{Sse, Event, KeepAlive}`. Pair with `tokio::sync::broadcast` channel + `tokio-stream::wrappers::BroadcastStream` for multi-client fan-out.

### Axum UDS (Unix Domain Socket)

`axum::serve` accepts `tokio::net::UnixListener` directly. Official example in axum repo. For cross-platform (including Windows named pipes), consider `tokio-listener` crate, but for unix-only daemon use, direct `UnixListener` is simpler and dependency-free.

### Daemon Management

Do NOT use a daemon management crate. The standard 2025 pattern for a Rust service daemon is:

1. **Foreground process** managed by systemd/launchd/etc.
2. **Signal handling** via `tokio::signal::ctrl_c()` + `tokio::signal::unix::signal(SignalKind::terminate())`
3. **Graceful shutdown** via `tokio::select!` + broadcast channel or `CancellationToken` from `tokio-util`
4. **PID file** (optional) via manual write, not a library

This matches how every major Rust service (deno, rust-analyzer, etc.) operates. External process managers handle daemonization; the binary handles graceful shutdown.

## Alternatives Considered

| Recommended | Alternative | Why Not the Alternative |
|-------------|-------------|------------------------|
| rusqlite (sync) | sqlx (async, compile-time checked) | sqlx's async model adds complexity for an embedded SQLite store. Compile-time query checking requires a live database at build time. rusqlite + spawn_blocking is simpler for single-connection embedded use. sqlx shines for Postgres/MySQL client-server setups. |
| axum 0.8 | actix-web 4.x | actix-web uses its own runtime (actix-rt), fragmenting the tokio ecosystem. axum is built by the tokio team, shares tower middleware, and has first-class SSE + UDS support. |
| axum 0.8 | warp | warp is effectively in maintenance mode. axum is its spiritual successor with better ergonomics and active development. |
| notify 8 | custom inotify/FSEvents | notify abstracts platform differences. Custom watchers are only justified for extreme performance needs this project does not have. |
| clap 4 derive | structopt | structopt was absorbed into clap 4. structopt is deprecated. |
| clap 4 derive | bpaf | bpaf is clever but niche. clap has overwhelming ecosystem adoption, shell completion generation, and documentation. |
| tracing | log + env_logger | log is fire-and-forget messages. tracing provides structured spans with temporal context — critical for diagnosing async request flows through axum + SQLite. |
| thiserror 2 | manual Error impls | thiserror eliminates boilerplate. v2 is current. |
| chrono | time (jiff) | chrono is more mature for ISO 8601 timestamp handling and has direct rusqlite integration via feature flag. jiff is newer and interesting but less ecosystem integration as of early 2026. |
| serde tag+flatten | serde untagged | Untagged tries variants in order (O(n) and fragile). Internally tagged is explicit and maps directly to Claude's `"type"` discriminator field. |

## What NOT to Use

| Avoid | Why | Use Instead |
|-------|-----|-------------|
| diesel | ORM overhead for an embedded SQLite tool. Schema DSL fights SQLite's dynamic typing. Compile times are painful. | rusqlite with raw SQL |
| reqwest | This is a server/daemon, not an HTTP client. No outbound HTTP needed. | N/A |
| rocket | Requires nightly Rust features historically, slower ecosystem adoption than axum, no tower middleware compatibility. | axum |
| daemonize crate | Unix-only fork() daemonization is an anti-pattern with modern init systems. Conflicts with tokio's runtime model. | tokio signal handling + systemd/launchd |
| daemon-slayer | Adds complexity for service install/management that belongs in deployment scripts, not the binary. Low adoption, unclear maintenance. | Manual PID file + systemd unit / launchd plist |
| sqlx | Async SQLite adds complexity without benefit for single-connection embedded use. Build-time database requirement is a DX burden. | rusqlite + spawn_blocking |
| serde_yaml / toml | The input format is JSONL. Config can be CLI flags + env vars via clap. No need for additional serialization formats. | clap + env vars |
| r2d2 (connection pool) | SQLite is embedded, single-writer. Connection pooling adds overhead without benefit. Use one connection per operation via spawn_blocking. | Direct rusqlite::Connection |

## Stack Patterns by Variant

**If adding WebSocket support later:**
- Add `axum::extract::ws` (built into axum, no extra deps)
- Because axum WebSocket uses the same hyper/tokio infrastructure

**If targeting Windows named pipes:**
- Add `tokio-listener` crate for unified listener abstraction
- Because direct `UnixListener` is unix-only; tokio-listener abstracts to named pipes on Windows

**If performance-profiling SQLite queries:**
- Enable rusqlite `trace` feature (already recommended above)
- Add `tracing-tree` for hierarchical span output during development
- Because SQLite trace callbacks integrate with tracing spans

**If the JSONL schema changes significantly:**
- The overflow `HashMap<String, Value>` absorbs unknown fields without breakage
- Add a `schema_version` detection pass before full deserialization
- Because forward compatibility is the primary defense against Claude Code updates

## Version Compatibility

| Package | Compatible With | Notes |
|---------|-----------------|-------|
| axum 0.8.x | tokio 1.x, tower-http 0.6.x, hyper 1.x | axum 0.8 requires hyper 1.x (not 0.14). tower-http 0.6 is the matching version. |
| rusqlite 0.38.x | libsqlite3-sys 0.36.x (SQLite 3.51.1) | bundled feature pins SQLite version. No system library needed. |
| notify 8.x | notify-debouncer-full 0.7.x, notify-types 2.x | Debouncer must match notify major version. |
| tracing 0.1.x | tracing-subscriber 0.3.x | These are paired; 0.1/0.3 is the stable combination. tracing 0.2 is not yet released. |
| clap 4.5.x | clap_derive 4.5.x | Derive macro version is locked to clap version automatically. |
| thiserror 2.x | Rust 1.65+ | v2 dropped MSRV below 1.56. Current stable Rust is well above this. |
| serde 1.0.x | serde_json 1.0.x | Always compatible within 1.x line. |
| tokio 1.49.x | All tokio-rs ecosystem | LTS policy ensures stability. |

## Sources

- [docs.rs/serde/1.0.228](https://docs.rs/crate/serde/latest) — version verified 2026-02-20
- [docs.rs/serde_json/1.0.149](https://docs.rs/crate/serde_json/latest) — version verified 2026-02-20
- [docs.rs/axum/0.8.8](https://docs.rs/crate/axum/latest) — version verified 2026-02-20
- [tokio.rs/blog/2025-01-01-announcing-axum-0-8-0](https://tokio.rs/blog/2025-01-01-announcing-axum-0-8-0) — axum 0.8 announcement, HIGH confidence
- [docs.rs/rusqlite/0.38.0](https://docs.rs/crate/rusqlite/latest) — version verified 2026-02-20
- [github.com/rusqlite/rusqlite build.rs](https://github.com/rusqlite/rusqlite/blob/master/libsqlite3-sys/build.rs) — FTS5 enabled unconditionally in bundled build, HIGH confidence
- [docs.rs/notify/8.2.0](https://docs.rs/crate/notify/latest) — version verified 2026-02-20
- [docs.rs/tokio/1.49.0](https://docs.rs/crate/tokio/latest) — version verified 2026-02-20
- [docs.rs/tracing/0.1.44](https://docs.rs/crate/tracing/latest) — version verified 2026-02-20
- [docs.rs/tracing-subscriber/0.3.22](https://docs.rs/crate/tracing-subscriber/latest) — version verified 2026-02-20
- [docs.rs/clap/4.5.59](https://docs.rs/crate/clap/latest) — version verified via libraries.io, HIGH confidence
- [docs.rs/tower-http/0.6.8](https://docs.rs/crate/tower-http/latest) — version verified 2026-02-20
- [docs.rs/chrono/0.4.43](https://docs.rs/crate/chrono/latest) — version verified 2026-02-20
- [docs.rs/uuid/1.21.0](https://docs.rs/crate/uuid/latest) — version verified 2026-02-20
- [docs.rs/thiserror/2.0.18](https://docs.rs/crate/thiserror/latest) — version verified 2026-02-20
- [docs.rs/anyhow/1.0.101](https://docs.rs/crate/anyhow/latest) — version verified 2026-02-20
- [docs.rs/notify-debouncer-full/0.7.0](https://docs.rs/crate/notify-debouncer-full/latest) — version verified 2026-02-20
- [serde.rs/enum-representations.html](https://serde.rs/enum-representations.html) — tag + flatten pattern, HIGH confidence
- [serde.rs/attr-flatten.html](https://serde.rs/attr-flatten.html) — overflow capture via HashMap flatten, HIGH confidence
- [github.com/serde-rs/serde/issues/1189](https://github.com/serde-rs/serde/issues/1189) — flattened internally tagged enum deserialization fixed in 1.0.46, HIGH confidence
- [docs.rs/axum/latest/axum/response/sse](https://docs.rs/axum/latest/axum/response/sse/) — SSE types: Sse, Event, KeepAlive, HIGH confidence
- [github.com/tokio-rs/axum unix-domain-socket example](https://github.com/tokio-rs/axum/blob/main/examples/unix-domain-socket/src/main.rs) — UDS pattern, HIGH confidence
- [tokio.rs/tokio/topics/shutdown](https://tokio.rs/tokio/topics/shutdown) — graceful shutdown pattern, HIGH confidence
- [lib.rs/crates/rusqlite/features](https://lib.rs/crates/rusqlite/features) — feature flag listing, HIGH confidence

---
*Stack research for: Rust CLI daemon — JSONL ingestion, SQLite store, HTTP/UDS/CLI API*
*Researched: 2026-02-20*
