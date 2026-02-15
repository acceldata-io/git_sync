# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Build & Development Commands

```bash
cargo build --release              # Release build (thin LTO, stripped)
cargo build                        # Debug build
cargo auditable build --release    # Release build with CVE tracking metadata
cargo test --verbose               # Run all tests
cargo test <test_name>             # Run a single test
cargo fmt --all -- --check         # Check formatting
cargo clippy                       # Lint (CI runs with RUSTFLAGS=-A warnings)
cargo audit --ignore RUSTSEC-2023-0071  # Security audit
cargo doc --no-deps --open         # Generate API docs
```

## Project Overview

**git_sync** is a Rust CLI tool for managing operations across multiple GitHub repositories — syncing forks, managing branches/tags, creating releases, opening PRs, running repo health checks, and backing up to local/S3. It uses a TOML config file (`git-manage.toml` via XDG) to define repository groups.

## Architecture

### Module Structure

- **`cli.rs`** — clap-based argument parsing with hierarchical subcommands (`branch`, `tag`, `repo`, `pr`, `release`, `generate`, `config`)
- **`config.rs`** — TOML config loading with XDG Base Directory support and env var overrides
- **`error.rs`** — `GitError` enum (thiserror) with `UserError` conversion for human-readable output; categorizes errors (Auth, RateLimit, Network, etc.) with suggestions
- **`init.rs`** — Config file generation
- **`github/`** — GitHub API layer:
  - `client.rs` — octocrab wrapper, single instance per run
  - `match_args.rs` — dispatches CLI commands to client methods
  - `backup.rs`, `branch.rs`, `tag.rs`, `fork.rs`, `pr.rs`, `release.rs`, `check.rs` — domain-specific operations
- **`utils/`** — Shared utilities:
  - `macros.rs` — `handle_api_response!`, `handle_futures_unordered!`, `async_retry!`
  - `repo.rs` — `RepoInfo`, `TagInfo` types and HTTP status extraction
  - `compress.rs`, `filter.rs`, `file_utils.rs`, `tables.rs`, `pr.rs`
- **`slack/`** — Slack webhook notifications

### Key Patterns

- **Async everywhere**: Tokio multi-threaded runtime; semaphore-based concurrency control for API rate limiting; configurable parallel jobs (1-64)
- **Retry with backoff**: `async_retry!` macro wraps `tokio_retry::RetryIf` with exponential backoff and jitter; `is_retryable()` filters transient errors
- **Concurrent futures**: `handle_futures_unordered!` macro for processing collections of repos concurrently via `FuturesUnordered`
- **API response handling**: `handle_api_response!` macro standardizes octocrab error mapping to `GitError`
- **TLS**: Forces aws-lc-rs as the default TLS provider (overrides ring in dependencies)
- **Error exit codes**: `MultipleErrors` exit code = min(error_count, 255); partial failures (fewer errors than repos) exit 0

### Feature Flags

- `aws` — S3 backup support (aws-sdk-s3)
- `slack` — Slack notification support
- Both enabled by default

### Environment Variables

- `GITHUB_TOKEN` — GitHub PAT
- `SLACK_WEBHOOK` — Slack webhook URL
- `CONFIG_FILE` — Path to config file

## Code Style

- Rust edition 2024, MSRV 1.86+
- Clippy warnings enabled: `clippy::all`, `clippy::pedantic`, `missing_docs`
- `clippy.toml` sets `too-many-lines-threshold = 150`
- All source files carry the Apache 2.0 license header
- Unix-only (not Windows compatible)
