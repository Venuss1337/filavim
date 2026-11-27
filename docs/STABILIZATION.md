# Stabilization implementation and verification — 2026-11-16

Checkout: `C:/Users/filet/Desktop/filavim-gpui`, branch `gpui-migration`.
Existing pending migration changes were preserved. No desktop/Computer Use tests
or real-window smoke runs were performed during this task.

## A. Stabilization results

**Windows save protection (`src/files.rs`).** Final verification and replacement
hold a read handle that denies ordinary write access and shares delete access.
An existing incompatible writer fails safely instead of allowing a stale save.
The old tempfile commit API requested incompatible access; the implementation now
uses ReplaceFileW, closes the source handle first, and supplies a backup path.
The displaced version is checked before backup cleanup. A detected commit-boundary
change preserves that external version and returns an error; editor content remains
dirty even if its snapshot already reached the destination. Documented partial API
failures retain available backup/snapshot paths for inspection. This is not a
transactional rollback or a universal expected-identity compare-and-swap. Windows
sharing, backup retention and failure behavior are tested on real temporary files,
including a separate process. Non-Windows platforms retain the documented race.

**Recovery (`src/recovery.rs`).** Session and entry failures are isolated. Valid
snapshots are offered even when adjacent snapshots are malformed, interrupted,
locked, oversized, or unreadable. Damaged data moves to uniquely named quarantine
files; failed moves leave originals intact. Errors identify source/retained paths
and omit serde field values. The 16 MiB decoded limit remains explicit; older
snapshots survive an oversized write and the document-specific warning explains
that latest edits may be unprotected. Encoded JSON bounds now allow worst-case
six-byte escaping. Buffered serialization/readback fixes excessive small disk I/O.
The maximum debounce wait now accounts for elapsed time rather than overshooting
its two-second deadline by another full quiet interval. Flush acknowledgments and
existing real crash-process fixtures remain the synchronization boundary.

**Search (`src/gui/search_refresh.rs`, `search.rs`, `gui/io.rs`, `gui/view.rs`).**
The existing engine/jobs remain authoritative. A separate owned GPUI task uses
150ms debounce, cancellation generations and document/revision/query/options keys.
Search no longer takes the file busy flag. Automatic publication preserves cursor,
mode, selection and command input; native status reports live counts. Small
unaffected highlight sets project through the edit journal during debounce; sets
over 4096 ranges or 64 pending changes wait for a fresh scan. Navigation and edits
never trust provisional ranges. Superseded searches cancel between matches; a
single regex engine call cannot be interrupted. Replacement still validates the
snapshot and uses one undo operation. Service-driven text changes also reconcile
search through EditorView::changed. Single replacement stops capture enumeration
once the chosen match is replaced.

**Other demonstrated defects.** Python comment lines ending in a colon no longer
open a block. Frontend absolute-path handling no longer lexically erases parent
components across potential symlinks. Workers follow platform filesystem semantics;
Win32's acceptance of `missing/../file` differs from Unix and is explicitly tested.
Action variants, names, parsing and enumeration now derive from one small registry;
the availability match remains exhaustive and tests iterate the real registry.

## B. Testing architecture

Keep the existing explicit Cargo targets and standard test runner. No wholesale
folder migration, additional package, fixture DSL, or test framework was justified.
A small owned workspace helper joins the existing LSP helper under tests/support.
Properties extend the existing seeded runner and independent oracles. Private
Windows/recovery boundaries stay in module tests; production EditorView behavior
runs on GPUI TestWindow. scripts/test-report.ps1 isolates pure report parsing;
its script checks run in Fast/Full and cover failed/empty/ignored-only selections.
Fast now includes critical file-safety integration tests; LSP is a focused subset;
Full remains the authoritative headless correctness profile. Reports collect
structured metrics from every selected measurement stage.

## C. Dedicated executable decision

No new executable. Existing library-test entrypoints provide exact,
environment-controlled crash and Windows-sharing child scenarios. READY is sent
after durable flush; the parent kills/waits for its owned child before recovery.
The existing Cargo-built mock-LSP executable remains reusable. This avoids another
application, GUI or production implementation while exercising actual processes.

## D. Test inventory

- Buffer/modal/grapheme/history/property regressions remain intact.
- Actual filesystem tests cover new paths, aliases, canonical ownership, timestamp
  preservation, identical-content replacement, deletion, permissions, stale save
  jobs, newer edits and replace/undo/save/external-edit combinations.
- Windows commit tests cover separate-process sharing denial, displaced-version
  preservation and failed replacement retaining both original and save snapshot.
- Recovery covers multiple documents, untitled/file snapshots, successful-save and
  discard cleanup, session locking, subprocess termination, partial temporary files,
  corrupt entry isolation, quarantine failure, escaped JSON, oversize data and root
  errors. Large escaped-JSON regression is Full-only, not an ignored required test.
- GPUI seeds 0/23 exercise rapid edits, exact 149ms/150ms debounce boundaries,
  same-revision query supersession, clearing/cancellation, save/search concurrency,
  tab ownership, focus, command input, invalid patterns, replacement undo/redo and
  application-close cancellation with multiple dirty documents.
- Mixed comments, blank/CRLF lines, configuration override and Python behavior have
  focused text assertions. Registry tests derive actions from the production source.
- Existing transport, diagnostics, completion, workspace-edit and document ownership
  mocks remain in Full and the focused LSP profile.
- Four properties each run 64 cases using seed 4045029414; new oracles compare
  literal replacement against String::replace and zero-width output against scalar
  interleaving. No property failures were found in this bounded run.

## E. Actual results

Windows x64 10.0.26200; rustc 1.93.1; two build jobs/test threads unless the selected
benchmark explicitly runs serially. No installed external server or network test.

| Command | Passed | Failed | Ignored | Report directory |
|---|---:|---:|---:|---|
| `pwsh -File scripts/test.ps1 -Mode Fast` | 118 | 0 | 7 | `20261116-172715-481-fast` |
| `pwsh -File scripts/test.ps1 -Mode Full` | 171 | 0 | 11 | `20261116-172352-063-full` |
| `pwsh -File scripts/test.ps1 -Mode Full -CoreOnly` | 140 | 0 | 10 | `20261116-172547-605-full` |
| `pwsh -File scripts/test.ps1 -Mode LSP` | 17 | 0 | 1 | `20261116-171649-601-lsp` |
| `pwsh -File scripts/test.ps1 -Mode Performance` | 4 | 0 | 0 | `20261116-171125-566-performance` |

Report directories are under target/test-reports and contain exact underlying
Cargo commands/logs, ignored names and unexecuted coverage. Counts are standard
Rust test results; report-parser checks are separate stages, not invented Rust cases.
Full includes eight opt-in performance cases and three optional external-tool
cases as ignored; Performance deliberately selects four relevant measurements.
Formatting and warnings-denied Clippy passed in Fast/Full/CoreOnly. Normal
`cargo build --locked` passed (13.05 s). Changed tracked files pass scoped
`git diff --check`; the whole-tree check still fails on pre-existing whitespace
at examples/performance.rs lines 43–45, which was not changed.

Initial baseline reproduced 145 passed/0 failed/9 ignored. A new highlight projection
regression demonstrated an unnecessary dropped boundary match and was fixed. The
path fixture initially assumed Unix semantics on Windows; it was corrected to
assert preserved input components and exact native resolution, without dropping
those assertions. Both final suites pass. No skipped stage is counted as a pass.

## F. Performance findings

Optimized internal operation timings, Instant/black_box, warm local filesystem
workloads. Heavy operations have seven samples; p95 therefore equals the maximum
and is not a statistically strong tail estimate. Values below are p50 milliseconds.

| Operation | ~4 KiB | ~128 KiB | ~4 MiB | ~1 MiB Unicode/CRLF |
|---|---:|---:|---:|---:|
| Verified file opening | 0.168 | 0.349 | 8.573 | 2.298 |
| Hash/identity verification | 0.053 | 0.110 | 2.460 | 0.574 |
| Literal search | 0.035 | 0.601 | 13.790 | 3.537 |
| Regex search | 0.559 | 1.334 | 17.519 | 5.320 |
| Replace All computation | 0.039 | 0.956 | 21.397 | 6.378 |
| Twenty edits + one rescan | 0.055 | 0.613 | 13.470 | 3.725 |
| Recovery flush + load | 7.067 | 9.973 | 41.954 | 17.640 |

4 MiB opening p95 was 162.968 ms despite an 8.573 ms median: first-use/security/cache
outliers matter. Recovery p95 was 47.857 ms. Document switches and persistent-Rope
undo/redo were near timer resolution; no nanosecond-level UI claim is warranted.
The long Unicode line's warm fragment median was 0.0062 ms, with a first-use maximum
of 10.1663 ms. These findings favor simple debounced background scans over a complex
incremental regex engine for now.

A targeted same-host/debug comparison of three 16 KiB NUL-text snapshot write/load
cycles measured 539.016 ms before buffering and 52.305 ms afterward. This is one
exploratory paired run, not a statistical performance gate. The later optimized
buffered run measured 32.596 ms total and is not directly comparable to debug.
The 16 MiB recovery limit is retained: one worker already has string/Rope/JSON and
startup aggregate retention costs; increasing it without broader resource data is
not justified. No Criterion dependency or broad performance rewrite was added.

The local chart is target/test-reports/20261116-171125-566-performance/stabilization-chart.html.
No GPU/display, OS input or end-to-end perceived-latency measurements were made.

## G. Remaining risks / Not Verified

- No universal atomic compare-and-swap against arbitrary path mutation. Windows
  backup retention provides a recovery path, not transactional rollback; exceptional
  commit conflicts/partial failures may require manual file comparison. Non-Windows
  race behavior and the Unix symlink regression were not executed on this host.
- Recovery still caps decoded documents at 16 MiB. Quarantine requires manual
  inspection and startup aggregate memory scales with recoverable content. Old
  unsaved data is not deleted merely due to age. Physical disk-full/power-loss,
  unusual/network filesystems and privileged writers were not exhaustively tested.
- Cancellation is cooperative between regex matches. Large highlight sets can be
  temporarily hidden during debounce. Seven timing samples do not establish SLAs.
- Desktop appearance, real OS keyboard/IME, DPI/GPU/accessibility behavior, external
  language-server compatibility and remote CI execution are Not Verified. No
  Computer Use, coordinate input, screenshot application tests or desktop smoke
  scripts were run. HTML chart rendering is only presentation of measured data.

## H. Contributor experience

Start with docs/TESTING.md: Fast for active edits, Full before document/async/native
changes, LSP for protocol focus and Performance only when measuring. Reuse isolated
fixtures and virtual-clock/acknowledgment boundaries. Preserve minimized seeds;
add readable deterministic regressions for discovered defects. Existing CI needs
no new service, graphical session, workflow or external language server.

## I. Future milestone readiness

The foundation is suitable for incremental language-intelligence and project-workflow
work with headless regression gates and explicit ownership/cancellation contracts.
This is not an unrestricted production-reliability certification. Preserve the
Windows/portable filesystem distinctions, bounded recovery work and UI-vs-operation
measurement boundaries. Advanced Vim, splits, terminals and other roadmap features
remain out of scope and were not implemented.

## J. Change summary

Production: src/files.rs, recovery.rs, search.rs, paths.rs, editor.rs, editing.rs,
input.rs, gui/io.rs, gui/view.rs and the new gui/search_refresh.rs child module.
Tests: gui/integration_tests.rs, tests/file_safety.rs, tests/properties.rs,
tests/performance.rs and new tests/support/filesystem.rs.
Runner: scripts/test.ps1, new scripts/test-report.ps1 and scripts/tests/test-report.ps1.
Docs: docs/TESTING.md, docs/EDITING_AND_SAFETY.md, docs/ARCHITECTURE.md,
docs/STABILIZATION.md; README/AGENTS links; root TESTING.md retained as historical
migration notes. No new dependency, test target, executable or CI workflow.
