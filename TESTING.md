# Filavim testing — historical migration notes

The current contributor guide is [docs/TESTING.md](docs/TESTING.md). Results below are historical observations, not verification of the current tree. Routine correctness tests are headless; desktop/GUI smoke is separate acceptance work.

## Technical plan and inventory

Preserve the existing Ropey editor and native GPUI 0.2.2. Reuse `tests/editor.rs`,
`native.rs`, `checkpoint2.rs`, `floating.rs`, `lsp_manager.rs`, in-module regressions,
the standalone JSON-RPC fixture, and `--smoke-test`. Existing terminal-only tests
not registered in this migration's Cargo manifest are not silently counted as run.

Highest-risk gaps: actual GPUI input/focus routing (adapter tests alone miss it),
Unicode edit/undo combinations, completion interaction across frames, and reporting
that distinguishes unavailable GUI runs from passes. Add small tests at those
boundaries, not another editor implementation.

Implementation order: bounded property tests; opt-in GPUI `test-support` tests using
the real EditorView; reusable mock-process fixtures; resource-limited runner/reports;
explicit performance samples; native smoke wrapper; CI and agent instructions.

GPUI test source is verified against the locally pinned 0.2.2 crate, not just Zed's
current main branch. Zed's test-context pattern informs fixtures and simulated keys;
no Zed/Neovim backend or test framework is imported.

## Commands

Run from this worktree in PowerShell 7 (`pwsh`). Defaults: two Cargo build jobs,
two test threads, 64 cases per property, reproducible seed 4045029414.

```powershell
pwsh -File scripts/test.ps1 -Mode Fast
pwsh -File scripts/test.ps1 -Mode Full
pwsh -File scripts/test.ps1 -Mode Full -CoreOnly
pwsh -File scripts/test.ps1 -Mode GUI -Interactive
pwsh -File scripts/test.ps1 -Mode Performance
```

Fast runs formatting, warnings-denied featureless Clippy, core library and editor
regressions. Full includes all registered correctness tests, bounded properties,
mock processes and real GPUI TestWindow tests. CoreOnly excludes native GPUI and
is used on Linux/macOS CI. Performance runs ignored release benchmarks serially;
it does not run all ignored tests (some download executable software).

GUI builds the normal Windows release with the existing SDK-aware build script,
then launches one self-terminating `--smoke-test` process against an isolated file.
It exercises native input handlers, undo, cursor/resize geometry, project/save,
pickers and information/confirmation panels. The original fixture hash must remain
unchanged. Save output stays beside the fixture under target. A supplied
`-Executable C:/path/filavim.exe` skips building; the caller must ensure freshness.
No user settings, files, tool installations or network services are required.

GUI without `-Interactive`, without Windows, or without an interactive session is
**not_executed**, with exit code 2, never passed. Failures exit 1; success exits 0.
The smoke program invokes application handlers; it does not send OS keyboard/IME
events and is not a substitute for manual interaction or visual inspection.

Reports and separate stdout/stderr logs are in
`target/test-reports/<UTC timestamp>-<mode>/`. JSON records each command, elapsed
time, exit/status, passed/failed/ignored test counts and names, seed, toolchain and
explicitly unexecuted coverage. Ignored tests remain ignored, not passes. A failure
stops subsequent stages; inspect the stage logs before rerunning its command.
Timeouts stop only the owned subprocess tree. Environment overrides are restored.
The driver peak RSS field can be null; it is not aggregate Cargo/rustc memory.

## Test boundaries

| Layer | Coverage | Limits |
|---|---|---|
| Existing core/adapter tests | Rope edits, modes, undo, positions, file safety, syntax, document isolation, completion, pairing | No actual OS window |
| `tests/properties.rs` | Independent String/grapheme oracle; generated edits, movement, replacement, undo/redo; separate modal invariants | Bounded exploration, not exhaustive fuzzing |
| `src/gui/integration_tests.rs` | Actual EditorView key/text dispatch, focus, persistent popup selection/acceptance, resize, shape-cache reuse, async save race | GPUI TestPlatform uses NoopTextSystem; no DirectWrite/GPU/pixels/IME |
| Existing mock LSP suites | Real framed JSON-RPC process, delays, out-of-order responses, cancellation, malformed/error/crash/shutdown, multiple providers and file switching | No guarantee of compatibility with every real server |
| Native smoke | Real compiled window and renderer, handler/geometry/state assertions | Requires desktop; no visual comparison or OS-input assertions |

GPUI 0.2.2's `test-support` is enabled only by `gpui-tests`. Tests use
`#[gpui::test]`, `TestAppContext`, `VisualTestContext`, `add_window_view`,
`simulate_keystrokes`, `simulate_input`, `run_until_parked`, and `simulate_resize`.
Two interaction tests run deterministic executor seeds 0 and 23. Save completion
is driven through the actual GPUI background executor. Test access stays inside a
test-only child module; no production public testing API is added.

The `test-tools` feature builds the tiny `filavim-mock-lsp` binary once through
Cargo. Standalone test commands without that feature retain a temporary rustc
fallback. Production builds enable neither feature. The mock does not need an
installed language server. Existing optional real-server/install tests stay
ignored; do not run `--ignored` globally without reviewing their side effects.

Property examples shrink with a 2-second/2048-step shrinking budget and persist
regressions in `tests/proptest-regressions.txt`. Reproduce a different seed with:

```powershell
pwsh -File scripts/test.ps1 -Mode Full -Cases 64 -Seed 4045029414
# Focus only the generated core sequences:
$env:FILAVIM_PROPTEST_SEED='4045029414'
$env:FILAVIM_PROPTEST_CASES='64'
cargo test --locked --no-default-features --test properties -- --nocapture
```

Keep persisted failures and turn confirmed bugs into readable deterministic tests.
The new GPUI acceptance test exposed a real regression: accepting a completion
called the change handler as though another character had been typed, immediately
reopening the popup. The targeted fix records the accepted document/revision/caret
identity; the next user edit still triggers normally. The test asserts both.

## Performance baselines

`tests/performance.rs` reports individual-sample p50/p95/p99/max in nanoseconds,
workload/sample counts and Windows process RSS before/loading/closing twelve
2 MiB documents. It measures buffer editing, 40-line viewport extraction batches,
warm middle-of-1 MiB-line horizontal movement, cursor simulation and repeated
document open/close. The existing completion-filter benchmark is run separately.
These are CPU/internal operations, **not text shaping, GPU presentation or
input-to-photon latency**. Closing RSS includes allocator retention; it is not a
leak assertion. Non-Windows RSS is null, not zero.

Archive JSON/logs before changing performance-sensitive code and compare identical
release workloads on the same machine/toolchain. No hard timing gate is set from
one noisy run. Existing `windows-native-perf.ps1` remains available for opt-in real
window startup, RSS and CPU sampling with caller-supplied File/Project fixtures;
it is separate from correctness and is not run by CI. Existing cursor/LSP smoke
scripts are retained. Actual DirectWrite shaping distributions and frame/display
latency remain future instrumentation work, not inferred from headless tests.

## CI and maintenance

GitHub Actions runs Full CoreOnly on Ubuntu/macOS and Full headless GPUI on Windows,
then a normal Windows release compile. Dependency caching and report artifacts
are enabled; superseded runs cancel. CI does not claim an interactive desktop.
The workflow is added locally; remote execution must be checked after publication.

No screenshot baselines are added: geometry/state assertions catch current risks
without DPI/font/animation-sensitive pixels. The pinned GPUI Windows backend has
no usable accessibility bridge for these controls, so `winapp ui`/UI Automation
would not provide meaningful selectors; no fragile coordinate automation is added.
Nextest is not installed and has no measured advantage for this small suite;
Cargo's runner avoids another tool dependency. Reconsider only with measurements.

Follow `AGENTS.md` for regression-first changes and exact coverage reporting.
The original terminal worktree and existing pending migration changes are preserved.

References: [GPUI 0.2.2](https://docs.rs/gpui/0.2.2/gpui/),
[Zed Vim test context](https://github.com/zed-industries/zed/blob/main/crates/vim/src/test/vim_test_context.rs),
[GPUI test context](https://github.com/zed-industries/zed/blob/main/crates/gpui/src/app/test_context.rs),
[Proptest](https://proptest-rs.github.io/proptest/),
[Nextest](https://nexte.st/docs/running/).

## Local validation record — 2026-10-26

Windows 10.0.26200, rustc 1.93.1, jobs/test threads 2:

- Fast: 73 passed, 6 ignored; formatting and warnings-denied Clippy passed.
- Full: 112 passed, 9 ignored, including four actual GPUI tests (two with two
  executor seeds) and two properties with 64 generated sequences each. No failures.
  Clippy took 25.90 s; correctness build plus execution took 52.24 s.
- Property regressions: none discovered in the bounded run. The completion popup
  regression was found by the deterministic GPUI interaction test and fixed.
- GUI without desktop acknowledgment correctly produced `not_executed`.
- Full CoreOnly on this Windows host: 92 passed, 8 ignored; formatting and Clippy
  passed. This validates the command/feature selection, not Linux/macOS behavior.
- Normal Windows release build passed (2 min 19 s); one native smoke passed in
  8.81 s. Input/undo, resize/caret geometry, picker, confirmation/information,
  project/save and cursor motion assertions passed. During its idle observation,
  editor and cursor frame counts stayed at 9 and `cursor_active=false`; no visual,
  pixel, OS keyboard or IME correctness claim follows from that observation.
- Performance: two explicit release tests passed. First compilation cost about
  96 s across two targets; warm workflow completed in 2.44 s. No thresholds imposed.

Warm performance run, individual samples (microseconds):

| Workload | Samples | p50 | p95 | p99 |
|---|---:|---:|---:|---:|
| Large-buffer insert/delete pair | 256 | 6.7 | 11.7 | 58.3 |
| Extract 40 visible line fragments | 128 | 56.1 | 80.2 | 111.0 |
| Warm horizontal move/fragment on 1 MiB line | 256 | 36.8 | 44.1 | 55.1 |
| Cursor simulation frame | 256 | 0.2 | 0.2 | 1.2 |
| Open/close 2 MiB document | 64 | 1246.5 | 1746.5 | 2060.5 |
| Filter 1,000 candidates, no edit journal | 100 | 477 | 546 | 606 |
| Filter 10,000 candidates, 64 journal edits | 100 | 2387 | 3090 | 3172 |

Test-process RSS was 7.63 MiB before, 35.16 MiB with twelve documents, and
7.31 MiB after closure. This is not native application/GPU memory and does not
prove long-session leak freedom. First/warm timing differences demonstrate why
these observations are baselines, not hard regression gates. No allocation-volume,
GPU/display timing or compiler-tree peak memory claim is made.

Reports used:
`target/test-reports/20261026-193435-412-fast`,
`20261026-193450-253-full`, `20261026-194151-595-performance`,
`20261026-194158-503-gui`, and `20261026-194432-532-full` (CoreOnly).
