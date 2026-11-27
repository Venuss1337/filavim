# Filavim — GPUI Checkpoints 3 and 4

## Scope and repository state

Worktree: `C:/Users/filet/Desktop/filavim-gpui`, branch `gpui-migration`.
Checkpoint 1 baseline is `48a8b89`; existing Checkpoint 2 modifications were
preserved, not reset, overwritten or automatically committed. Original terminal
`master` remains at `dbcfd3a`. No push/merge or unrelated feature migration.

Repository inspection confirmed functional tabs, per-document Rope/undo state,
background explorer, Tree-sitter, LSP transport/completion/hover/diagnostics and
native floating pickers. Their outstanding parity gaps remain documented in
`GPUI_CHECKPOINT2.md`: splits/tab pages, MRU/filesystem-management dialogs,
snippet-field cycling, command-based/resolved code actions and advanced Git UI.
This task does not rebuild those missing features.

## Stage A — implemented and checked before cursor effects

- Central theme roles for secondary text, borders, active line, warning and info.
  Rosé Pine Moon and embedded Agave Nerd Font Mono are retained.
- Active-line tint; a translucent full-height Normal/Visual block and Insert bar.
  Cursor widths use shaped grapheme/tab boundaries, not UTF-8 byte counts.
- Stable bounded document tabs, filename ellipsis, active underline, hover state
  and a separate close hit target. Existing modified markers remain.
- Explorer row spacing/indentation, directory text hierarchy, hover/selected states,
  truncated headings and responsive width reserving room for the editor.
- Status mode badge, truncated file information, right-hand cursor/diagnostic info;
  existing full-sized command/status typography remains. Popup borders, headers
  and footer typography now share theme values and clipping conventions.
- Diagnostic gutter colors distinguish errors, warnings and information.
- Scrolling reuses overlapping native shaped rows with the same document,
  revision, syntax epoch, horizontal clipping and search. Only entering rows are
  constructed; incompatible revisions/style/width still rebuild a viewport safely.
  The cache retains at most a viewport, not the entire document.
- Visible search-match spans are computed with the row, using a forward scalar
  scan, rather than allocating/scanning text and prefix-counting on every paint.
  Selection, IME, syntax and search remain mapped to shaped grapheme boundaries.

A brief native smoke asserted one-line scrolling shapes at most one entering row
and reuses overlapping rows. Edit/undo/redo, geometry, resize, picker input isolation,
confirmation cancellation, explorer and save checks passed before Stage B.
Settled editor frames remained 12 → 12 over two idle seconds.

## Baseline, captured before Stage B

Same Windows host (i5-12400F, Radeon RX 7600); release; LSP disabled for process
samples. Window timing means process spawn to native window metadata, not first
paint or display. Three observations: 265.53 / 239.15 / 247.22 ms (median 247.22).
File idle: 56.30 MiB resident / 60.33 MiB private; 250 ms CPU over 10,000.48 ms wall.
Project index with empty buffer: 55.43 MiB resident; 78.125 ms CPU / 10,005.85 ms.
These are noisy observations, not acceptance budgets or evidence of a regression.

Existing release backend benchmark (10,000 individual samples each):

| Operation | p50 µs | p95 µs | p99 µs | max µs |
| --- | ---: | ---: | ---: | ---: |
| Navigation + clipped viewport fragment | 18.5 | 23.3 | 40.8 | 177.2 |
| Typing + clipped viewport fragment | 39.5 | 87.4 | 118.2 | 334.4 |

Checkpoint 1 typing p99 was 111 µs, window observations 203–397 ms and memory
55–93 MiB. Backend observations remain in that range of scale; no causal speedup
or end-to-end input-to-display improvement is claimed. This benchmark excludes
GPUI shaping, GPU work, composition and display latency.
Raw baseline: `target/checkpoint2-measurements/checkpoint3-baseline.json`.

## Stage B — corrected Animated Cursor with Smear

The intended reference is Neovide's [Animated Cursor](https://neovide.dev/features.html#animated-cursor),
not its separate particle VFX. The previous particle and fading-stamp implementation
has been removed completely from the native frontend: no emission, RNG, particles,
lifetimes, halos, particle buffers, density settings, stamp history or rendering loops.
Old particle performance samples are not measurements of this corrected renderer.
The Stage A refinements, layout and all noncursor functionality are unchanged.

### Implementation

The existing controller/native-view split, scene cache, frame scheduler, colors,
mode shapes, caret geometry and lifecycle safeguards are reused. `gui/cursor.rs`
now holds four corners in fixed arrays, not a trail or another authoritative caret.
The editor supplies the immediate logical destination as before.

Each motion captures the current quadrilateral and computes a direction. Leading
and trailing ease-out timings produce a shared directional deformation; intermediate
corners interpolate by their projection along that direction. The leading edge
advances faster, the rear catches up and the quadrilateral contracts to the exact
rectangular destination. Retargeting starts at the current shape rather than a
previous logical position. A convexity guard snaps safely if an extreme geometry
change would fold the polygon. No corner/state allocation or velocity backlog.

This independently implemented, finite-duration interpolation is inspired by
Neovide's [corner timing implementation](https://github.com/neovide/neovide/blob/main/src/renderer/cursor_renderer/mod.rs)
and [configuration](https://neovide.dev/configuration.html), not an exact port of
its spring mathematics. It deliberately retains a subtle smear for short horizontal
moves too. No source was copied and no new rendering dependency was added.

`gui/cursor_view.rs` draws one shape: axis-aligned geometry uses a GPUI quad;
diagonal deformation uses GPUI's public convex `Path` fan (two triangles, no general
path tessellator). GPUI owns small transient path/scale vertex allocations on those
diagonal frames; the simulation itself has no heap allocations. The existing
cached editor remains a sibling, so cursor-only frames do not shape document text.

Short horizontal travel (up to two measured text cells, including Insert bars)
uses at most 40 ms by default. Other moves use 120 ms. Smear strength is 0..1.
Same-document cursor navigation, including numeric line commands and automatic
viewport following, animates regardless of distance. Command entry pauses and hides
the document cursor without discarding its origin. Document changes, resize/font/DPI
transitions, focus loss, off-screen carets and pauses above 180 ms retain cleanup/snap
behavior. Normal mode uses a brighter theme off-white block at 60% opacity; Insert
and Visual cursor colors remain unchanged.
Logical edits, Unicode/grapheme positions, selections and popup anchors are unchanged.
Animation stops exactly at settlement; the existing FPS cap, one pending timer and
CPU submission-cost guard are retained. No extra worker or continuous idle timer.

### Current settings and compatibility

Configuration remains `%APPDATA%/filavim-gpui/config.toml`, overridable using
`FILAVIM_GPUI_CONFIG`. Use existing `:set key=value` commands; settings persist
through the same validated atomic write. No restart or new settings interface.

```toml
[cursor]
enabled = true
smooth = true
smear = true
trail_size = 0.85
duration_ms = 120
short_duration_ms = 40
animate_in_insert_mode = true
reduced_motion = false
fps = 120
```

- `enabled=false`, `smooth=false` or `reduced_motion=true`: static cursor, no animation.
- `smear=false` or `trail_size=0.0`: smooth rigid cursor with no deformation.
- `trail_size`: finite 0..1. General/short durations: 16..250 ms; short duration
  never exceeds the general duration. FPS remains 24..120 (default 60).
- `animate_in_insert_mode=false`: Insert cursor snaps while other modes may animate.
- Previously saved explicit settings such as `smear=false` or `duration_ms=70`
  are respected; new defaults do not overwrite the file.
- Legacy `[cursor] pixiedust` and `intensity` entries are ignored only while loading.
  They are absent from runtime configuration and cannot be set through `:set`.
  Unknown unrelated options still fail validation. Startup does not rewrite the
  config, and subsequent setting writes preserve unrelated preferences.

### Minimal verification for this correction

Four targeted tests passed via:

```powershell
cargo test --no-default-features --lib gui::cursor::tests -- --nocapture
```

Coverage: horizontal/vertical/diagonal front/rear behavior and exact settlement;
continuous rapid reversals and glyph-aspect changes without nonconvex geometry;
time-step consistency; rigid movement with smear off; short Insert-bar timing;
scene/large jumps, suspension, reduced motion and master disable; old configuration
loading and isolated persistence preserving unrelated preferences. No user settings
or fonts were modified by tests. No full-suite reruns or new benchmarks.

Final correction checks passed:

- `cargo fmt` and `cargo fmt --check`.
- `cargo clippy --all-targets --all-features -- -D warnings`.
- `cargo build --release --locked` with the existing isolated FXC path.
- One bounded Windows release `--smoke-test` with `FILAVIM_CURSOR_SMOKE=all`:
  motion, edit/undo/redo, resize/caret geometry, viewport reuse, floating picker,
  confirmation, information, project and save checks all passed. Original fixture
  SHA-256 was unchanged. Editor paints settled at 50 → 50, cursor paints 52 → 52
  over two idle seconds; cursor_active=false. The viewport retained 23 rows,
  with 195 shaped / 40 reused through the entire short replay. Artifact:
  `target/cursor-correction-smoke.txt`.

This is native launch/prepaint/synthetic-handler verification, not visual
validation, physical-input/IME testing or input-to-photon measurement. No real
language-server session was rerun for this cursor-only correction. Earlier Stage A
numbers remain historical baseline observations only. Historical terminal-only
source files are retained as before and are not compiled into the native app.

### Exact manual walkthrough

Launch:

```powershell
& 'C:\Users\filet\Desktop\filavim-gpui\target\release\filavim.exe'
```

Ctrl-O opens a file; Ctrl-Shift-O opens a project. Before testing, if earlier saved
settings disabled smear, enter each command with `:` then Enter:

```text
:set cursor.enabled=true
:set cursor.smooth=true
:set cursor.smear=true
:set cursor.trail_size=0.65
:set cursor.reduced_motion=false
```

1. **Horizontal.** Open a source file, Escape into Normal, then A/D or Left/Right
   through a line. Expect one connected cursor stretching along travel and
   contracting at rest. Report detached copies, sparkles, folding or wrong settlement.
2. **Vertical.** Use W/S or Up/Down repeatedly. Expect the same connected vertical
   deformation. Report detached fragments, sideways drift or a misaligned block.
3. **Insert.** Run `:set cursor.animate_in_insert_mode=true`; press I and type a
   sentence. Expect immediate text and short bar transitions. Escape, U undoes,
   Ctrl-R redoes. Report input delay, incorrect edits, misalignment or raw artifacts.
4. **Rapid motions.** In Normal, hold S then W and alternate A/D quickly. Expect
   stable retargeting from the current shape, no growing lag and exact settlement
   after release. Report inversion, runaway stretch or continuing motion at rest.
5. **Line commands and jumps.** Enter `:8`, `:16`, `:200` then `:1` in a long file.
   Same-document cursor movement should animate, including when the viewport follows.
   Ctrl-Tab switches between files opened with Ctrl-O; document changes still snap.
   Report missing line-command animation or trails outside the editor.
6. **Disable and tune.** `:set cursor.enabled=false` should give immediate static
   movement with no residual motion. Re-enable; `:set cursor.smear=false` should
   retain rigid smooth movement. Restore smear and try `:set cursor.trail_size=0.9`.
   `:set cursor.reduced_motion=true` suppresses all animation. Report lingering
   motion after disabling. Restore preferred values with the same commands.
7. **Regression.** Open/edit/save with Ctrl-O, I, Escape, Ctrl-S; switch documents
   with Ctrl-Tab, scroll and resize, and navigate through tabs/emoji/combining text.
   With rust-analyzer, Ctrl-Space opens completion; arrows and Enter/Tab select,
   Escape dismisses; try `:Hover` and `:Diagnostics`. Minimize/restore and change
   focus. Expect unchanged editing, popups above the cursor, clean viewport snaps
   and no residue. Report alignment, file/undo, focus or popup regressions.

### Limits and Git state

Only Windows is built/launched here. Appearance, physical keyboard/mouse/IME,
DPI transitions and minimized-window behavior require the manual walkthrough.
Convex guard snapping is intentional for pathological intermediate shapes.
GPUI's native alpha/path rendering approximates the intended look; exact Neovide
spring behavior, blur and GPU/display timing are not claimed. No performance
benchmark campaign was run for this correction.

Changes remain uncommitted on `gpui-migration`, HEAD `48a8b89`, alongside the
previous checkpoint work. No push/merge. Original terminal worktree `master` at
`dbcfd3a` is unchanged. Correction touches cursor config/controller/view, existing
smoke reporting, compatibility rejection in `setup.rs`, README and this report;
no editing, LSP, explorer, tabs, theme or file-operation architecture was redesigned.


### Follow-up: line-command motion and Normal color

Numeric line navigation now retains the cursor origin through Command mode and
animates across automatic scrolling and longer same-document jumps. Normal mode
uses the theme's off-white text color at 60% opacity, keeping underlying glyphs
visible; Insert/Visual colors are unchanged. Focus loss, document replacement,
resize/DPI and disabled/reduced-motion guards remain. Added regression coverage
for :8/:16-scale and longer motions and compatible command/scroll transitions.

Follow-up verification: six targeted cursor/scene tests passed; formatting, Clippy with warnings denied and the Windows release build passed. Command-navigation appearance and the brighter cursor remain for manual review.


## Latest refinement — continuous input tracking and shared command caret

Insert-mode animation now uses analytically integrated critically damped tracking.
New keystrokes change the target without restarting an easing timeline or clearing
velocity; reversals discard momentum directed away from the new destination.
Backspace, tabs, Unicode and newlines use the same authoritative shaped caret.
The follower settles exactly below a subpixel/velocity threshold. Ordinary typing
has no added smear. `short_duration_ms` controls its response speed; disabling
Insert animation, reduced motion and master disable retain their existing meaning.

Normal/Visual movement retains the elastic quadrilateral. Faster leading corners,
a slightly longer strength-dependent short-motion tail, and default trail_size
0.85 make it more pronounced. The default notification cap is now 120 FPS.
Explicit saved settings, including a previous FPS/strength choice, are preserved.

Command/search input no longer draws a text-glyph caret. Its native shaped text
and measured window-coordinate insertion point feed the same cursor overlay.
Crossing between editor and command input temporarily retains elastic motion and
a spanning clip; once settled, command typing uses the input follower. Stretch is
bounded to avoid an enormous bridge across a large window. Long command text
scrolls horizontally to keep its actual caret in view. IME range geometry uses
that same command text layout, rather than an estimated offset from the editor.
Only one cursor is drawn. Other noneditable popups and native OS dialogs do not
receive artificial cursor transitions. Editor layout, palette and backend remain.

Per the user's instruction, no tests, Clippy, benchmarks, formatting checks or GUI
interaction were run for this refinement. An existing Insert geometry test was
adapted to the new tracking entry point without running it. The Windows release
build succeeded; startup-only launch is the sole runtime check. Earlier automated
results in this document refer to previous work and do not verify these changes.
Appearance, physical typing, IME and transition quality require manual review.

## Official Moon palette correction

The native theme now resolves all colors from the 15 exact official RGB values in
`src/palette.rs`. Current-line, selection and border roles use highlight_low,
highlight_med and highlight_high respectively. Floating surfaces and active tabs
use overlay; secondary labels use subtle and comments use muted. Diagnostic hints
use iris. Unclassified syntax punctuation uses subtle; the existing seven token
classes remain unchanged (there are no separate parameter/property/builtin classes).
Cursor and smear colors remain palette-derived with their existing alpha values;
animation timing, geometry, layout and behavior are unchanged.

Existing theme preferences are not rewritten. Overrides using official Moon RGB
values still work; off-palette overrides fall back to the role's official default.
Highlight names are now accepted by configuration validation. Native OS-controlled
window decorations and dialogs remain outside the application palette.

Validation for this correction: `cargo build --release --locked` succeeded on
Windows with the existing local FXC tool. No tests, Clippy, benchmarks or formatting
checks were run. A single startup-only launch is used; colors require user review.

## Compact custom Windows title bar

The native Windows title bar is hidden using GPUI's transparent-titlebar option.
Document tabs now occupy the top row at the command bar's font size and line
height, with reduced horizontal padding. A single small application-close × stays
at the right; it uses the existing unsaved-document confirmation and shutdown
flow. No minimize/maximize buttons are drawn. Empty space after the tabs is a
window drag area, while
GPUI retains border resizing. Overflowing tabs scroll independently of close.
This change does not alter the editor, command bar or cursor animation behavior.


Title-bar drag correction: the empty top-row area now starts a native Windows
move loop after a 4-logical-pixel drag threshold. It no longer uses caption hit
testing, so double-clicking that area does not maximize the window. Move requests
are posted to avoid reentering GPUI during input dispatch. Tabs and the close
button retain their existing behavior; border resizing remains native.
