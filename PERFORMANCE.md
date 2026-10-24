# Responsiveness work

The original pass is retained below as historical evidence. See
[Second runtime pass](#second-runtime-pass) for the current implementation,
individual-sample benchmarks, tradeoffs, and remaining limitations.

## Reproduce without an interactive terminal

```text
cargo run --release --example performance
cargo test --release --lib -- --include-ignored --nocapture
cargo test --all-targets
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
```

The example uses Ratatui TestBackend, in-memory documents, owned completion
responses, and a VT100 parser. It does not start Filavim, a PTY, or filesystem
workers. Its allocator counts allocation/reallocation calls and cumulative
requested bytes, not retained/peak memory. Fixtures, warmup, and completion
response fixture cloning are excluded from the counted interval. Timings are
arithmetic means, not latency percentiles or terminal presentation measurements.

The library tests inject events directly into the same runtime loop used by the
real input source, with TestBackend instead of a live terminal. The sustained
input test sends 240 events at 120 events/s and reports the final queue-drain tail.
It does not establish a general latency guarantee under every workload.

## Changes and safety boundaries

- Input processes only already-queued events, at most 32 or 2 ms per batch.
  Empty/no-op navigation does not rebuild the UI. Background drains have limits;
  budget exhaustion schedules another wake rather than stranding unread results.
- A 256-edit / 1 MiB journal records pre-edit UTF-8/UTF-16/UTF-32 coordinates and
  Tree-sitter edits. Incremental LSP updates send these changes in order. Undo,
  reload, or journal expiry falls back safely to full synchronization.
- Unsent document changes can coalesce, but never across document-open or
  request/notification barriers. Pipe writes do not hold the shared pending-slot
  mutex. Full-sync servers still receive complete text.
- Automatic completion has a separate quiet period capped at 60 ms. Candidate
  selection runs on a latest-job worker, selects the best 200 without sorting
  every item, and formats only retained items. Filter/width results are cached.
- Cached completions accept immediately only when recorded identifier edits
  prove their replacement remains valid. Additional edits, server data, commands,
  expired history, and uncertain ranges retain the fresh-response path. Snippet
  expansion and current-version additional edits retain existing validation.
- Rendering traverses rope graphemes up to the viewport boundary without copying
  whole lines. Contiguous rope slices use ordinary Unicode segmentation; boundary
  clusters use context-aware segmentation. Cursor columns are revision/cursor/tab
  keyed. Diagnostic lookup uses sorted line ranges; explorer Git lookup uses
  normalized keys.
- Parsing applies recorded edits instead of discovering them by whole-document
  comparison. Unaffected highlight spans remain visible while new results arrive.
- Terminal publications contain visible cells and input modes, shared with the UI
  through immutable Arc snapshots. Scrollback stays on the worker. Hidden output
  does not dirty the editor; a separate control queue takes priority over output.
- Git refreshes memoize unchanged baseline/current diffs with an 8 MiB retention
  limit. Ordinary Insert-mode Backspace avoids copying an unchanged register.

Cursor spring physics, rasterization, timing, Normal-mode restrictions, Ratatui
cell diffing, output buffering, theme, and keybindings remain intact. Existing
headless cursor/frame equivalence tests remain part of verification.

## Evidence limits and remaining work

Baseline release measurements taken before the relevant changes:

| Workload | Mean time | Allocations | Requested bytes |
|---|---:|---:|---:|
| 120×40 frame, start of 1,000,000-character line | 839.93 µs | 169 | 3,031,416 |
| Unbound Insert-mode key lookup | 1.68 µs | 27 | 195 |
| 10,000-item completion processing | 18,789.94 µs | 189,998 | 27,379,970 |

Final release results below are medians of three runs of the same example;
each run reports a workload mean. Baselines above were recorded once before
the corresponding optimization. System load/timer variance is not controlled.

| Workload | Final mean (median run) | Allocations | Requested bytes |
|---|---:|---:|---:|
| Same long-line frame | 135.65 µs | 88 | 16,050 |
| Same key lookup | 0.40 µs | 5 | 19 |
| Same completion processing | 6,379.94 µs | 21,614 | 1,186,624 |
| Legacy terminal clone, 24×120 with 2,000 history rows | 237.80 µs | 2,026 | 2,140,928 |
| Visible-cell snapshot of that same terminal | 45.02 µs | 1 | 92,160 |

The final release library benchmark measured full-sync message generation at
1,000,175 bytes / 1,985.81 µs versus incremental generation at 255 bytes / 3.72 µs
for a one-character (`é`) insertion in a 1 MB document. This excludes server
processing and terminal display. The queued-input test processed 1,003 ordered
events in 33 frames / 26.09 ms. Sustained input at 120 events/s preserved all 240
events, with a 504 µs final drain tail. These are headless test observations.

Verification passed: 110 regular tests (`cargo test --all-targets`; two benchmark
tests excluded there), all 33 release library tests including those benchmarks,
release binary/example build, formatting, and Clippy with warnings denied.
Cached-completion acceptance is checked synchronously with no additional request;
idle wakes and boundary navigation produce no extra frames; hidden terminal
publications do not dirty the editor. No interactive/manual verification ran.

The terminal example retains the old full-screen clone as a same-process
comparison against viewport capture. The sync benchmark compares full and
incremental wire generation with the same 1 MB document and edit. Full-sync
generation uses the current implementation; it is a payload/control comparison,
not an untouched historical build.

Dirty frames still reconstruct visible panes. Very long horizontal prefixes,
full highlight collection, large individual diagnostic messages, periodic Git
subprocesses, serial filesystem work, and actual host-terminal throughput remain
profiling targets. A worker time budget cannot preempt one expensive event.
Real-server turnaround and key-to-photon latency were not measured.

## Manual verification (not executed)

1. Hold WASD, change direction, and alternate Normal/Insert/Visual modes; check
   smear appearance, convergence, native cursor restoration, splits, and scrolling.
2. Type quickly, then accept completion after another letter or Backspace. Check
   snippets, Unicode names, additional imports, resolution, and undo/redo.
3. Open long lines and several splits with diagnostics/explorer visible. Check
   syntax refresh, tabs, combining marks, emoji, horizontal scrolling, and resize.
4. Generate terminal output, fill/scroll history, hide/show the panel, and switch
   sessions. Check input, colors, wide characters, modes, resize, and cleanup.
5. Exercise search/grep and Git while editing; check that results remain current
   and the Rosé Pine Moon UI and keybindings look and behave as expected.


## Second runtime pass

Implemented on 2026-10-24 following the principal runtime audit. All validation
is noninteractive. No host-terminal appearance or key-to-photon result is claimed.
No libraries were replaced and no dependencies were added or upgraded.

### Changes and invariants

- **Syntax critical path:** retain the last parsed highlight snapshot and project
  only spans relevant to visible pane regions through the ordered edit journal.
  Reverse mapping splits at replacements, so deleting a large region does not
  first copy all its old highlights. Enclosing tokens remain eligible. The old
  whole-vector transform remains the correctness reference, not the runtime
  document-wide update path. Journal expiry/undo safely suppress stale spans
  until a current parse arrives.
- **Syntax lifecycle:** requests have unique generations in addition to document
  identity and revision. Renames and syntax configuration changes also invalidate
  scheduling. Completed old generations cannot retire newer requests. Hidden
  edited/replaced/closed documents retire obsolete pending work; parser progress
  callbacks and highlight collection cooperate with cancellation. Cancelled parses
  reset Tree-sitter before another input. Syntax results wake the loop directly;
  syntax pending state no longer selects a periodic 16 ms timeout. Exhausted
  result drains schedule another wake.
- **Long lines:** eight recently used long-line display indexes contain sparse
  grapheme-boundary character/display-column checkpoints. They are built lazily,
  reused for columns, vertical movement and horizontal viewport starts, and
  invalidated from before an edit's boundary. Later lines are invalidated;
  earlier lines/prefixes remain usable. Undo clears the index. Short lines bypass
  it. This costs roughly 16 KiB per million indexed scalar values per line,
  plus vector capacity, and does not index whole documents eagerly.
- **LSP preparation:** one lazy keyed worker prepares diagnostics, locations and
  code actions. Different servers/documents do not cancel each other. Pending
  and completed results coalesce per key. Running work checks generations;
  publication validates server epoch, request ID, document, revision and context.
  Replaced/stale payloads and large panels are destroyed on the preparation
  worker. Transport order and document/request barriers remain intact.
- **Fairness/rendering:** server drain starting order rotates under the existing
  cooperative budget. Ignored notifications and diagnostic preparation do not
  themselves force a frame. Empty panel filters avoid lowercasing every label.
  Current-line diagnostics use sorted line ranges and their status preview is
  bounded. Newline indentation reads only the leading whitespace from the Rope.
  Full-pane reconstruction, Ratatui cell diffing and cursor scene reuse remain.
- **Transport:** queues expose current/peak depth, approximate payload volume,
  oldest queued age and worst consumed age. Empty LSP supervisors wait for actual
  input/deadlines rather than waking every 50 ms. One lazy shared write watchdog
  kills a server when an individual pipe write exceeds its configured timeout.
  It uses a condition variable and has no idle timer loop. Producers never wait
  for queue capacity. Nonreplaceable protocol queues remain unbounded; metrics
  are observability, not a claim of a global memory/backlog bound.
- **Terminal:** hidden/inactive sessions keep parsing but suppress repeated cell
  snapshots. Atomic application-cursor/bracketed-paste metadata remains current.
  Showing a session wakes it and publishes a fresh viewport. Final EOF publication
  and child-exit monitoring remain, including the ConPTY cleanup path.
- **I/O/lifetimes:** already-queued opens/searches/history coalesce in bounded
  batches, without crossing saves, mutations or project transitions. Skipped
  history entries still complete accounting. Running I/O is not preempted.
  Dismissed completion snapshots and closed-buffer change metadata are released.
- **Git:** human-readable subprocess capture keeps its 512 KiB limit and marks
  truncation explicitly. Machine data uses strict complete-or-error capture with
  separate stdout/stderr. Status is capped at 32 MiB; other Git data at 8 MiB.
  Oversized/unavailable baselines produce an explicit unavailable line-indicator
  state, never a truncated/empty baseline masquerading as valid content. The
  current-buffer diff input also has an 8 MiB limit. Errors remain visible and
  ordinary untracked files still use an empty baseline.

Smear spring/rasterization, Normal-only behavior, theme, keybindings, Rope undo,
transactional edits, atomic saves, incremental LSP barriers and native terminal
output buffering are preserved. No broad rendering invalidation framework,
interval tree, per-response worker pool or filesystem parallelism was added.

### Reproduce current checks and measurements

```text
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets -- --test-threads=1
cargo test --release --all-features --all-targets -- --include-ignored --nocapture --test-threads=1
cargo build --release --bin filavim --example long_lines --example performance
cargo run --release --example long_lines
cargo run --release --example performance
cargo test --release --features perf --lib highlight_projection_latency -- --ignored --nocapture --test-threads=1
cargo test --release --features perf --lib mixed_load_individual_latency -- --ignored --nocapture --test-threads=1
cargo test --release --features perf --lib stale_hidden_syntax_does_not_periodically_poll -- --ignored --nocapture --test-threads=1
```

Build first, then run timed workloads without concurrent builds. The mock-server
fixture is compiled by tests and every child is automatically terminated. The
optional `perf` recorder is process-global: run its measurements serially.
One debug timeout test failed while concurrent release LTO builds saturated the
host; the serial debug suite subsequently passed. This is not evidence of a
latency guarantee under arbitrary machine load.

### Instrumentation and methodology

`--features perf` enables monotonic enqueue/dispatch/action/frame samples, background
poll and frame-construction/diff-output distributions, JSON parse/preparation
samples, edit-applied timestamps, completion preparation/publication, and raw LSP
lifecycle timestamps. `perf::samples` and `perf::traces`
allow offline analysis. The recorder caps each stage at 100,000 samples and is
absent from default builds. Queue metrics remain available independently.

Frame diff/output includes Ratatui diffing and backend completion. TestBackend
completion is not a host-terminal flush/display measurement. LSP traces identify
requests/documents where known; they do not establish real-server compute time
or automatically pair every edit with a response/frame. Full historical mixed-load
p95/p99 baselines are unavailable: the earlier runtime harness recorded only a
final drain tail. No equivalent before/after claim is made for that workload.

Long-line fixtures contain approximately 1 MB of ASCII, tabs, combining sequences
or emoji. They measure first/cold frames separately, then individual navigation
and local-edit frames at the start, middle and end, early-prefix invalidation,
and four split views. Navigation has 100 samples per case; edit cases have 40,
early-prefix cases 20. Small-sample p99 is descriptive, not a stable production
SLO. The original before-run covers navigation/local edits, not the subsequently
added four-split and early-prefix cases.

The syntax comparison applies the same 32 edits to 100,000 artificial disjoint
spans, comparing full adjustment with a 400-byte viewport projection. Reference
fixture cloning is outside the timed interval. It measures highlight maintenance,
not parsing, complete rendering or terminal output.

Mixed load uses two real mock LSP subprocesses, each publishing 24 responses with
20,000 diagnostics over approximately 1.2 seconds, a syntax-enabled Rust buffer,
another Python buffer, finite PTY output, and 240 characters at 120 events/s.
Mode/quit keys are also sampled (246 total input events). Assertions verify
preserved input and successful preparation. Functional tests additionally cover
20,000 reference/code-action items, stale-response rejection, blocked pipe writes,
Git outputs above 512 KiB, hidden-session publication/modes, generation retirement,
Unicode checkpoint equivalence, cancellation and durable I/O barriers.

### Results

Measurements use the same Windows 11 / i5-12400F / Rust 1.93.1 GNU host as the
audit. Quantiles are calculated from individual samples using nearest rank.
Repeated-run ranges describe the run quantiles, not pooled p99 values.
Allocation counts in the original example remain unchanged: 88/16,050 requested
bytes per long-line-start frame, 5/19 per key lookup, 21,614/1,186,624 per completion
fixture, and 1/92,160 per terminal viewport snapshot. These are allocation-call
and requested-byte counters, not retained/peak heap memory.

| Workload | Before p95 / p99 | After p95 range / p99 range (three runs) |
|---|---:|---:|
| ASCII end navigation + frame | 81.217 / 91.475 ms | 0.126–0.198 / 0.149–0.263 ms |
| Tabbed end navigation + frame | 70.164 / 72.907 ms | 0.113–0.190 / 0.163–0.236 ms |
| Combining end navigation + frame | 50.204 / 53.365 ms | 0.122–0.197 / 0.187–0.233 ms |
| Emoji end navigation + frame | 33.402 / 37.644 ms | 0.159–0.223 / 0.202–0.271 ms |
| 100k highlights / 32 edits, full reference vs projection | 5.199–5.231 / 5.735–5.959 ms | 0.029–0.034 / 0.034–0.040 ms |

The long-line before values are one 100-sample baseline run taken before the
checkpoint implementation. The syntax before column is the retained reference
algorithm measured in the same process as projection, in three runs. These are
operation-specific comparisons, not an end-to-end application speedup.

| Mixed-load stage | p50 range | p95 range | p99 range |
|---|---:|---:|---:|
| Input queue wait | 0.012–0.013 ms | 0.822–0.967 ms | 1.274–1.759 ms |
| Input enqueue → action complete | 0.045–0.047 ms | 0.847–0.998 ms | 1.302–1.780 ms |
| Input enqueue → headless frame complete | 1.205–1.309 ms | 2.843–2.866 ms | 3.816–3.916 ms |
| Frame construction + diff/output | 0.243–0.254 ms | 0.439–0.474 ms | 0.486–0.657 ms |

Each mixed run has 246 individual input samples. Peak input depth was six,
including the deliberately queued mode/quit-key burst. There is no historical
individual-sample mixed-load baseline to compare against. LSP preparation/JSON
parsing itself can still take tens of milliseconds on workers.

Four-split navigation/frame p99 ranges were 0.305–0.488 ms (ASCII),
0.281–0.358 ms (tabs), 0.391–0.485 ms (combining), and 0.610–0.676 ms (emoji).
No pre-change four-split baseline was recorded. Early-prefix invalidation while
viewing the end of an ASCII line still measured 28.22–33.75 ms p99; the sparse
cache deliberately rebuilds invalidated content instead of guessing widths.
First access also retains a linear prefix cost.

The stale/hidden syntax regression observed two main-loop background polls over
250 ms, rather than recurring 16 ms polls. This is a wake-count observation, not
a process-CPU measurement. Hidden PTY tests verify periodic publication stops
while parsing continues and showing the session refreshes its cells. Mode metadata
is also tested directly, because ConPTY may consume DEC mode-control sequences.

Validation: 124 regular debug tests passed (three benchmarks ignored); 129 release
tests passed with all features/ignored benchmarks enabled; release binary/examples,
Clippy with warnings denied, formatting, and CLI version checks passed. The
reference-result test also confirms navigation without editing does not invalidate
an otherwise current references request.

Raw local run logs: `target/long-lines-before.txt`, `target/long-lines-final.txt`,
`target/mixed-final.txt`, `target/highlights-final.txt`, `target/micro-after.txt`,
`target/debug-final.txt`, and `target/release-final.txt`. Target artifacts are
reproducible outputs, not required source fixtures.

### Remaining limits and manual validation

Cold access or an edit before most indexed content still has to rebuild the
required long prefix. More than eight simultaneously used long lines can evict
indexes. Active syntax projection scales with visible regions and retained edits;
full parsing/highlight collection still consumes worker CPU. Nonreplaceable LSP
queues, already-running I/O, serialized IDE setup/edit-planning jobs, PTY writes,
large nonempty panel filters, oversized hover/resolve documentation and actual
terminal backpressure remain profiling targets. No global p99 guarantee, process
idle-CPU percentage, peak-memory improvement or Linux/macOS performance result
is established by these tests.

Manual verification (not performed):

1. Hold WASD and change direction/mode; inspect smear convergence, native cursor
   restoration, scrolling and splits.
2. Navigate deep into long Unicode/tabbed lines; edit locally and at the beginning,
   then undo/redo and resize.
3. Exercise completion acceptance, snippets, resolve/additional edits, references,
   diagnostics, formatting and server restart while typing.
4. Hide/show busy terminals and switch sessions; verify fresh cells, application
   cursor keys, bracketed paste, resizing and cleanup.
5. Inspect Git indicators on files above 512 KiB and the explicit unavailable state
   above the machine-data limits; exercise saves, project switching,
   explorer/search and exit while background work is active.
