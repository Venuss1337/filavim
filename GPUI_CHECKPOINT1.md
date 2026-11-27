# Native foundation checkpoint 1

Historical report. The checkout details, capabilities, and validation below describe that checkpoint.

## Workspace and scope

The supplied Desktop directory had no Git metadata. With explicit user
authorization, it was initialized and its source/tests/docs were committed as
`dbcfd3a` (terminal baseline). Build output and private `.local` assets were
excluded; the user's untracked `something.trxt` was left untouched.

- Terminal: `C:/Users/filet/Desktop/filavim`, branch `master`.
- Native: `C:/Users/filet/Desktop/filavim-gpui`, branch `gpui-migration`.
- Migration changes are left uncommitted for inspection.

## Reused engine and frontend boundary

`buffer.rs` retains Ropey, grapheme traversal, sparse display checkpoints,
revision/edit journal, grouped persistent undo/redo and newline handling.
`editor.rs` retains modal actions, selections, registers, motions, operators,
commands and save/open/search requests. Only its save request method's visibility
and frontend-neutral messages changed. `files.rs` is reused unchanged, including
atomic saves, symlink handling and literal search. `input.rs` keeps the same
keymap/remapping/leader implementation with frontend-neutral key data from
`key.rs`. Coordinate math used by the edit journal moved out of the LSP transport
into `coordinates.rs`.

The native modules are:

| Module | Responsibility |
| --- | --- |
| `gui/application.rs` | GPUI application/window, bundled font, lifecycle and explicit smoke harness |
| `gui/model.rs` | Existing Editor/Keymap ownership; UTF-16/IME bridge into the real Rope |
| `gui/io.rs` | Existing file/search functions and revision/document-checked completion |
| `gui/input.rs` | GPUI binding events translated into existing semantic keymap inputs |
| `gui/viewport.rs` | Clipped Rope text, tab expansion and scalar ↔ shaped-byte boundaries |
| `gui/view.rs` | Visible-row shaping/cache, cursor/selection, input, scroll and native dialogs |
| `gui/theme.rs` | Rosé Pine Moon colors |

There is one authoritative text buffer. The viewport cache contains only visible
presentation fragments and shaped lines, keyed by document/revision and viewport.
Ordinary cursor motion reuses shaped rows. Edits rebuild the viewport, not the
whole document. Horizontal clipping reuses existing sparse buffer checkpoints.
A cold jump into an unindexed long line still scans its prefix once.
GPUI handles glyph fallback, text shaping, logical pixels and DPI scaling;
caret/hit-test/selection positions use the resulting shaped byte positions.
The code does not force one glyph per Unicode scalar.

Native text input uses `EntityInputHandler`, with Rope scalar ↔ UTF-16 conversion,
surrogate-range adjustment, marked/preedit ranges, CRLF normalization and grouped
undo. Printable Insert/Command input reaches the native text handler rather than
being inserted a second time by key-down events. Normal/Visual bindings reuse
the existing keymap. Command input retains the original append-oriented model.

File/search work runs on GPUI's executor. One job is allowed in flight; retries
receive an explicit message. This bounds work and preserves save ordering without
adding a permanent IO worker. Editing continues during IO. Stale opens/searches
are rejected; a saved snapshot cannot mark newer edits clean or close over them.
Native open/save dialogs and Save/Discard/Cancel window-close protection are
included. No global config/font installation is performed.

`Cargo.toml` defaults to the `native` feature. `--no-default-features` builds/tests
the reusable core without GPUI. Only `tests/editor.rs` and `tests/native.rs` are
registered in this checkpoint; old TUI/service tests and examples remain as
historical sources and are not silently presented as native coverage. Ratatui,
Crossterm, PTY sessions, animation, smear and Pixiedust are absent from the native
module graph and dependency tree. Existing LSP/Git/workspace sources are retained
but their services/UI are not activated in this checkpoint.

## Dependencies, font and builds

GPUI is pinned to official crates.io `=0.2.2` and the entire dependency graph is
locked. This is the published monolithic API, not Zed main's newer split
`gpui_platform` API. No component toolkit, web technology or GUI server was added.
GPUI itself has a substantial dependency graph (including image decoding), even
though this viewport uses only text/quads.

The bundled, unmodified Agave Nerd Font **Mono** regular face is loaded directly
into GPUI's native text system. Its family, monospacing and selected Nerd Font
glyphs are tested. See `assets/fonts/README.md`, `OFL.txt` and
`NERD-FONTS-LICENSE` for pinned provenance, SHA-256 and redistribution notices.
Operator Mono and its development feature are not part of the native build.

```powershell
cargo check --all-targets --locked
cargo fmt --all -- --check
cargo clippy --all-targets --all-features --locked -- -D warnings
cargo test --locked -- --test-threads=1
cargo build --locked
./scripts/windows-build.ps1 -Release
cargo test --no-default-features --locked
```

Windows development was performed with Rust 1.93.1 on x86_64-pc-windows-gnu.
MSVC and Linux/macOS were not locally compiled/verified. GPUI's default platform
features retain the native Windows, macOS, X11 and Wayland backends. Linux needs
GPUI's system libraries; macOS needs Xcode/command-line Metal tooling.

On Windows, release GPUI builds compile HLSL with `fxc.exe`. Debug builds compile
shaders at runtime and succeeded without the full SDK installation. The build
helper searches `GPUI_FXC_PATH`, `.local/tooling/fxc/fxc.exe`, and installed SDK
locations. It never downloads/installs tools or changes persistent environment.

This host initially lacked FXC. For local validation only, Microsoft's official
NuGet `Microsoft.Windows.SDK.CPP` **10.0.26100.7705** package was downloaded into
ignored `.local/tooling`. Only `c/bin/10.0.26100.0/x64/fxc.exe` and its adjacent
`d3dcompiler_47.dll` were extracted there. FXC's Authenticode signature verified
as Microsoft Corporation. The SDK tools/package are not application assets and
must not be included when distributing the editor. Alternatively use a normal
Windows SDK installation and set `GPUI_FXC_PATH`.

Native settings use a separate `filavim-gpui/config.toml` directory under
APPDATA (Windows) or XDG_CONFIG_HOME / ~/.config. `FILAVIM_GPUI_CONFIG` overrides
it. The terminal's configuration/history are not modified or auto-migrated.
Minimal example:

```toml
font_size = 18.0
tab_width = 4
expand_tabs = true
auto_indent = true
line_numbers = true

[keybindings.normal]
# f = "right"
```

## Validation and measurements

Windows 11 Pro build 26200, Intel i5-12400F (12 logical CPUs), Radeon RX 7600,
Rust 1.93.1. Checks passed: `cargo check`, formatting, Clippy with warnings denied,
debug and release tests (**33 regular tests**, one ignored benchmark run
separately), and debug/release builds. The independent core check/test also
passed with `--no-default-features` (32 regular tests; GPUI key translation is
excluded in that configuration). The initial release failure due to missing
FXC was resolved using local official SDK tools as described above.

Three consecutive visible release smoke runs loaded the same temporary
65,053-scalar / 5,004-line fixture (Unicode, CRLF, tabs and a 5,000-character
line), exercised input/modes/undo/redo/Visual selection, resized and checked
shaped cursor/hit-test geometry. All checks and exits succeeded; the fixture's
SHA-256 stayed unchanged. Each final report recorded 4 frames before and after
the two-second settled idle interval, with 22 cached viewport rows.

| Native release measurement | Run 1 | Run 2 | Run 3 |
| --- | ---: | ---: | ---: |
| Spawn → visible titled window (ms) | 396.98 | 215.20 | 202.95 |
| Resident working set (MiB) | 93.11 | 55.31 | 55.07 |
| Private committed memory (MiB) | 99.42 | 56.41 | 56.11 |
| Process CPU consumed in ~1 s idle sample (ms) | 15.625 | 31.25 | 15.625 |

Window availability was sampled using process window metadata at 10 ms polling
intervals, not a first-pixel/first-presentation instrument. The startup median
is 215.20 ms across these three cache-warm runs; this does not establish cold
startup or a tail distribution. CPU accounting has coarse Windows granularity;
one-second samples are noisy. Process memory excludes separately accounted GPU
memory and is not a lifetime/peak guarantee. The larger first-run footprint is
reported without attributing its cause. Release executable size is 9,309,184
bytes in the final validated build.

A separate ordinary release launch (no smoke timers) remained responsive after
four seconds of settling, then consumed **218.75 ms process CPU over 10,001.98 ms**
of idle wall time. This is approximately 2.19% of one logical CPU, or 0.18% of the
12-logical-CPU host's aggregate capacity. Resident/private memory measured
54.79/59.44 MiB. A native clean-window close request exited successfully with
code 0. The process CPU is measurable despite the absence of application redraw
scheduling; attribution to GPUI, drivers or other runtime activity requires a
profile. Do not treat the no-redraw assertion as proof of zero idle CPU.

The optimized, GPU-independent benchmark recorded individual samples:

| 10,000 samples in a 1 MB line + 100,000-line document | p50 (µs) | p95 (µs) | p99 (µs) | max (µs) |
| --- | ---: | ---: | ---: | ---: |
| Warm horizontal navigation + visible fragment | 19.7 | 25.0 | 52.0 | 245.0 |
| Mixed ASCII/emoji/combining insertion + fragment | 39.3 | 79.4 | 111.3 | 275.8 |

These are backend/fragment wall times, not complete graphical frame or input
latency. An earlier navigation-only run measured p95/p99 54.2/98.6 µs; host
variation is material and no optimization speedup is inferred. Native allocation
volume, input-to-present percentiles, GPU frame time and cold startup remain
unmeasured. No performance guarantee is inferred from compilation or headless
measurements.

The standalone bounded native smoke harness is explicit:

```powershell
./target/debug/filavim.exe --smoke-test target/smoke.txt path/to/fixture.txt
```

It opens a real window, loads the fixture asynchronously, exercises the same
view key/text handlers in memory, checks modes/undo/redo/Visual selection,
resizes, verifies shaped caret/hit-test agreement on a long line, scrolls,
records frame counters and exits. The fixture is never saved or modified.
It is synthetic handler exercise, not physical keyboard/IME or visual QA.
The report must contain successful input/geometry checks and a loaded document.
A hidden window can suppress document repaint; use a visible native window for
paint validation. Normal launches have no smoke timers or periodic redraw task.

The core benchmark is separately runnable without a GPU or SDK:

```text
cargo test --release --no-default-features --test native native_boundary_performance -- --ignored --nocapture --test-threads=1
```

It measures 10,000 individual warmed horizontal navigation + 120-column fragment
samples in the middle of a 1 MB line, in a document with another 100,000 lines.
It then measures 10,000 mixed ASCII/emoji/combining text-input + fragment samples.
It excludes GPU shaping, event delivery, presentation and cold checkpoint scans.

## Manual acceptance and deferred work

Manually verify ordinary source, CJK/emoji/combining marks, tabs, CRLF, a 1 MB
line and many-line files. Exercise W/A/S/D, word jumps, I/Esc, Visual cut/yank/paste,
U/Ctrl-R and command mode; open and save a temporary file through dialogs and
`:e`/`:w`. Test both-axis scrolling, clicking/dragging selections, resize,
maximize/minimize, focus return, multi-monitor DPI changes and a real Windows
IME (composition, cancellation and candidate placement in document/commands).
Close dirty files: Save, Discard and Cancel; cancel Save As and confirm no loss.

Hands-on visual quality, physical input-to-display latency, DPI-monitor moves,
real IME delivery, native dialog interaction, minimize/maximize and full bidi
behavior are not established by the automated checks. Soft wrapping, syntax
highlight presentation, multiple documents/splits/tabs, project explorers,
LSP/Git UI and effects are deliberately deferred. Cursor effects are entirely
absent from this checkpoint, including idle cursor blinking.

The implementation stops at Checkpoint 1. Its native shell is functional; it is
not a claim that every eventual desktop editor feature has migrated.

References: [GPUI release](https://docs.rs/gpui/0.2.2/gpui/),
[official examples](https://github.com/zed-industries/zed/tree/69e2130295c2649963eb639fc70b4f2ee8ea1624/crates/gpui/examples),
[Windows SDK package](https://www.nuget.org/packages/Microsoft.Windows.SDK.CPP/10.0.26100.7705).
