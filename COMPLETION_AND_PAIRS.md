# Instant completion and intelligent pairs

This report records the completion and auto-pair implementation checkpoint.

## Reliability correction (2026-10-25)

Confirmed additional suppression paths, distinct from the original debounce fixes:

- A complete **empty** LSP list was treated as reusable for every longer query.
  No later request was sent in that session. Complete cached lists that no longer
  produce usable visible candidates could similarly suppress a useful refresh.
- Typing behind a pending request was coalesced correctly, but an error from that
  older request removed it without scheduling the newer input state.
- The transport acknowledged dynamic completion registration/unregistration without
  delivering it to `Services`. The existing capability handler was unreachable.
  After registration, an existing prefix also needed reevaluation without another key.
- Automatic completion depended on `lsp.enabled`; there was no independent local
  TypeScript keyword provider. Type contexts depended entirely on the server.
- Save As/path replacement could retire the session while leaving its old popup
  visible when document/revision/caret identity happened to remain unchanged.

The fixes retain one in-flight automatic completion per provider/session and the
existing popup identity, filtering and range projection. Empty or unusable caches
can refresh on **new input**, not repeatedly at the same revision. An older failed
request dispatches the coalesced newer revision once. Manual Ctrl+Space can supersede
a pending request; cancelled/out-of-order replies cannot overwrite the new request.
Dynamic registrations now reach the capability handler before acknowledgment.

`gui/completion_context.rs` supplies a small TypeScript/TSX type-keyword provider:
`string`, `number`, `boolean`, `bigint`, `symbol`, `object`, `unknown`, `never`,
`undefined`, `null`, `void`, `any`. It recognizes common aliases, interface/class
properties, variable/parameter/return annotations, unions and generic type positions.
It excludes ordinary object values, member access, imports, comments, strings and
other languages. It works with LSP disabled and at a colon/whitespace type position
with default prefix settings. Higher configured minimum-prefix values remain honored.

Lexical inspection is bounded to 64 preceding lines / 8 KiB and cached once per
identifier session. It is deliberately a fallback, not a TypeScript parser: complex
types and lexical state originating outside that window can remain ambiguous, and an
oversized context yields no local keywords. LSP remains the source for project symbols,
members, imports and richer language semantics. No global keyword list is injected
into JavaScript or unrelated contexts.

Local candidates appear synchronously, before LSP readiness or a response. Provider
lists merge into the same session. Equivalent valid LSP labels replace local labels
after range validation, preserving the selected label. Empty, filtered-out or unusable
LSP items cannot hide safe local candidates. No GUI component, theme, pairing behavior
or settings format was replaced. There are no new dependencies, timers or threads.

Validation:

- Formatting and `cargo clippy --locked --all-targets --features perf -- -D warnings` passed.
- `cargo test --locked --lib --test editor --test native --test floating --test checkpoint2 --test lsp_manager`:
  **104 passed, eight ignored** (optional external-server checks and benchmarks).
- Targeted completion integration rerun additionally covers an old nonempty list
  filtered to zero results, alongside delayed empty/error and out-of-order replies.
  Each scenario asserts exactly two requests and no retry loop.
- Tests cover type aliases/interfaces, primitive types, colons/newlines/whitespace,
  parameters/returns/classes/generics, exclusions, incremental typing/backspace,
  popup identity, Unicode acceptance, deduplication, stale ranges, buffer/path changes,
  dynamic readiness and TS/JS request-to-popup contexts. Existing two-provider tests
  verify progressive publication without waiting for the slow server.
- An opt-in **real vtsls 0.3.0** headless test supplied and accepted all three primitive
  types from actual server responses in isolated temporary TypeScript files. No
  packages were installed and no user preferences or project dependencies changed.
  Ordinary tests do not require vtsls; the opt-in test uses `FILAVIM_TEST_VTSLS` and
  `cargo test --locked --lib real_vtsls_type_keywords_and_acceptance -- --ignored --nocapture`.
- Windows release build passed. No live GUI was opened for this correction. Popup
  flicker and input-to-display latency still require manual observation; controlled
  mock delays and passing headless tests are not display-latency measurements.

## Confirmed original causes

- `EditorView::changed` restarted an owned GPUI task after **every** document/caret change and waited `lsp.debounce_ms` (120 ms by default). Continuous typing continually cancelled that timer.
- `Services::update` cancelled completion requests and removed the panel on **any** revision/caret change. Even replies that were still useful for a longer prefix were discarded.
- Each reply built a panel with the request ID and selected row zero. The view treated that as a new popup, resetting scrolling and documentation state.
- Response preparation retained only 200 already-filtered items and discarded the rest. There was no usable session candidate cache.
- All service activity incremented the syntax/layout epoch. Completion and documentation replies therefore invalidated otherwise unchanged viewport text shapes.

## Completion lifecycle

`KeyDown -> native text commit -> existing Editor/Buffer -> didChange -> immediate local session filtering -> asynchronous completion refresh -> worker preparation -> same popup session -> validated atomic acceptance`.

`gui/completion.rs` owns a bounded context: session/document/mode identity, anchor, revision, caret, query, lifecycle, and provider candidate snapshots. `Services::pending` owns request IDs, providers, source snapshots/revisions, and resolve identities. The native panel owns filtered rows and selection. Its request identity is the **session** identity, independent of in-flight requests.

A compatible edit changes only the current identifier prefix; it cannot move into another scope, line, buffer, or mode. Candidate identity preserves selection across filtering and provider refreshes. Backspacing to a query broader than the original server query refreshes the list. Expired edit journals trigger a refresh. No candidates are reused across documents or Save As path changes.

Each provider has at most one outstanding completion request per session. A valid slow request survives continued typing and can serve a narrower prefix; incomplete responses refresh the latest revision afterward. Providers publish progressively. Incompatible requests are cancelled with the existing transport. There is no typing debounce or periodic completion timer.

Preparation retains up to 20,000 items per provider, applies supported CompletionList item defaults, and precomputes matching keys off the UI thread. Prefix, case-insensitive, CamelCase, substring and subsequence matching select at most 200 presentation rows. The GPUI uniform list renders visible rows only. Existing element IDs and scroll handles remain stable; the completion menu reserves an eight-row height to avoid resizing with every keystroke. Empty matching results hide the panel, while its context can still accept a valid pending reply.

Original response snapshots and the buffer edit journal project UTF-8/16/32 text edits into the current compatible revision. Additional edits overlapping later changes are rejected. Resolve requests use the original provider item/snapshot, and results require the current document/revision/selection identity before application. Completion acceptance validates all edits before changing the Rope and groups the edits for undo. Snippets use the existing restricted expansion and place the caret at the first field; unsupported syntax is rejected instead of inserted literally.

Only syntax results invalidate the syntax-layout epoch. Popup-only updates reuse viewport shapes. Large superseded candidate snapshots are retired on the existing preparation worker.

## Pairing

`pairs.rs` supplies per-document, bounded provenance (256 owned pairs), language rules and bounded lexical context. `Editor::type_character` is the intentional-character entry point; the native adapter consumes a matching actual key event. Paste, IME composition/commits, platform replacements, snippets and workspace edits use ordinary buffer edits instead.

Supported behavior:

- Brackets, language-appropriate quotes and backticks, nested pairs, owned-closer overtype, and deletion of untouched empty pairs.
- Paired newline expansion using current indentation, tab/space settings and line endings.
- Unicode/reverse characterwise Visual selection surrounding in one undo transaction.
- Existing edit journals move or invalidate provenance; undo/replacement retires uncertain provenance rather than treating manual closers as owned.
- Current revision syntax spans suppress pairing in comments/strings. Otherwise conservative local lexical scanning is bounded to 32 preceding lines / 8 KiB; no synchronous parsing occurs.
- Completion insertion bypasses pairing. Matching empty owned delimiters are reused; a snippet caret inside an empty pair adopts its closer. Signature requests respond to the **typed** trigger, not the generated companion character.

## Persistent settings

Use the existing `:set NAME=VALUE` command, or the existing GPUI config (`FILAVIM_GPUI_CONFIG`, normally `%APPDATA%/filavim-gpui/config.toml`). Defaults:

```toml
[completion]
enabled = true
auto_trigger = true
minimum_prefix = 1
matching = "fuzzy" # prefix | substring | fuzzy (case-insensitive)
accept_enter = true
accept_tab = true

[pairs]
enabled = true
surround = true
in_comments = false
in_strings = false
smart_delete = true
overtype = true

# Optional extension-specific two-character pairs; [] disables an extension.
# [pairs.languages]
# js = ["()", "[]", "{}", "\"\"", "''", "``"]
# txt = []
```

`completion.enabled=false` or `auto_trigger=false` suppress automatic requests; Ctrl+Space remains available. Plain text and untitled files do not pair unless an extension-specific rule applies. Default Rust apostrophe pairing is conservative because apostrophes also introduce lifetimes/labels.

## Measurements

Windows, release, headless, 100 individual samples per case; p50/p95/p99 include local matching, top-200 presentation, explicit-range projection and allocations. They exclude LSP I/O, GPUI layout/GPU work and display latency. These are single short runs and contain normal host noise.

| Candidates | Intervening edits | Before shared journal projection p50/p95/p99 (ms) | Final shared-journal + unchanged-snapshot fast path p50/p95/p99 (ms) |
|---|---:|---:|---:|
| 1,000 | 0 | 0.452 / 0.599 / 0.710 | 0.511 / 0.806 / 1.014 |
| 10,000 | 0 | 1.816 / 2.576 / 2.721 | 2.114 / 2.887 / 3.124 |
| 1,000 | 64 | 0.970 / 1.677 / 1.708 | 0.655 / 1.197 / 1.598 |
| 10,000 | 64 | 2.384 / 3.443 / 3.494 | 2.357 / 2.865 / 3.126 |

The shared-journal change avoids cloning the same edit journal once per visible candidate. Edited cases improved in these runs; unchanged cases varied upward, so this is not evidence of a universal speedup. All measured release cases met the provisional sub-5-ms local-filter target. An earlier debug run, before restoring the unchanged-snapshot fast path, had p99 4.631 / 12.600 ms (1k/10k, no intervening edits), and 3.554 / 9.379 ms with 64 edits.

The original **120 ms reset-on-type delay is source-confirmed**, not a measured input-to-display baseline. It is removed; no end-to-end before/after visual latency or physical popup-flicker measurement is claimed. Allocation counts, GPU timings and idle CPU were not profiled in this task.

Reproduce the short benchmark:

```powershell
cargo test --locked --release --no-default-features --lib completion_filter_latency -- --ignored --nocapture
```

Optional `--features perf` records monotonic individual samples with p50/p95/p99, without timestamp/recorder overhead in ordinary builds:

- `completion.first_available`: eligible key callback to first prepared candidate publication (automatic typing); excludes display.
- `completion.filter`: local filtering and row/range preparation.
- `completion.lsp`: request scheduling to parsed reply processing.
- `completion.prepared_to_update`: worker preparation to service polling.
- `completion.popup_build`: native popup element construction, not GPU/layout completion.
- Candidate totals and popup mount/unmount counts; existing LSP traces also mark wire receipt, parsing and request writing.

In a perf build, `:CompletionPerfReset` resets samples and `:CompletionPerf` displays them in the existing native information panel. No continuous performance polling is added.

## Validation

- `cargo fmt --check` passed.
- `cargo clippy --locked --all-targets --features perf -- -D warnings` passed.
- Debug core/native-boundary/editor/checkpoint/floating/manager checks: **87 passed** in the regression run, plus the added escaped-quote regression passed separately (**88 distinct ordinary tests**). Seven measurements/real-server suites were explicitly ignored in the broad run.
- Release library/checkpoint/floating checks: **59 passed** in the regression run, plus the escaped-quote regression passed separately (**60 distinct ordinary tests**). Updated session/Save As/combining-prefix tests and the delayed-provider integration were also rerun in release. Six measurements/real-server suites were explicitly ignored in the broad run.
- Delayed asynchronous mock providers verify progressive publication, persistent session IDs through typing/backspace, incomplete/trigger-character request kinds, stable selection identity, late-range acceptance, buffer switching and cancellation. Existing stale resolve, diagnostics and workspace-edit tests also pass.
- Real installed rust-analyzer completion acceptance, hover and diagnostics passed in **debug and release**.
- Previously authorized temporary five-installer smoke suite passed: rust-analyzer 2026-10-19, vtsls 0.3.0, TypeScript 7.0.2, basedpyright 1.40.2 and Ruff 0.17.2. vtsls, TypeScript and basedpyright retained a session through another typed character and accepted the completion. Ruff diagnostics worked alongside basedpyright. All temporary installations were removed; global installations/settings were unchanged.
- `cargo build --release --locked` passed with the existing local `GPUI_FXC_PATH` shader compiler. The release executable is available at the path below.
- A brief Windows GUI startup-and-cleanup check was rejected by automatic approval review with “blocked by policy.” It was not run, and visual/native-input behavior is not claimed as verified.

## Remaining limits and manual review

- No fake/local-word provider was added. With no cached candidates, the first list still waits for the language server.
- Commit characters accept already usable/resolved items. If resolve is required and still pending, the character is typed normally rather than delaying input behind a server response.
- The existing restricted snippet grammar is retained; full interactive/mirrored tab-stop navigation is not introduced. Complex snippets fail safely.
- Lexical fallback is conservative, not a complete parser for every language. In particular, long multiline lexical constructs and embedded JSX/template-language expressions need manual review. Long current lines beyond the scan budget suppress pairing rather than scanning the document.
- Dynamic completion options are handled at the existing server scope; per-selector dynamic registrations are not advertised as a new fully implemented capability.
- No multicursor engine exists. Physical keyboard/IME behavior, pixel-level popup flicker and actual presentation latency require manual Windows testing. Linux/macOS were not validated.

From the repository root, launch `.\target\release\filavim.exe`.

1. Open a Rust source file with Ctrl+O, press I, and type a partial identifier without pausing. Continue typing and press Backspace. Expect one steadily updated menu, no close/open flash, and stable selected identity where it still matches.
2. Press Ctrl+Space, select with Down/Up, accept with Tab or Enter. Expect the right replacement and no raw snippet syntax. Escape closes the menu; a subsequent Escape returns to Normal mode.
3. In a TypeScript project with a configured server, type `object.` and continue its property prefix. Expect contextual suggestions promptly, then the same menu while filtering. Switch tabs while waiting; late results must not enter another document.
4. In a `.js` or `.rs` file, type `(`, `[`, and their closers. Expect paired insertion and owned-closer skipping. Type `(` then Backspace in an empty context: both characters should disappear. Pasted punctuation must remain unchanged.
5. Type `{` then Enter. Expect an indented interior line and aligned closing brace. Test with tabs/spaces settings. Undo/redo should preserve valid text and caret positions.
6. Select Unicode text in Visual mode, type `(`, then undo. Expect one surround transaction and correct restoration. Try quotes in comments, escaped quotes, and apostrophes; report unwanted pairing.
7. Use `:set completion.auto_trigger=false`, then Ctrl+Space; manual completion should still work. Use `:set pairs.enabled=false` to disable pairing. Restore both to `true` afterward. Confirm cursor animation, saving, tabs and LSP panels remain normal.
