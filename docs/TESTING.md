# Testing FilaVim

Language installer/protocol details and limitations are documented in
[LANGUAGE_INTELLIGENCE.md](LANGUAGE_INTELLIGENCE.md). The required-server acceptance
test is opt-in, downloads official executable tools, and uses temporary storage:

```powershell
cargo test --locked --no-default-features --lib required_catalog_install_and_completion_acceptance -- --ignored --nocapture --test-threads=1
cargo test --release --locked --no-default-features --lib language_intelligence_operation_timings -- --ignored --nocapture --test-threads=1
cargo test --release --locked --no-default-features --lib completion_filter_latency -- --ignored --nocapture --test-threads=1
```

The acceptance fixture checks real installation, default activation, completion
acceptance, revision-current diagnostics, supported definition navigation and
shutdown. It retries navigation during upstream indexing/content-modified replies.
It does not establish all real-server formatting, rename or refactoring behavior.
Local downloader fixtures use a test-only loopback policy; production only accepts
the curated HTTPS origins. No OS desktop is controlled by these tests.

Use standard Cargo tests and the existing GPUI TestPlatform. No custom test
framework, external language server, paid service, or desktop session is needed.
Run commands from the `filavim-gpui` checkout, not the terminal edition.

## Tools and commands

Install the Rust stable toolchain (currently validated with 1.93.1), rustfmt,
Clippy, and PowerShell **7.2+** (`pwsh`, including on Windows). Cargo dependencies
must be available before offline testing; tests themselves do not need a network.
GPUI is pinned to 0.2.2. Windows native compilation needs the SDK tools configured
by the existing `.cargo`/Windows build setup.

```powershell
pwsh -File scripts/test.ps1 -Mode Fast
pwsh -File scripts/test.ps1 -Mode Full
pwsh -File scripts/test.ps1 -Mode Full -CoreOnly
pwsh -File scripts/test.ps1 -Mode LSP
pwsh -File scripts/test.ps1 -Mode Performance
cargo build --locked
```

- **Fast:** report-parser checks, formatting, warnings-denied core Clippy, library,
  editor and real-filesystem save-safety tests. No native linking or large fixtures.
- **Full:** formatting/Clippy and all registered correctness targets, seeded
  properties, mock LSP processes and headless GPUI interaction tests. Includes the
  large escaped-JSON recovery regression. Two build jobs/two test threads by default.
- **Full -CoreOnly:** the same featureless correctness boundary with test-tools;
  excludes native GPUI. Existing Linux/macOS CI uses this profile.
- **LSP:** transport unit tests and existing document/protocol integration tests.
  Reuses the Cargo-built mock server; includes document tests that share its target.
- **Performance:** only explicitly selected ignored release measurements. Ordinary
  tests do not generate multi-MiB workloads or enforce noisy timing thresholds.

Full already runs headless native component tests. To focus during development:

```powershell
cargo test --locked --features gpui-tests --lib gui::view::integration_tests -- --test-threads=2
cargo test --locked --no-default-features --test file_safety -- --test-threads=2
cargo test --locked --no-default-features --lib recovery::tests -- --test-threads=2
```

The legacy GUI/desktop smoke scripts remain available for separately authorized
manual acceptance work. They are **not** part of this headless architecture and
were not executed during stabilization. Appearance, OS input/IME, DPI, native
rendering, accessibility and desktop interaction are **Not Verified** by these tests.
Do not substitute coordinate automation or screenshot comparisons for those limits.

## Where tests belong

`autotests = false` and explicit `[[test]]` entries in Cargo.toml are intentional.
Do not assume a new file under `tests/` runs automatically. Add cases to the
nearest existing target; add a new target only for a distinct build/feature boundary.
Nested support files are not standalone Cargo targets.

| Location | Responsibility |
|---|---|
| `src/*` unit tests | Private invariants, recovery commit/scan/locking and Windows file primitives |
| `tests/editor.rs` | Buffer/modal editing and keymap public behavior |
| `tests/file_safety.rs` | Real temporary files, save jobs and editing/save/conflict combinations |
| `tests/checkpoint2.rs`, `lsp_manager.rs` | Documents, mock protocol, services, workspace edits and ownership |
| `src/gui/integration_tests.rs` | Actual EditorView handlers on GPUI TestWindow; no desktop window |
| `tests/properties.rs` | Independent String/grapheme oracles and generated valid edits/replacements |
| `tests/performance.rs` | Ignored operation timings, resource observations, synthetic workloads |
| `tests/support/filesystem.rs` | Small owned temporary-workspace helper |
| `tests/support/mod.rs`, `fixtures/mock_lsp.rs` | Existing shared protocol executable fixture |
| `scripts/test-report.ps1`, `scripts/tests/test-report.ps1` | Pure Cargo summary parsing and its regression checks |

Do not move tests just to fit folders or create a generic fixture DSL. A local
explicit fixture is preferable until multiple tests genuinely share behavior.
No coverage-percentage gate, Criterion, Nextest, snapshot framework, or new test
executable was added: existing Cargo/test-support capabilities cover current needs.

## Adding a regression

1. Reproduce the failure in the smallest responsible layer. Assert exact text,
   cursor/selection, dirtiness, owner/revision and filesystem state where relevant.
2. Add the test alongside the fix, with a behavioral name and isolated state.
3. Run the focused command, Fast, then Full for input/document/async/native changes.
4. Preserve minimized property seeds. Report failures, ignored cases and limitations.

Files must live under an owned `tempfile::TempDir`. Never use a real user document,
recovery directory, config, tool installation, or repository for a test. Preserve
both disk and buffer contents in failure assertions; a returned error is not enough.
Permission tests use readonly attributes or Windows sharing handles, not global ACL
changes or administrator assumptions. Windows-specific guarantees are cfg-gated.
The runner redirects configuration and LSP storage into its report directory.
Direct unit fixtures disable LSP; native TestWindow fixtures disable persistent
recovery access. Recovery tests always pass an isolated storage root explicitly.

## Deterministic asynchronous tests

- GPUI tests use the pinned TestPlatform executor. `run_until_parked()` executes
  ready work. `cx.executor().advance_clock(Duration::from_millis(150))` makes the
  search debounce timer ready without a wall-clock sleep. Run ready work again.
- Cover the boundary just before/at debounce, multiple edits, generation changes
  with the same revision, document switching and cancellation. Seeds 0 and 23 vary
  executor ordering. Never rely on a large sleep to let a worker finish.
- Recovery tests use `flush()` acknowledgments to establish durable boundaries.
  Pure debounce wait calculations test the maximum delay. Interrupted writes use
  real temporary files. Child-process tests signal READY only after flush, then
  the parent terminates and waits for the owned child before reopening storage.
- File replacement tests exercise the real commit primitive at a controlled
  boundary. Windows tests also prove sharing denial against another process.
- Existing mock LSP scenarios deliver delayed/out-of-order/error/malformed replies.
  Keep those fixtures reusable rather than adding a second service implementation.

No additional crash executable is needed: the existing library test binary runs
one exact environment-controlled fixture case. With no fixture environment that
case returns normally. Parent timeouts are failure bounds, not synchronization
sleeps. The existing mock-LSP executable remains the sole purpose-built fixture.

## Properties and reproduction

The runner defaults to seed `4045029414`, 64 cases/property, two threads. Tests use
Proptest with bounded shrinking and persist minimized cases in
`tests/proptest-regressions.txt`. The oracles use independent String/grapheme logic.

```powershell
pwsh -File scripts/test.ps1 -Mode Full -Seed 4045029414 -Cases 128 -Threads 1
$env:FILAVIM_PROPTEST_SEED='4045029414'
$env:FILAVIM_PROPTEST_CASES='128'
cargo test --locked --no-default-features --test properties -- --nocapture --test-threads=1
```

Keep meaningful seeds and add a readable deterministic case for a demonstrated
bug. Generated literal replacements compare with `String::replace`; zero-width
regex replacement compares with independent scalar interleaving. Undo/redo and
independent buffer contents must remain exact.

## Reports, failures and optional checks

Each run writes `target/test-reports/<UTC timestamp>-<profile>/report.json`, plus
per-stage stdout/stderr. Reports include exact commands, exit status, pass/fail/
ignored counts and reasons, seed, toolchain, elapsed time and unexecuted coverage.
Formatting and compilation failures have failed stages, not invented test cases.
Zero selected tests or ignored-only required commands fail. Parser checks cover
multiple targets, compilation text, failure counts and empty selection.

A failed stage stops subsequent stages with nonzero exit status; a timeout stops
only the owned subprocess tree. Environment overrides are restored. Driver RSS is
not aggregate compiler memory. A zero-test binary target inside Full is normal;
the required Cargo command must execute nonignored tests overall.

Inspect the first failing stage log before retrying. Re-run its exact command;
for a race use the same seed and one test thread. Do not remove tests, loosen
assertions, or retry repeatedly until green. Optional installed-server, installer
and performance cases stay ignored in Full; never use global `--ignored` because
some optional cases download/run software. No network/installed server is required
by the default suite.

## Measurements and CI

Performance uses `Instant` and black_box, emits structured FILAVIM_METRICS records,
and records workload bytes, samples, p50/p95/p99/max, optimized status and limits.
Datasets include ~4 KiB, 128 KiB, 4 MiB, Unicode/CRLF content and a 1 MiB Unicode
line. Recovery flush bypasses debounce to measure actual serialization/write/load.
Seven heavy-operation samples are exploratory, not statistically strong tails;
near-clock-resolution undo/switch timings must not be interpreted as UI latency.

Use the same host/profile/settings and compare distributions for an optimization.
Warm filesystem caches, security scanning, allocator retention and first-use costs
can dominate individual samples. Do not infer input-to-display latency, GPU costs
or long-session leak freedom. See STABILIZATION.md for the recorded run.

The existing CI already runs core suites on Linux/macOS and headless Full on
Windows, followed by normal native compilation. No workflow, paid service,
external server, network test requirement or graphical session was added. Local
Windows success does not prove other platforms or remote CI execution.
