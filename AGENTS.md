# Filavim engineering navigation

Filavim is a native Rust + GPUI editor with a reusable Ropey/modal editing core.
Prioritize correct editing, low input latency, low idle work and bounded retention.

**Before implementing a change, consult the architecture documentation, locate the
responsible subsystem, inspect its relevant interfaces and call sites, and only
then modify the implementation.** Start with [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md).
Use targeted `rg -n 'symbol' path/to/module.rs` and caller searches, not repeated
repository-wide scans. Check `src/lib.rs`, `src/gui/mod.rs` and Cargo target/feature
declarations to determine whether a source file is active in this frontend.

## Repository map

- `src/main.rs` -> `gui::run`: native entry, configuration, font/window bootstrap.
- `src/buffer.rs`, `editor.rs`, `input.rs`, `pairs.rs`: authoritative text/history,
  modal actions, keymaps and user-typing pairing; do not duplicate these in GPUI.
- `src/gui/model.rs`, `documents.rs`: per-document input/IME state and tab ownership.
- `src/gui/view.rs`: input routing, commands, effect dispatch, viewport/layout glue.
- `src/gui/services.rs`, `completion.rs`: syntax/LSP orchestration and completion sessions.
- `src/gui/toast.rs`, `toast_view.rs`: notification lifetimes and independent native
  overlay. Keep application notices separate from Editor's command-feedback channel.
- `src/lsp.rs`, `lsp_catalog.rs`, `lsp_manager.rs`: transport, curated tools and installation.
- `src/gui/viewport.rs`, `cursor*.rs`, `popup.rs`, `floating.rs`: coordinate mapping,
  cursor presentation, headless popup policy and native chrome.
- `src/gui/explorer.rs`, `navigation.rs`, `gui/io.rs`, `files.rs`: project/file work.
- `src/config.rs`, `appearance.rs`, `palette.rs`, `gui/theme.rs`, `gui/wallpaper.rs`:
  settings, semantic colors and cached imagery; font assets/licenses are in `assets/`.
- `tests/`, `src/gui/integration_tests.rs`, `scripts/test.ps1`: regression workflows.
  [docs/TESTING.md](docs/TESTING.md) describes exact commands, coverage and reporting limits.
- Historical terminal sources remain in `src/`; they are not the native entry path.
  Do not infer a working GPUI feature just from an old terminal implementation.

## Implementation boundaries

- Preserve Rust + native GPUI (pinned in Cargo), no WebView/embedded terminal or
  backend server. Avoid new dependencies or refactors without a concrete need.
- The core owns logical cursor/text/undo. GPUI owns focus, layout and visual cursor;
  animations must not determine edit positions or invalidate text layouts per frame.
- Buffer indices are Unicode scalars at grapheme boundaries, syntax spans are bytes,
  LSP positions use negotiated encoding, and GPUI IME ranges use UTF-16. Use the
  responsible conversion interfaces; do not interchange these units.
- Keep blocking file/process/network work off GPUI input/paint. Reuse existing
  executors/workers and validate document/revision/request identity on results.
- Preserve viewport shaping, stable completion sessions, coalescing and event-driven
  wakes. Do not add idle polling, unbounded caches or whole-buffer clones on typing.
- Use semantic theme roles and existing settings; preserve unrelated preferences.
- Use idiomatic `Result` for recoverable failures. Check pinned GPUI APIs locally
  before adapting upstream examples. Document non-obvious contracts with `///`,
  subsystem boundaries with `//!`, and implementation reasons with concise `//`.

## Validation and preservation

Preserve existing uncommitted work. Do not reset, commit unrelated changes, or alter
the terminal worktree. Keep editor behavior authoritative in the existing Rust core.

For substantive changes:
1. Identify the affected user behavior and nearest existing regression tests.
2. Add/update a regression, demonstrating failure before the fix when practical.
3. Implement the smallest correct change. Do not duplicate production logic in tests.
4. Run the relevant focused tests, then `pwsh -File scripts/test.ps1 -Mode Fast`.
5. Use `-Mode Full` for shared input, document, service, rendering or async changes.
6. Investigate failures. Do not delete tests, weaken assertions or retry until green.
7. Report exact commands/results, skipped/unexecuted checks, seeds and limitations.

Defaults are two Cargo build jobs and two test threads for a 16 GB host. Do not run
several Cargo builds concurrently or enable every feature combination speculatively.
GPUI tests use `gpui-tests`; normal release builds must not enable test features.
Use existing mock LSP fixtures and isolated temp paths. Routine tests must not access
user configuration, install tools, use the network or require a running language server.

Property failures shrink and persist in `tests/proptest-regressions.txt`; preserve
relevant seeds and add a readable deterministic regression for a confirmed bug.
Use `FILAVIM_PROPTEST_SEED`/`FILAVIM_PROPTEST_CASES` to reproduce/bound exploration.

Native windows require explicit `-Mode GUI -Interactive`; do not silently treat an
unavailable desktop as a pass. No screenshot claim without actually taking/checking it.
For performance-sensitive changes, use `-Mode Performance` before/after with the same
release build settings and host. Compare distributions; do not gate on noisy one-off
timings or label internal timings as input-to-display latency. See TESTING.md.

Build normally with `cargo build --locked`; Windows release uses
`pwsh -File scripts/windows-build.ps1 -Release` for SDK shader compiler discovery.
Documentation-only changes need formatting/build or rustdoc checks when Rust
comments change, not a speculative full GUI campaign.

## Documentation maintenance

Update the architecture guide when ownership, data flow or subsystem boundaries
change; update this file when structure or workflows change. Keep important API
contracts next to their implementation. Correct misleading comments when touching
that code. Ordinary internal changes need no architecture-document churn unless
they alter a documented contract. Link detailed feature/testing guides rather than
duplicating them here; describe implemented behavior, not future plans.
