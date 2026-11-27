# GPUI Checkpoint 2

Historical report. The checkout details, capabilities, and validation below describe that checkpoint.

Checkpoint 1 baseline: `48a8b89` on `gpui-migration`. Terminal baseline `dbcfd3a`
on `master` remains unchanged. This is an implementation record; validation and
final status are updated at completion.

## Source-backed inventory before implementation

| Original subsystem | Evidence | Checkpoint 1 | Migration direction |
| --- | --- | --- | --- |
| Rope, graphemes, grouped undo, modal commands | buffer.rs, editor.rs, input.rs | Migrated | Preserve |
| Multiple buffers, independent state, splits/tab pages | workspace.rs, app.rs | Single document only | Native document tabs/state; split presentation remains outstanding |
| Explorer, ignore-aware index, fuzzy picker, regex grep, MRU | navigation.rs, storage.rs, app.rs | Not exposed | Reuse background indexing/search; native tree/pickers |
| Rust/Python/JavaScript/JSON Tree-sitter | syntax.rs | Not exposed | Reuse coalescing/incremental worker and viewport span projection |
| JSON-RPC/LSP, incremental sync, cancellation, completion/locations/hover/diagnostics | lsp.rs, ide.rs | Not exposed | Reuse transport/range/edit helpers with native response ownership |
| Rename/format/code actions/snippets/signature | ide.rs, snippet.rs, contextual.rs | Not exposed | Expose where reliable; distinguish outstanding workflows |
| Git status/diff/line indicators/stage/hunks/commit/previous | git.rs, ide.rs | Not exposed | Reuse command backend; restrained native commands/results |
| TOML settings and keybindings | config.rs, setup.rs | Core settings only | Restore service schemas and existing persistence helper |
| File/directory create/rename/delete | storage.rs/app.rs | Not exposed | Preserve original source; assess safe native access |
| External file changes | No watcher in original; explicit :e! reload | Explicit reload | Keep explicit reload; do not invent automatic watcher |
| Replace and command history | No general substitute or command-history implementation found | Not present | Record absent, not migrated |
| PTYs, terminal fonts/setup, OSC52, terminal backend | terminal.rs/font/runtime | Excluded | Not applicable/excluded for native frontend |
| Cursor effects | animation.rs/pixiedust.rs | Excluded | Excluded in Checkpoint 2 |

## Implemented native integration

- `gui/documents.rs` centrally owns real `Model`/`Editor` instances. Switching
  moves ownership instead of copying document text. Each document retains its
  cursor, selection, viewport, key state and grouped undo/redo. Identities are
  monotonic, including reloads and replacement of the last document.
- Graphical document tabs have modified indicators, close controls and horizontal
  scrolling. Ctrl-Tab / Ctrl-Shift-Tab and existing buffer leader bindings switch
  documents. Ctrl-W closes a document. Save As rejects a path owned by another
  open document. Save/search completions apply only to their original document.
- Window closure walks unsaved documents through Save/Discard/Cancel prompts.
  Saving a snapshot cannot mark newer edits clean. LSP shutdown is requested
  before application exit, with a bounded grace period and the original kill guard.
  Restarted clients remain owned until shutdown completes, including restart-then-quit.
- `gui/explorer.rs` reuses the original ignore-aware bounded index and cancellable
  tree projection. Workers are created lazily. GPUI `uniform_list` virtualizes rows;
  collapsed directories do not generate native widgets. Indexing traverses the
  project in the background (it is bounded full indexing, not lazy filesystem IO
  per directory). Native divider dragging and `:SidebarWidth N` resize the sidebar.
- `gui/services.rs` adapts existing syntax/LSP services. Incremental/coalesced syntax
  parsing stays off the UI thread; visible clipped text receives native GPUI text
  runs. Unaffected highlighting is projected through pending edits. Closed,
  replaced, hidden and stale generations cannot install results in another buffer.
- The original LSP client owns process/JSON-RPC/document-sync transport. One server
  is reused per configured name in the current project. Native orchestration
  validates document/revision/cursor/mode/request identities, fairly drains up to
  32 events with a 2 ms collection budget, and delegates diagnostic/completion/
  location/workspace-edit preparation to one shared coalescing worker. This is a
  collection budget, not a hard upper bound on all UI work.
- Diagnostic publications coalesce per server/document. Retired diagnostic data
  and obsolete response values are destroyed on the preparation worker. Cancellation
  now wakes the original worker to release retired data even when otherwise idle.
  Completions keep at most 200 candidates and discard unused raw documentation on
  the worker. Diagnostics cap at 50,000 with an explicit status message; references
  cap at 10,000 and code actions at 200.
- Native diagnostic markers/counts/results (first 2,000 navigation rows), hover, signature, definition,
  implementation, references, debounced completion, formatting, rename and edit-based
  code actions reuse original encoding/edit/snippet helpers. Workspace edits are
  prevalidated before any open buffer is mutated; all captured document identities
  must remain current. Server-initiated workspace edits are explicitly rejected.
- Native command/search bars reuse the original modal keymap and editor commands.
  Async command submission invalidates the view immediately; unavailable-LSP messages
  are displayed without waiting for a subsequent background event.
  Literal search runs asynchronously; visible matches are highlighted. `:Files`
  and `:Grep` use the original fuzzy ranking and regex grep. Buffers and Commands
  use the same results UI and dispatch system.
- Git status, bounded diff previews, stage/unstage and explicit commit commands run
  on GPUI's background executor using the existing Git implementation and output
  limits. Formatting large Git results also stays on that task. Staging a dirty
  document requires saving it first. No periodic Git refresh is added.
- `:set key=value` uses the existing validated, atomic TOML persistence helper and
  updates all open editors. Existing native configuration location is preserved:
  `%APPDATA%/filavim-gpui/config.toml`, overridable by `FILAVIM_GPUI_CONFIG`.
  Original LSP/Git/navigation sections are available. Terminal configuration is
  not loaded or rewritten.
- `paths.rs` extracts the original terminal-independent path helpers; targeted changes in original LSP/Git integration include those helpers, background didSave serialization, and boxing the rare LSP send-error return for current Clippy. No terminal renderer, terminal
  emulator, second text buffer, GUI runtime/server, or cursor effect is introduced.

## Feature parity after implementation

| Original capability | GPUI status | Scope/remaining gap |
| --- | --- | --- |
| Rope/graphemes/modes/operators/undo/redo/clipboard | Migrated | Existing engine; native IME boundary retained |
| Open/save/Save As/dirty protection/reload | Migrated | Atomic IO; `:e!` reload retires old identity |
| Multiple buffers and navigation | Migrated | Native document tabs, per-document state |
| Split layouts and terminal tab pages | Not yet migrated | Original source preserved; native document tabs are not split/tab-page parity |
| Explorer/project navigation | Migrated | Background ignore-aware index, virtual tree, keyboard/mouse, resizable sidebar |
| Fuzzy files/regex grep/buffer/command results | Migrated | Query through command bar; no continuously updating fuzzy-query overlay |
| MRU/history/project launcher | Not yet migrated | Original storage orchestration is not activated |
| Filesystem create/rename/delete dialogs | Not yet migrated | Avoids incomplete destructive workflows |
| Rust/Python/JS/JSON syntax | Migrated | Original grammars, incremental worker, viewport text runs; default 2 MiB syntax limit |
| LSP lifecycle/sync/cancellation | Migrated with constraints | One current project root per window; external servers must already be installed |
| Diagnostics/hover/locations/signature | Migrated | Native bounded results; plain documentation, not a Markdown renderer |
| Completion | Migrated with constraints | Real-server-tested automatic/manual floating menu, asynchronous item resolve, replacement/additional edits and snippet expansion; completion commands and snippet-field cycling outstanding |
| Formatting/rename/code actions | Partially migrated | Explicit acceptance of edit-based results; no range-format UI, action resolve/execute-command, or server-initiated applyEdit |
| Git | Partially migrated | Status/file stage/unstage/commit and bounded diff preview; no hunk controls, previous-revision UI or live gutter refresh |
| TOML/keybindings/theme/edit/service settings | Migrated with constraints | Native schema plus reused service sections; no wholesale import of terminal-only settings; terminal explorer-width units are retained but native width is controlled by divider/command |
| External file watching | Not present originally | Explicit `:e!`; no new watcher |
| General substitute/command history | Not found in original | Not claimed as migrated |
| PTY/OSC52/terminal font installers | Not applicable | Native frontend does not use terminal fonts or terminal rendering |
| Cursor effects | Excluded | Simple existing nonanimated cursor only |

## Commands and interaction

- `filavim [FILE|DIRECTORY]`, Ctrl-O: file picker; Ctrl-Shift-O / `:OpenProject`:
  project picker. Relative `:e`/`:w` paths resolve against the current project.
- Ctrl-Tab / Ctrl-Shift-Tab, `:bn` / `:bp`: switch; Ctrl-W / `:bd`: close;
  `:new`: new independent document. `:q` closes the current document (quits when
  last); `:qa` safely quits all. `:q!` / `:bd!` explicitly discard the current one.
- Ctrl-S / `:w`: save; Ctrl-Shift-S: Save As; `:wq`: save/close; `:e!`: reload.
- `:Explorer`, Ctrl-E: show/focus explorer. W/S or arrows and Enter navigate;
  Escape returns to editor. Mouse opens files/toggles directories. `:ExplorerFocus`
  focuses it directly; divider drag or `:SidebarWidth 300` changes width.
- `/literal`, `n`: search and next match; `:Files query`, `:Grep regex`:
  project results after indexing. `:Buffers`, `:Commands`: native results.
- `:Diagnostics`, `:Hover`, `:Definition`, `:Implementation`, `:References`,
  `:Signature`, `:Complete` / Ctrl-Space, `:Format`, `:Rename new_name`,
  `:CodeActions`, `:LspInfo`, `:LspRestart`. Existing corresponding leader keys
  are reused. Results use floating panels: Up/Down or Ctrl-N/P, Enter/Tab accept, Escape dismiss. `:Diagnostics` shows current-line diagnostics; `:DiagnosticsAll` opens the navigation picker.
- `:GitStatus`, `:GitDiff [staged]`, `:GitStage`, `:GitUnstage`,
  `:GitCommit message`. A preview is intentionally bounded and is not a full diff editor.
- `:set font_size=20`, `:set lsp.enabled=false`, etc. Validation precedes atomic
  persistence; arbitrary unsupported settings are rejected.

## Validation

- `cargo check --all-targets`: passed.
- `cargo fmt --check`: passed; formatting normalized throughout compiled targets.
- `cargo clippy --all-targets --all-features -- -D warnings`: passed.
- `cargo test --locked -- --test-threads=1`: **59 passed**, five opt-in tests ignored (four benchmarks and one real-server test).
- `cargo test --no-default-features --locked -- --test-threads=1`: **58 passed**,
  five opt-in tests ignored. This validates independent core/service integration
  without linking GPUI; the GPUI-specific keystroke translation test is excluded.
- `cargo test --release --locked -- --test-threads=1`: **59 passed**, five opt-in tests ignored.
- `cargo build --locked`: passed. `scripts/windows-build.ps1 -Release` and
  `cargo build --release --locked` with the existing isolated FXC path: passed.
- `--help` and `--version`: exit 0; version `filavim-gpui 0.4.0`.
- Debug and release real-window smoke runs: passed synthetic mode/text/undo/redo/
  selection, cursor hit-testing, scroll/resize, two-document preservation,
  project indexing and asynchronous Save. The release fixture had 80,035 scalar
  characters and 5,005 lines, including tabs, Unicode and a 5,000-character line.
  Saved output SHA-256 exactly matched the unchanged source fixture.
- Initial Checkpoint 2 release settled-frame counts were **10 → 10 over two idle seconds**, with
  23 cached viewport rows. This verifies no unsolicited application frames in that
  interval, not zero framework/driver CPU.
- LSP integration fixture runs two processes for Rust/Python, 20,000-entry diagnostic
  floods and reference responses, incremental sync, completion/snippet expansion,
  hover, formatting/undo, stale response rejection, and restart-then-quit with a
  delayed shutdown response. This mock fixture alone does not establish real-server compatibility; the separate real rust-analyzer verification below supplies that evidence.
- Git and settings tests use temporary repositories/configurations only. No actual
  user Git index, font installation, or terminal settings were changed.

## Measurements (before the floating UI addition)

Windows 11 / i5-12400F (12 logical processors) / Radeon RX 7600, same host as
Checkpoint 1. All timing is observational. No profiler/allocation counter or
GPU/display-latency instrument was available in these checks.

Three repetitions of each release harness collected 10,000 individual samples
per workload, then sorted them for each run. The following are ranges of the
three **per-run sample percentiles**, not percentiles of run averages:

| Headless workload | p50 µs | p95 µs | p99 µs | Largest observed sample µs |
| --- | --- | --- | --- | --- |
| Existing navigation + viewport fragment | 19.5–19.6 | 36.8–38.2 | 42.4–43.7 | 201.6 |
| Existing mixed Unicode typing + fragment | 39.6–41.6 | 76.2–83.4 | 105.3–110.5 | 359.0 |
| New document/services replay, background load off | 35.3–35.6 | 63.0–74.0 | 100.7–103.6 | 397.3 |
| Same replay, syntax + diagnostic preparation on | 37.4–40.3 | 73.8–97.8 | 90.8–114.5 | 429.5 |

The existing harness navigates in the middle of a one-million-character ASCII
line, then inserts mixed ASCII/emoji/combining text. The new replay performs edits,
service synchronization/polling and viewport clipping over a 10,000-line Rust
fixture; loaded runs also submit 50,000 synthetic diagnostics and incremental
syntax work. It does not run GPUI shaping, GPU presentation, real LSP IPC or a
human input queue. Noise/cache/scheduling variation is visible (including a loaded
run faster than an unloaded run). These observations support comparable backend
latency scale, not a proven systematic speedup or an end-to-end latency guarantee.
Prior Checkpoint 1 typing p99 was 111.3 µs; navigation p95/p99 was 25.0/52.0 µs.

| Native release process measurement | Result | Prior Checkpoint 1 |
| --- | --- | --- |
| Spawn → native window metadata, three runs | 587.26 / 251.59 / 245.90 ms | 396.98 / 215.20 / 202.95 ms |
| Median of those three startup observations | 251.59 ms | 215.20 ms |
| Resident memory, startup runs after settling | 65.09 / 56.04 / 55.96 MiB | 93.11 / 55.31 / 55.07 MiB |
| Same original file, four-second settle then ten-second idle | 55.83 MiB resident / 59.75 MiB private; 125 ms CPU / 10,000.16 ms wall | 54.79 MiB resident / 59.44 MiB private; 218.75 ms CPU / 10,001.98 ms wall |
| Own Rust project directory open, empty document, same idle method | 55.27 MiB resident / 54.80 MiB private; 109.375 ms CPU / 10,005.94 ms wall | Not measured |
| Two-document native smoke with syntax and fixture explorer | 59.30 MiB resident / 75.78 MiB private at one sample | Not measured |

Startup was slower in this sample: the median was about 36 ms higher. Three
uncontrolled cache-warm observations cannot attribute that to an individual
subsystem. File-idle CPU was about 1.25% of one logical core (~0.10% of host CPU),
versus the previous 2.19%; no causal CPU improvement is claimed. LSP was disabled
for native process comparisons so external servers cannot contaminate memory/CPU
accounting. The project-directory measurement includes the explorer/index and an
empty document; it is not a many-buffer/LSP programming-session measurement.
GPU allocations, system compositor, cold OS-cache startup and long-session peak
memory are excluded/unmeasured.

Raw local artifacts (ignored under `target/`): `checkpoint2-benchmarks.txt`,
`checkpoint2-release-smoke-final.txt`, `checkpoint2-smoke-memory.json`, and
`checkpoint2-measurements/windows-native-perf.json`.

Reproduction (run serially; avoid concurrent builds while measuring):

```powershell
$env:GPUI_FXC_PATH = "$PWD/.local/tooling/fxc/fxc.exe"
cargo test --release native_boundary_performance -- --ignored --nocapture --test-threads=1
cargo test --release native_services_latency_replay -- --ignored --nocapture --test-threads=1
./scripts/windows-native-perf.ps1 -File ./target/native-launch-fixture.txt -Project .
```

The process script only writes an isolated configuration/artifacts under target,
restores its environment override, launches bounded test processes and closes only
those processes. It does not change actual user configuration.

## Git state and review

Checkpoint 1 commit: **48a8b89**, `Checkpoint 1: native GPUI editor foundation and
approved typography`. Current branch: `gpui-migration`. Checkpoint 2 changes are
intentionally uncommitted for review. Nothing was pushed or merged. `master` in
the original worktree remains at `dbcfd3a` with no tracked changes; its pre-existing
untracked file is preserved.

Primary additions are `gui/documents.rs`, `gui/explorer.rs`, `gui/services.rs`,
`paths.rs`, `tests/checkpoint2.rs`, and `scripts/windows-native-perf.ps1`. Integration
changes are in native view/application/IO/theme, service configuration, Cargo
manifests, module exports, and the original backend path/wakeup helpers.

Remaining performance questions for review include GPUI shaping/GPU/display
latency, many-buffer service bookkeeping, stale large-index disposal on project
switches, diagnostic-heavy panel construction, and long-lived real-server sessions.
These need representative profiling before further architectural changes.


## Known limits and manual verification

Windows is the only platform compiled/launched here. GPUI remains pinned to
0.2.2 through Cargo.toml/Cargo.lock. Release builds use
`./scripts/windows-build.ps1 -Release`; this host needs the already isolated,
ignored FXC tooling described in Checkpoint 1. No SDK installation was performed.

Native smoke tests launch real Windows windows and call native view handlers in
memory; they do not simulate physical OS keyboard/mouse input and do not establish
visual approval, human IME behavior, display latency, or multi-monitor DPI behavior.
No screenshots or interactive visual inspection were used.

Remaining manual checks: use a real Rust project and installed rust-analyzer;
exercise explorer mouse/keyboard/dragging, off-screen tabs, completion acceptance,
Visual selections, actual Save/Discard/Cancel prompts across multiple dirty buffers,
project switching, real IME/AltGr, long Unicode lines, DPI changes and native focus.
A project switch restarts services under the selected project root while retaining
open documents; independent multi-root workspaces remain outstanding.

Advanced original features above remain honest parity gaps rather than being
replaced with shallow placeholder UIs. No Checkpoint 3/4 work was started.


## Floating UI and autocomplete addition

Before this addition, GPUI already requested real LSP completion, supported
keyboard acceptance and exposed hover/diagnostics through a docked result strip.
Its automated evidence used mock servers; real rust-analyzer compatibility had
not been established. The strip has now been replaced by in-window native GPUI
overlays, not separate OS windows.

- `gui/popup.rs` supplies logical-pixel placement and keyboard policy: caret menus
  flip/clamp inside the viewport; selection pickers are centered. Layout uses
  current-frame shaped caret geometry before deferred overlay painting.
- Completion has virtualized rows, kind/type labels, selected-row highlighting,
  keyboard and mouse acceptance, side documentation, and F1 documentation view
  when the window is too narrow for side panels. Enter/Tab acceptance remains
  configurable. Escape cancels requests and delayed triggering. Insert text
  automatically requests suggestions after debounce; Ctrl-Space requests explicitly.
- Item resolution runs through the existing transport/preparation worker. Pending
  acceptance waits without blocking; document/revision/cursor/mode/request/selection
  validation rejects stale results and cancellation cannot insert late text.
- Hover/signature and current-line multi-severity diagnostics use contextual,
  scrollable information panels. Code actions use contextual menus. Commands,
  files, buffers, grep, references/navigation and all-diagnostic results use
  centered pickers. Save/Discard/Cancel uses an internal confirmation overlay.
  Native OS file-open/Save As dialogs remain appropriate native file pickers.
- Modal menu input is consumed before editor commands; passive information panels
  allow editing and dismiss when obsolete. IME composition suppresses suggestions.
  The status line retains mode/file/cursor/modified/progress information only.
- Real-server testing exposed Windows drive-letter URI differences in diagnostics;
  document matching now uses decoded platform path identity. didSave notifications
  honor advertised capabilities and serialize optional text off the UI thread.

Configuration (existing config location, no migration needed):

```toml
[completion]
auto_trigger = true
accept_enter = true
accept_tab = true
```

Verification added: four popup placement/key-policy unit tests, configuration and
multiple-diagnostic tests, delayed resolve/cancel/save-notification mock coverage,
and an opt-in real rust-analyzer integration test. Native bounded smoke runners
exercise overlay prepaint, resize, modal input isolation, confirmation cancellation,
and automatic completion through the actual view input handler.

Real rust-analyzer 1.93.1 (01f6ddf7 2026-02-11) returned `compute` for an isolated
Rust fixture. Acceptance produced `widget.compute()` with no raw snippet syntax;
hover documentation and fresh error diagnostics were verified. The real-server
backend test passed in 4.33 seconds (test duration, not completion latency).
The debug real-window replay also passed at a resized 620×380 viewport with
resolved F1 documentation and unchanged on-disk fixture. These are synthetic
handler/protocol tests; they do not establish human input, appearance or IME quality.

Reproduce, with rust-analyzer installed on PATH:

```powershell
cargo test --test floating real_rust_analyzer -- --ignored --nocapture --test-threads=1
./scripts/windows-lsp-smoke.ps1
./scripts/windows-lsp-smoke.ps1 -Executable ./target/release/filavim.exe
```

Debug ordinary native smoke settled at 15 painted frames and remained at 15 over
two idle seconds; this is an observed absence of unsolicited application frames,
not a CPU/display latency measurement. Earlier tables in this report predate the
floating addition; no floating UI input-to-display, allocation or GPU timing claim
is made.

Remaining constraints: bounded plain-text documentation, no Markdown rendering;
no snippet placeholder cycling or completion-item command execution; no code-action
resolve/executeCommand. Strict diagnostic revision validation hides older markers
until a fresh publication; servers may deduplicate unchanged diagnostics, so
unchanged markers can remain absent after edits. includeText didSave is withheld
if a newer dirty buffer would misrepresent the saved snapshot. Actual mouse,
physical keys, IME/AltGr, multi-monitor DPI, popup aesthetics and Linux/macOS still
need manual/platform verification. No cursor effects or later checkpoints added.

Final floating-addition validation: formatting and Clippy with warnings denied
passed; debug/default tests passed 59, release tests passed 59, and no-default
core/service tests passed 58 (five opt-in tests excluded in each applicable suite).
Both debug and release builds succeeded. Release real-server native smoke passed
at stage 9 with 17 painted frames and unchanged fixture contents. Release ordinary
native smoke passed picker, modal input, confirmation, information, geometry,
project and Save checks; settled frames stayed 12 → 12 over two idle seconds.
Artifacts: `target/floating-release-smoke.txt` and
`target/floating-lsp-fixture/native-lsp-report.txt`.

Final floating-build process sample (same release script, run after builds/tests,
LSP disabled): window metadata appeared in 240.63 / 244.53 / 240.39 ms (median
240.63 ms). Settled file process used 56.64 MiB resident / 60.20 MiB private and
140.625 ms CPU over 10,008.05 ms wall (~1.405% of one logical core). Indexed project
with an empty buffer used 55.48 MiB resident and 125 ms CPU over 10,006.21 ms wall.
Before-floating observations were 251.59 ms median startup, 55.83 MiB file resident
and 125 ms CPU/10 seconds; these small uncontrolled differences establish neither
a speedup nor a meaningful regression. This sample does not measure an open popup,
real-server memory, input-to-display p95/p99, first paint or GPU allocations.
Raw sample: `target/checkpoint2-measurements/windows-native-perf-floating.json`.
