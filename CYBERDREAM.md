# Cyberdream native theme and wallpaper

## Sources and licensing

Palette and highlight roles follow [Cyberdream](https://github.com/scottmckendry/cyberdream.nvim):
`lua/cyberdream/colors.lua` (current filename; `colours.lua` is absent),
`extensions/base.lua`, `treesitter.lua`, `blinkcmp.lua`, `cmp.lua`, `telescope.lua`,
and `util.lua`. Upstream's MIT notice is retained in
`assets/licenses/cyberdream-MIT.txt` (Scott McKendry).

No reference screenshots were attached in this session. No wallpaper was identified,
licensed, downloaded, or bundled as Cyberdream artwork. The application accepts a
user-supplied local file. A synthetic checker image was used only as an ignored local
verification fixture.

## Configuration

Cyberdream Dark is the default for an unset theme. Existing explicit
`theme_name = "rose-pine-moon"` and legacy `[theme]` Moon palette overrides remain
readable. A new `theme.name` takes precedence over the legacy selector.

The existing configuration file is `%APPDATA%/filavim-gpui/config.toml` on Windows,
or the existing `FILAVIM_GPUI_CONFIG` override. No new settings UI is required.

```toml
[theme]
name = "cyberdream"          # cyberdream | rose-pine-moon
variant = "dark"            # dark | muted | light (Cyberdream)
transparent = true          # composited backgrounds inside an opaque window
saturation = 1.0            # 0..1, upstream-style HSL desaturation
italic_comments = false

[theme.surfaces]
editor_opacity = 0.0        # theme-colored tint over the wallpaper
popup_opacity = 0.30        # visibly transparent floating tint; text stays opaque
sidebar_opacity = 0.0       # wallpaper continues through the explorer

[background]
enabled = true
image = "C:/Pictures/wallpaper.webp"
fit = "cover"              # cover | contain | stretch
opacity = 1.0
    # Themed readability overlay: dark for Dark/Muted, light for Light.
overlay_opacity = 0.45
```

All opacity/saturation values must be finite and within 0..1. Invalid enum values,
unknown options and malformed values fail validation before the configuration is
written. Text opacity is independent and remains fully opaque. For bright or busy
artwork, increase `background.overlay_opacity` and/or `theme.surfaces.editor_opacity`.
Contrast depends on the supplied artwork; the default tint cannot guarantee contrast
for every possible image.

Examples using the existing command bar:

```text
:set theme.name=cyberdream
:set theme.variant=muted
:set theme.variant=light
:set theme.name=rose-pine-moon
:set background.image=C:/Pictures/wallpaper.png
:set background.fit=contain
:set background.overlay_opacity=0.7
:set theme.surfaces.popup_opacity=0.30
:set background.enabled=false
:BackgroundReload
```

Relative image paths resolve against the configuration directory, independently of
project changes. `:BackgroundReload` explicitly re-reads the same file after an external
image edit. There is no filesystem polling or image watching during ordinary editing.
Theme and background settings update live without restarting language servers.
In transparent mode the title/tabs row, status line and command line share the root
wallpaper and readability overlay, rather than using nearly opaque panel fills.
The active tab retains a translucent selection tint. Popup opacity stays independent.

## Minimal floating UI refinement

Contextual completion, hover, signature help and documentation share native floating
chrome: a 1px neutral outline, 30% default tint and a small corner `⚡`, cyan `i`, or
diagnostic `!`. Assistance headers and the completion instruction footer are removed;
centered pickers retain their useful labels. Selected completion rows use a subtle
35% theme-highlight tint. Explicit saved popup opacity and opaque-mode preferences
remain supported. No desktop transparency, blur, shadow or animation was added.

Signature labels and function-like documentation headers receive lightweight syntax
colors; the active signature parameter is cyan and bold, with validated UTF-8 spans.
Prose retains its existing plain-text rendering; this is not a new Markdown engine.
Completion and signature help retain separate presentation when both are available.
The secondary signature uses free space opposite the completion menu and stays hidden
when there is insufficient non-overlapping space. Existing revision validation applies.

The status separator is removed. The explorer's 3px resize hit region remains but
has no visible fill. Mode text uses `-- NORMAL --`, `-- INSERT --`, `-- VISUAL --`,
`-- COMMAND --` (and existing auxiliary mode labels), with semantic foreground colors
and no badge background. Visual Line/Block and Replace are not existing backend modes.

Refinement verification: `cargo fmt`, `cargo check --locked`, three targeted signature/
placement/coexistence tests and the existing palette/mode-color test passed. The
Windows release build passed. No live GUI or screenshot matching was performed for
this refinement; no reference screenshot was attached in the accessible conversation.

## Architecture and mappings

- `src/palette.rs`: authoritative typed Cyberdream Dark, Muted and Light palettes;
  existing 15-color Moon palette retained. Upstream neutral blends are computed here.
- `src/appearance.rs`: frontend-independent typed theme names/variants, surface
  settings, image settings and fitting modes; validation/persistence regression tests.
- `src/config.rs`: backward-compatible parsing of new theme options alongside legacy
  Moon overrides and the existing selector/configuration format.
- `src/gui/theme.rs`: resolves semantic colors once per settings change. Rendering uses
  editor, text, sidebar, surface, popup, border, selection, cursor, gutter, search,
  diagnostic, completion-match/kind and syntax roles rather than hex literals.
- `src/gui/wallpaper.rs`: owned asynchronous decode task, generation validation,
  reusable `Arc<RenderImage>`, explicit GPU-image retirement and clean failure fallback.
- `src/gui/view.rs`: one cached wallpaper entity beneath existing editor/chrome/popups;
  live settings integration, popup labels, search foregrounds, optional comment italics,
  and a readable command-line surface. Existing popup IDs, scroll handles and completion
  sessions are preserved.
- `src/gui/completion.rs` / `services.rs`: expose the current session query and produce
  bounded UTF-8-safe visible-label highlights; signature help emphasizes the active
  parameter in cyan, converting LSP UTF-16 label offsets to validated UTF-8 ranges.
  Provider/request behavior is unchanged.
- `src/syntax.rs`: extends existing Tree-sitter node classification with booleans,
  operators, statements, preprocessor constructs, parameters and properties. Existing
  incremental parsing, edit projection, cancellation and viewport shaping are retained.
- `Cargo.toml` / `Cargo.lock`: reference the same `image` 0.25.10 already linked by GPUI;
  PNG/JPEG/WebP support adds no separate image framework or GPU renderer.

Exact default RGB palettes come from upstream. Strings are green, functions blue,
types purple, keywords orange, statements magenta, operators purple, constants pink,
numbers orange, booleans/preprocessors cyan, comments grey and identifiers white in
Dark. Tree-sitter properties and parameters follow upstream Identifier/foreground
links, rather than inventing extra colors. Syntax classification falls back to the
foreground when the existing engine has no more specific token.

Diagnostics use red/yellow/blue/cyan. Cursor and insertion cursor use the foreground,
matching upstream Cursor/ICursor. Normal floats use the base palette background,
cyan titles and neutral borders; completion/documentation uses upstream's blended
Blink border, cyan matches and Blink category colors. Selection and active tabs use
`bg_highlight`. Search uses foreground-colored fill and a contrasting `bg_alt` text
color through the existing shaped line.

Native adaptations: background alpha values are local compositor settings; current
line fill is subdued with alpha, active line numbers retain the existing clear cyan
accent, and native sidebar/chrome become transparent in transparent mode, sharing
the full-window tint. Opaque mode retains secondary palette surfaces. Neovim highlight
inheritance is mapped into Filavim's existing token categories rather than porting Lua.
The existing client has no LSP semantic-token rendering pipeline; this task does not
claim one. Existing grammars remain Rust/Python/JavaScript/JSON; no TypeScript grammar
or unrelated language subsystem was added.

## Image lifecycle and performance

Layer order is opaque window/base -> one full-window cached image -> readability
overlay/editor tint -> existing viewport/gutter/chrome -> deferred floating UI ->
existing cursor layer. The image does not scroll with document content. GPUI's
ObjectFit implements centered Cover/Contain/Stretch, clipping and DPI-aware drawing.
Neither the OS window nor all its child text is made transparent.

Only an enabled/path change or explicit reload starts decoding. Fit, opacity, theme
changes and keystrokes reuse the same image ID and GPU cache. File reading, format
inspection, decoding and RGBA-to-BGRA conversion run on GPUI's background executor.
A superseded task cannot publish over a newer generation. Animated containers are
rendered as a single static frame, so wallpaper does not schedule idle animation.

Limits: regular local file, at most 64 MiB compressed, 32 megapixels, 16,384 pixels
per dimension, and a 128 MiB decoder allocation limit (the image library documents
allocation limits as best-effort; dimension limits are strict). Missing, corrupt,
unsupported and oversized files fall back to the palette base and emit an actionable
message. No network image loading, blur, processing during typing, or new permanent
worker is introduced.

GPU uploads are cached by image ID; no per-keystroke decode or texture upload is
requested. GPUI may still sample the cached wallpaper when compositing a changed
frame. No damage-only redraw, GPU timing, idle-CPU or memory-improvement claim is made.
Theme changes invalidate only existing viewport shapes, not document contents or syntax
trees. Image/opacity changes are infrequent settings operations.

## Validation

- `cargo check --offline` and subsequent `cargo check --locked`: passed.
- `cargo fmt --check`: passed.
- `cargo clippy --locked --all-targets --features perf -- -D warnings`: passed.
- `cargo test --locked --lib --test editor --test native --test checkpoint2 --test floating --test lsp_manager`:
  **95 passed**, seven explicitly ignored benchmark/real-server tests. Includes
  settings persistence, all three image formats, corrupt/missing/oversized image
  rejection, BGRA channel order, cached image identity, fitting geometry, official
  colors, alpha isolation, legacy settings, editing/save/IME/cursor/LSP regressions.
- New Unicode completion-match byte-range assertion: passed in a targeted rerun.
- Targeted signature-help test: passed for string parameter labels, UTF-16 ranges
  after emoji, invalid surrogate boundaries and out-of-range labels.
- `cargo build --locked` and `cargo build --release --locked`: passed using the
  existing local GPUI FXC toolchain. Final command-line surface and active signature
  parameter adjustments are included in the final release rebuild.

Native visual check: the debug build launched into a real Windows GPUI window.
Cyberdream Dark without wallpaper and the full-window synthetic wallpaper were
observed. The wallpaper was continuous across the editor and gutter. The bright
fixture exposed weak command-line contrast. The initial themed chrome fix was later
replaced by shared full-window tinting to keep the wallpaper visible across all
chrome. Increase the global overlay for bright artwork. The follow-up transparency
correction passed its targeted theme test and release build, but has not been visually
rechecked. Cursor geometry and timings were not changed.

Further UI activity was stopped after user activity/minimization interrupted the check.
The temporary wallpaper preference was restored byte-for-byte from the original
configuration, with a comparison preventing overwrite of unrelated preference changes.
Floating completion/docs, other variants, artwork contrast, resize/maximize and
multi-monitor DPI still require manual visual checks. Headless fitting/color tests
are not substitutes for those checks. No reference-screenshot fidelity is claimed.

## Manual checks

From the repository root, launch `.\target\release\filavim.exe`.

1. Open a Rust file using Ctrl+O. Verify strings/functions/types/booleans/comments and
   that Normal/Insert/Visual editing, save and undo/redo work as before.
2. Set `background.image` to your PNG/JPEG/WebP and adjust the overlay. Scroll the
   file; artwork should remain stationary and continuous through the gutter.
3. Switch `background.fit` between cover/contain/stretch. Resize/maximize; expect
   centered crop, letterboxing or stretching respectively, with no image re-decoding.
4. Type a prefix with an active LSP or press Ctrl+Space. Verify persistent completion,
   cyan matches, category-colored kind labels, selection and the documentation panel.
5. Open `:Hover`, `:Diagnostics` and `:LspManager`. Check borders, readable popup
   surfaces and consistent severities. Escape should retain existing behavior.
6. Switch `theme.variant` through light/muted/dark, then select rose-pine-moon and back.
   Verify no editor or LSP restart, settings persist, and cursor smear stays intact.
7. Point at a missing or corrupt file. Expect a message and solid theme fallback;
   editing must remain responsive. Disable background to restore the plain theme.
