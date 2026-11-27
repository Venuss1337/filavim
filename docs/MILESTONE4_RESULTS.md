# Milestone 4 implementation results — Windows x64, 2026-11-16

Milestone 4 is **partial**, not complete. Existing unrelated checkout changes were
preserved. No desktop automation or Computer Use was performed.

## Implementation and architecture

- `src/lsp_catalog.rs`: required five servers, platform matching, consistent primary
  defaults, retained legacy alternatives and catalog invariants.
- `src/lsp_manager.rs`, `src/lsp_install.rs`: shared installer extended with clangd,
  managed Node LTS, locked isolated npm, integrity checks, bounded extraction,
  redirect validation, streaming byte progress and cached update hints. Existing
  generation receipts, atomic activation and cross-process locking are reused.
- `src/gui/manager_view.rs`: direct installation action, official project link,
  accurate platform details and unsupported-platform disabling. Existing searchable
  native manager, controls, status, cancellation and focus ownership are retained.
- `src/lsp.rs`, `src/gui/services.rs`: meaningful configuration responses, truthful
  applyEdit capabilities, code-action resolve and advertised command execution,
  declaration/type-definition/symbol/range-format requests, diagnostic navigation,
  crash diagnostic cleanup, malformed completion/action guards and no-result errors.
- `src/snippet.rs`, `src/editor.rs`, `src/gui/model.rs`, `src/gui/view.rs`: retained
  placeholder sessions, selection, mirrors, nested defaults, Tab/Shift+Tab, safe
  cancellation and compatibility with automatic pairs/IME/document revisions.
- `src/syntax.rs`: maintained TS/TSX/C/C++ grammars and required extension mapping.
- Protocol, local HTTP, hostile archive, configuration, catalog, snippet and
  headless native tests extend existing Cargo targets and mock fixtures.

See [LANGUAGE_INTELLIGENCE.md](LANGUAGE_INTELLIGENCE.md) for usage, trusted sources,
storage, runtimes, security guarantees, licensing and maintainer instructions.
No new test executable or testing framework was introduced.

## Real-server acceptance

`cargo test --locked --no-default-features --lib required_catalog_install_and_completion_acceptance -- --ignored --nocapture --test-threads=1`

One passed acceptance test, zero failures, 111 filtered out; 51.88 seconds.
Official downloads installed and initialized rust-analyzer **2026-11-16**, clangd
**23.1.0**, typescript-language-server **6.1.0**, Pyright **1.1.414**, and
vscode-json-languageserver **1.3.4**. All five automatically activated, returned
useful completion, accepted completion after further typing, published current
diagnostics and shut down. The four programming-language servers returned definition
locations. JSON did not advertise definition. Managed Node fallback was exercised;
the system Node was too old. All storage was temporary and removed after testing.

Earlier acceptance attempts failed and informed fixes: clangd display labels had
leading whitespace; npm rejected a shared user/global configuration pathname; an
acceptance assertion incorrectly matched a declaration before insertion completed.
The final test waits for an actual revision change. Navigation retries account for
real asynchronous indexing and content-modified responses.

## Regression validation

- `pwsh -NoProfile -File scripts/test.ps1 -Mode Full`: **187 passed, 0 failed,
  13 ignored**. Includes formatting, all-target warnings-denied Clippy, properties,
  mock transport, existing editing/recovery/search/file-safety and headless GPUI.
- `pwsh -NoProfile -File scripts/test.ps1 -Mode Fast`: **131 passed, 0 failed,
  9 ignored**, including formatting and warnings-denied core Clippy.
- `pwsh -NoProfile -File scripts/test.ps1 -Mode LSP`: **19 passed, 0 failed,
  1 ignored** (3 protocol plus 16 document/protocol integration).
- `cargo build --locked`: passed, native debug build and link completed.
- Real installation remains opt-in; ordinary tests use local HTTP and isolated
  filesystem fixtures, never internet server downloads.
- Full-tree `git diff --check` reports pre-existing trailing whitespace in
  `examples/performance.rs:43-45`. The milestone's tracked files pass the scoped
  check; unrelated whitespace was preserved.

Machine-readable final profile reports are in `target/test-reports`. Actual server
logs are in `target/language-acceptance.log`; these are local build artifacts.

## Performance

Optimized release builds, Windows x64, synthetic isolated operations, single test
thread; no GPU, process startup, network or user-perceived latency claims:

- `cargo test --release --locked --no-default-features --lib language_intelligence_operation_timings -- --ignored --nocapture --test-threads=1`:
  nine-entry catalog, 10,000 filters: 23,521 microseconds total; 45-byte snippet,
  10,000 parses: 17,781 microseconds total. One test passed.
- `cargo test --release --locked --no-default-features --lib completion_filter_latency -- --ignored --nocapture --test-threads=1`:
  10,000 candidates, 100 samples: p50 2,256 microseconds, p95 3,282 microseconds;
  with 64 journal edits p50 2,249 microseconds, p95 3,156 microseconds. One test passed.
  These are operation measurements, not evidence of a comparative improvement.

## Material remaining work / next milestone readiness

Resource create/delete/rename workspace edits are explicitly rejected; native
destructive-operation confirmation and file-safety integration remain unfinished.
Server-initiated applyEdit is deliberately unadvertised and rejected. Commands
depending on it are consequently unsupported. Automatic crash retry, bounded old
generation retention, runtime garbage collection and a user-facing rollback selector
are not implemented. Dynamic registrations beyond completion are declined.
Snippet variables/choices/transformations are unsupported explicitly.

Real formatting, rename and code-action acceptance across all five tools, exhaustive
installer rollback/concurrency/timeout fault injection, complete native manager
progress/control tests, non-Windows installation and desktop appearance/OS focus
behavior remain unverified. The new mock action test covers resolve, validated edit,
command ordering, undo, unsaved disk preservation and stale-action rejection.

New dependencies: tar and tree-sitter TypeScript/C/C++ grammars; curl and managed
Node/npm are external runtime tools. See the language guide for license obligations.
The foundation supports continued development, but closing these safety and
acceptance gaps should precede declaring Milestone 4 complete or treating Milestone
5 project workflows as fully unblocked.
