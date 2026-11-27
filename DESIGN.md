# FilaVim design system: Lucent

Lucent is FilaVim's visual identity and the system that produces it. This document
explains the idea behind the interface, the rules that keep it coherent, and the
code that implements it. The code is the contract; if the two disagree, fix this
document.

| Concern | Source |
|---|---|
| Tokens, type scale, shell geometry, materials, components | [`src/gui/design.rs`](src/gui/design.rs) |
| Semantic roles, palettes, glass qualities | [`src/gui/theme.rs`](src/gui/theme.rs), [`src/palette.rs`](src/palette.rs) |
| The world, pane materials, mode light | [`src/gui/stage.rs`](src/gui/stage.rs) |
| Frosted-glass bakes and the procedural atmosphere | [`src/gui/frost.rs`](src/gui/frost.rs) |
| Springs and motion characters | [`src/gui/motion.rs`](src/gui/motion.rs) |
| Shell composition (capsules, panes, footer, lenses, welcome) | [`src/gui/shell_view.rs`](src/gui/shell_view.rs) |
| Editor surface (guides, rail, current line, selection) | `prepare`/`paint` in [`src/gui/view.rs`](src/gui/view.rs) |
| Caret light | [`src/gui/cursor_view.rs`](src/gui/cursor_view.rs) |
| Glyph names | [`src/gui/icons.rs`](src/gui/icons.rs) |

Engineering boundaries (input latency, ownership, coordinate units) live in
[AGENTS.md](AGENTS.md) and [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md).

---

## 1. The idea: glass in a lit world

Most editors are a rectangle of chrome around a rectangle of text. FilaVim is an
instrument made of glass, resting in a place.

The **world** is the user's wallpaper, or, when none is set, a procedural night
lit by soft colored glows. It fills the window and stays visible around and
between everything else. On it rest **panes** of frosted glass (the explorer and
the editor) and small **capsules** of glass (identity, tabs, window controls).
There is no title bar and no status "bar": chrome has dissolved into objects.

Three ideas make it Lucent rather than a themed editor:

1. **The world is part of the interface.** It is never dimmed to near-black or
   hidden behind full-bleed panels. Glass lets its color through; a lit pane over
   an emerald wallpaper is itself faintly emerald. Every user's FilaVim looks a
   little like the place they put it in.
2. **Light carries state.** The modal editor's most important state is its mode,
   and the mode is a light: the mark's core, the caret and its bloom, the current
   line number, the footer chip, the explorer's current-file light, and a tint
   that settles into the lit rim of whichever pane holds focus. You feel a mode
   change, and where your keystrokes go, at the edge of vision before you read it.
3. **Material carries hierarchy.** Instead of borders and fills, surfaces are
   made of a small set of materials (well, lens, plate, glass, pane), each lit
   the same way from above. What a thing *is* tells you how it behaves.

The lineage is deliberate. FilaScope and FilaMCUI established the vocabulary: one
frosted slab with a lit upper rim, film grain, a gliding lens for selection, ink
in a few emphasis levels, one signal hue, and springs with distinct characters.
Lucent translates that vocabulary to a tool used for eight hours a day, where the
document must always be the brightest, calmest thing on screen.

### What Lucent is not

Not a showcase of effects. Nothing pulses, shimmers or animates at rest. Glass
never lowers code contrast. Motion never delays input. Decoration that does not
carry meaning (a glow without state, a gradient without light) does not belong.

## 2. Principles

1. **Code is the subject.** It is the largest running text, drawn on the calmest
   glass. Chrome is smaller, dimmer and further from the center.
2. **Legibility is guaranteed, not hoped for.** Glass may be thin because the bake
   clamps luminance (§4.3). Ink is always drawn at full alpha.
3. **One light, from above.** Every raised surface has the same key light: a lit
   top rim that tapers along the corners, a faint inner sheen, a soft shadow below.
   Never light a surface from another direction.
4. **Color is signal.** The signal hue (`accent`) marks focus, primary actions and
   unsaved work; mode lights mark the mode; severities mark outcomes. Everything
   else is ink. Identity hues (file types, symbol kinds) identify; they never
   report state.
5. **Depth is structural.** A surface's job decides its layer; its layer decides
   material, radius, shadow and motion together.
6. **Keys are visible.** Every instrument shows the keys that drive it, as keycaps.
7. **Still at rest.** A settled FilaVim schedules no frames. Motion exists only to
   explain a change, and ends.
8. **Never color alone.** State pairs color with a glyph, a word or a position.

## 3. Composition

```text
 world ─────────────────────────────────────────────────────────────────────────
 ╭◉ project ◧╮          ╭─[ lens: main.rs ]── lib.rs ──╮          ⟳ work  ⊖ ⊕ ⊗   capsule row
 ╭ explorer pane ╮ ╭ editor pane ─────────────────────────────────────────────╮
 │ FILES      30 │ │   12  fn main() {          ╭ : command lens ──────────╮ ▮ │
 │ › src         │ │   13  │   let x = 1;       ╰──────────────────────────╯ │ │  overview rail
 │   main.rs   ● │ │   14  }                    ╭ results hang from lens ──╮ │ │
 │               │ │                            ╰──────────────────────────╯ │ │
 │               │ │ ───────────────────────────────────────────────────────── │
 │               │ │ ● NORMAL  ⧉ src/main.rs · feedback     ⊗ 0 ⚠ 1  Ln 12 Col 5 │  footer instrument
 ╰───────────────╯ ╰───────────────────────────────────────────────────────────╯
 world ─────────────────────────────────────────────────────────────────────────
```

`design::Shell::new(viewport, scale, docked, sidebar)` is the single source of
geometry. The stage paints materials from it and the editor lays content out in
it; never compute pane bounds anywhere else.

| Measure | Value | Notes |
|---|---|---|
| Frame of world (`margin`) | 12 px, 8 px when maximized/fullscreen | Panes never touch the window edge |
| Gap between panes | 10 px | World is visible between panes |
| Capsule height | `control() + 6` (35 at 18 px code) | Top row of floating capsules |
| Panes begin at | `margin + capsule + 8` | |
| Pane radius | `radius::SLAB` 18 | |
| Explorer width | user resizable 160–600, clamped to keep the editor ≥ 280 | Drag its right edge |

The top row has no bar. Bare world anywhere outside a pane or capsule moves the
window (double-click zooms); capsules stop that propagation. Windows caption
plates use native hit testing (`WindowControlArea`), which keeps Snap Layouts.
On macOS the row reserves no caption plates.

## 4. Materials and light

### 4.1 The material vocabulary

| Material | What it is | Built by | Used for |
|---|---|---|---|
| **World** | The image behind everything | `Stage` | Wallpaper or atmosphere, with `world_tint` |
| **Pane** | Large frosted glass resting in the world | `Stage` (bake or gradient) | Explorer, editor (future: splits, terminal) |
| **Glass** | Near-opaque floating glass above panes | `design::glass`, `design::capsule` | Popups, pickers, toasts, dialogs, capsules |
| **Well** | A recess cut into glass | `design::well` | Segmented tracks, search fields |
| **Lens** | A raised, brighter pane marking *the* selection | `design::lens` | Active tab, selected row, active segment, rail thumb |
| **Plate** | A control resting on glass | `design::button`, `keycap`, `icon_button` | Actions, keys, caption controls |
| **Light** | A small luminous dot | `design::light`, `design::mark` | Mode, unsaved state, status |

There is exactly one lens per selection group. A lens may glide (§7); a
well never moves.

### 4.2 The key light

Every raised surface is lit from above, identically:

- **Rim**: `design::rim` draws a rounded quad with only a top border. GPUI tapers
  a single-side border along rounded corners, which produces a crescent: full
  brightness across the top edge, fading to nothing down the upper corners. This
  is the same light FilaMCUI computes in its surface shader, achieved here with a
  stock primitive. Panes get theirs from the stage; floats, lenses and plates from
  their component.
- **Sheen**: a short `sheen` → transparent gradient just inside the top edge.
- **Edge**: a dim 1 px outline (`edge`) that the rim fades into.
- **Shadow**: two layers, a broad ambient shadow offset downward and a tight
  contact shadow, so surfaces read as lifted rather than outlined.

| Layer | Ambient shadow | Contact shadow |
|---|---|---|
| Pane | 0, 16 · blur 40 · spread −8 · `shadow`×0.7 | 0, 2 · blur 5 · ×0.45 |
| Floating glass | 0, 10 · blur 28 · spread −4 | 0, 1 · blur 3 · ×0.6 |
| Modal glass | 0, 24 · blur 64 · spread −4 | 0, 1 · blur 3 · ×0.6 |
| Lens | — | 0, 1 · blur 4 · ×0.5 |

### 4.3 Frosted panes

GPUI 0.2.2 has no backdrop sampling or custom shaders, and the world is a static
image. So a frosted pane is a texture, baked off the UI thread
([`frost.rs`](src/gui/frost.rs)):

1. **World reduction**, once per image: a box-filtered copy at most 480 px wide,
   blurred with three box passes of radius 8 (≈ Gaussian). Clear glass keeps a
   1280 px sharp copy instead.
2. **Bake**, once per pane geometry: for each device pixel of the pane, sample the
   reduced world through the same cover/contain mapping the stage paints with,
   apply `background.opacity` and `world_tint`, mix the glass gradient
   (`glass_top` → `glass_bottom`), apply the **legibility clamp**, then add
   film grain.
3. **Paint**: one textured quad per pane, with the pane's corner radii.

The **legibility clamp** is what lets Lucent glass stay thin. After tinting, each
pixel's relative luminance passes a soft knee: dark glass compresses anything
above 60 % of a 0.05 ceiling smoothly toward 0.05; light glass lifts anything
below a 0.62 floor. Hue and saturation survive, so a vivid world still colors the
glass, but comments (`muted` ink) keep at least ~3:1 and code ink ~9:1 contrast on
any wallpaper.

Grain is a stable per-pixel hash of ±5 (dark) or ±3 (light) 8-bit steps. It gives
the glass a material surface and removes gradient banding.

Bakes are keyed by every input that changes their pixels (viewport, pane rect,
scale factor, fit, opacity, all tints, grain, clamp). The first bake is
immediate; later ones wait 90 ms so a live resize bakes once at the end, while the
previous texture stretches. Results are generation-checked, and replaced textures
are released from the sprite atlas. Until a bake exists, and always for solid
glass, a pane is a gradient quad of the same tint.

### 4.4 Glass qualities

`[theme] glass = "frosted" | "clear" | "solid"` (FilaScope's three qualities).
`theme.transparent = false` forces solid glass and a plain world.

| Quality | World under glass | Pane alpha (dark) | Pane alpha (light) |
|---|---|---|---|
| `frosted` (default) | Blurred | 0.42 → 0.66 | 0.55 → 0.74 |
| `clear` | Sharp | 0.62 → 0.80 | 0.70 → 0.84 |
| `solid` | Hidden | 1 | 1 |

Floating glass uses `theme.surfaces.popup_opacity` (default 0.92) for its top and
+0.04 for its bottom. It is deliberately near-opaque: it covers code, and code
beneath must never compete with what floats above it.

### 4.5 The world

`background.image` sets the wallpaper (PNG, JPEG or WebP, bounded decode,
relative paths resolve against the configuration directory). `background.fit`,
`background.opacity` and `background.overlay_opacity` (the `world_tint`, default
0.15) shape it. With no image, the stage generates the **atmosphere**: a
1600 × 1000 night gradient in `base` lit by four broad glows in theme hues (aqua,
the accent, lilac, mint), screen-blended and grained. It regenerates when the
palette changes. `background.enabled = false` leaves a plain world.

## 5. Color

`Theme::new` resolves every role once per settings change. Renderers read roles;
they never hold hex values or palette constants.

### 5.1 Roles

| Family | Roles | Meaning |
|---|---|---|
| Ink | `text`, `secondary`, `muted`, `faint` | One hue at four emphasis levels; always opaque |
| Signal | `accent`, `matched` | Focus, primary action, unsaved state, matched characters |
| Mode | `mode(mode)` | Normal, Insert, Visual, Command lights |
| Outcome | `success`, `warning`, `error`, `info`, `hint`; `diagnostic(n)` | Severities and results only |
| World | `base`, `world_tint` | Night color, tint over the wallpaper |
| Materials | `glass_top/bottom`, `float_top/bottom`, `rim`, `edge`, `sheen`, `well`, `well_edge`, `lens_top/bottom/edge` | §4 |
| Interaction | `hover`, `pressed`, `hairline`, `popup_selection` | Ink-derived tints (6–10 %) |
| Depth | `scrim`, `shadow` | Modal dimming and shadows only |
| Editor | `selection`, `active_line`, `gutter`, `search`, `punctuation`, `token(t)` | Text surface |
| Identity | `hue(Hue)`, `completion_kind(n)` | File types and symbol kinds; never state |

### 5.2 Lucent palette

Pastel and evenly bright, so no token class shouts; tuned for luminous glass.

| Name | Dark | Light | Roles |
|---|---|---|---|
| night | `#080c14` | `#dfe5ee` | `base` |
| glass top / bottom | `#222b3a` / `#0d1119` | `#fdfeff` / `#eef2f8` | pane tint |
| ink 1–4 | `#f1f4f9` `#b6c1d1` `#7b889c` `#4d586a` | `#161d29` `#3f4a5c` `#6b7689` `#9fa8b7` | text, secondary, muted (comments), faint (gutter) |
| ice | `#7db4f4` | `#2766c4` | accent, info, functions, Normal |
| mint | `#72dcae` | `#11845a` | success, snippets, Insert |
| lilac | `#b9a4f6` | `#6248c9` | keywords, hints, Visual |
| amber | `#f5b84a` | `#a76800` | warnings, Command |
| coral | `#f2735f` | `#c53d2b` | errors |
| aqua | `#79dcd6` | `#0b7f86` | types |
| sage | `#a9dc9c` | `#3b7f2c` | strings |
| peach | `#f3b98d` | `#ae5a20` | numbers, booleans |
| rose | `#f494aa` | `#bf345a` | control flow |
| orchid | `#e3a3ee` | `#9535a8` | `self`, `null`, constants |
| gold | `#e9c87e` | `#8a6400` | attributes, preprocessor, search matches |
| slate | `#95a4ba` | `#5b6880` | operators, punctuation |

`variant = "muted"` keeps the dark glass with calmer hues. Cyberdream and Rosé Pine
Moon remain available as syntax palettes (`theme.name`); their materials are
derived from their own backgrounds by the same rules, so every palette gets the
same glass, light and composition.

### 5.3 Mode lights

| Mode | Lucent | Cyberdream | Rosé Pine Moon |
|---|---|---|---|
| Normal | ice | blue | pine |
| Insert | mint | green | foam |
| Visual | lilac | purple | iris |
| Command and others | amber | cyan | gold |

The mode color appears in: the mark's core (identity capsule), the caret and its
bloom, the current line number (bold), the footer chip, the explorer's
current-file light and, at reduced strength (45 % dark, 25 % light), the rim of
the **focused pane** and a broad glow in the world above the editor pane.
Unfocused panes keep a neutral rim at 70 % strength: focus is light. Rims and
glow settle with the `Settle` spring; everything else changes instantly.

## 6. Typography

FilaVim uses one family: **Agave Nerd Font Mono**, bundled in Regular and Bold.
Everything is monospaced, chrome included, which gives the interface its
instrument character and makes widths predictable (the tab well computes tab
positions from the advance instead of measuring).

`design::Scale::new(font_size)` derives the scale from the code size:

| Step | Formula | At 18 px | Use |
|---|---|---|---|
| `display` | `round(code × 3.4)`, 40–96 | 61 | The welcome wordmark; reserved for heroes |
| `code` | `font_size` | 18 | Document, completion labels, hover and signature text, command input |
| `ui` | `round(code × 0.84)`, 12–17 | 15 | Tabs, explorer, pickers, dialog titles |
| `small` | `ui − 2` (≥ 11) | 13 | Footer, buttons, secondary paragraphs |
| `micro` | `ui − 3` (≥ 10) | 12 | Keycaps, section labels, counts, timestamps |

**Bold** marks identity and emphasis, never decoration: the project name, panel
and toast titles, section labels, the mode chip, the current line number, the
command-lens chip and the wordmark. Body text, code and labels stay Regular.
Chrome is never italic; italics belong to the optional `italic_comments`.

Section labels are `micro`, `muted`, bold and UPPERCASE (`design::section`). Paths
are relative to the project root with the Windows verbatim prefix removed
(`design::display_path`, presentation only).

## 7. Motion

Motion explains change and then stops. Owners sample a `motion::Motion` while
rendering and request another frame only while it is `active()`.

| Character | Tuning | Feel | Used by |
|---|---|---|---|
| `Quick` | 7 Hz, ζ 1.0 | Immediate, no overshoot | Reserved for hover/press feedback |
| `Glide` | 3.6 Hz, ζ 0.86 | Brisk travel, a trace of overshoot | Tab lens position and width; command lens entrance |
| `Settle` | 2.2 Hz, ζ 1.0 | Calm, ambient | Mode light on rims and world glow |

Other motion, each owned by its own entity:

- **Caret**: the four-corner simulation in `cursor.rs` (configured by `cursor.*`),
  now drawn in the mode color with a 10 px bloom on settled frames.
- **Toasts**: 180 ms entrance, 220 ms exit, lifetime bar; one owned timer.

Rules:

- Motion is presentation only. It never decides an edit position, and logical
  state is always current before anything animates.
- Typing-driven surfaces (completion, signature help) appear instantly.
- Springs retarget without restarting; a stall longer than 0.5 s settles instead
  of replaying.
- `cursor.reduced_motion = true` snaps every spring and removes toast slides.
- The stage is a cached root sibling *beneath* the editor and the caret a sibling
  above it, so ambient light and caret frames never rebuild editor layout.

## 8. Components

Components are stateless functions in `design.rs` returning unattached GPUI
elements. Callers own ids, listeners, focus and state.

| Component | Contract |
|---|---|
| `Shell::new(viewport, scale, docked, sidebar)` | The composition (§3). `panes()` lists pane bounds, explorer first. |
| `glass(theme, Elevation)` / `capsule(theme, Elevation, corner)` | Floating or modal glass with gradient, edge, rim, sheen and shadow. Capsules take an explicit radius (pills). |
| `well(theme, corner)` | Recess for tracks and fields. |
| `lens(theme, corner)` | The selection pane; children sit on it. |
| `rim(color, corner)` / `sheen(theme, corner, height)` | Key-light parts for custom surfaces; add as first children of a surface with a 1 px border. |
| `shadow(theme, Elevation)` | Shadow stacks; `glass` already applies them. |
| `scrim(theme, viewport)` | Full-window dim beneath a modal; no listeners. |
| `mark(color, size)` | The FilaVim mark: a glass lens whose core is lit in `color`. |
| `light(color, size)` | A status light with a soft glow. |
| `button(theme, scale, id, label, Tone, enabled, focused)` | Pill plate. `Primary` is the recommended choice (≤ 1 per group), `Secondary` ordinary, `Ghost` plate-less until hovered, `Danger` destroys data and is never default. Focus adds an accent ring and glow. |
| `icon_button(theme, id, glyph, size, extent)` | Round, plate-less until hovered. |
| `row(theme, id, selected)` | List row; the selected row is a lens. Lists get `space::XS`–`SM` side padding. |
| `badge(text, color, size)` | State chip: light + label on a faint tint. Not clickable. |
| `keycap(theme, scale, key)` / `hints(theme, scale, &[(keys, meaning)])` | Keys as small plates; every instrument shows its keys. |
| `section(theme, scale, text)` | Uppercase group label. |
| `heading(theme, scale, glyph, color, title)` | Panel title with its signal glyph (bold). |
| `notch(theme, glyph, color, size)` | A small glass tab on a minimal float's top edge naming its source. |
| `divider(theme)` | Horizontal hairline. |
| `icon(glyph, size, color)` | A glyph in a square slot so mixed icons align. |
| `severity(theme, n)` | LSP severity → (glyph, color). |
| `typeface(div, theme, size)` | Root font setup for an independent overlay entity. |

## 9. The shell

### Capsule row

- **Identity capsule**: the mark (mode light), the project name (bold; click opens
  the project picker) and the explorer toggle.
- **Tab well**: one glass capsule holding every document, centered when it fits
  and scrolling when it does not. The active tab sits on a lens that glides
  between tabs and resizes as it travels. Inactive tabs are muted with their
  file-type hue at 55 %. An unsaved document shows an accent light that the close
  plate replaces under the pointer.
- **Work capsule** (only while something runs): a sync glyph in the accent and a
  sentence.
- **Caption plates**: minimize, maximize/restore, close as round glass plates; close
  turns coral under the pointer.

### Explorer pane

A `FILES` section label with the entry count, then rows: chevron (faint), file-type
glyph, name. Directories are `secondary`, files `muted`. The open file is a lens
with a light in the current mode color; keyboard focus moves the lens. The right
edge of the pane resizes it.

### Editor pane

- **Gutter**: numbers in `faint` at `code − 2`; the current line number is bold in
  the mode color; diagnostics put a round severity mark in the gutter margin.
- **Current line**: a faint (`active_line`, 3.5 % ink) rounded band across the
  text column, inset from the pane edges.
- **Indent guides**: hairlines at each indentation level (detected indent unit);
  blank lines continue the shallower neighbour; the guide enclosing the cursor's
  block is brighter (ink 16 %).
- **Selection**: `selection` with 3 px radius. **Search**: gold tints, the current
  match filled and outlined. Matches keep their syntax colors.
- **Overview rail**: a 5 px track on the right edge with a lens thumb for the
  visible span, plus 2 px ticks for search matches (gold) and diagnostics
  (severity; errors win a shared pixel row). Inputs are capped and ticks are
  deduplicated per pixel row.
- **No end-of-buffer markers**: glass below the last line stays clean.
- **Footer instrument**: mode chip (light + bold label), file glyph, directory
  (muted) and name (ink), `modified` with an accent light, then feedback
  (secondary; errors coral with a glyph). On the right: search position, the server
  glyph and diagnostic counts when language intelligence is attached, and `Ln`/`Col`
  with figures in ink.

### Floating instruments

- **Command lens**: Command mode rises as a pill of glass near the top of the
  editor pane (Glide entrance). A chip shows `:` (or the search glyph for `/`) in
  the Command light; the input is shaped like code, so the caret travels from the
  text up into the lens. Hints on the right.
- **Pickers hang from the lens**: file, grep, buffer and symbol results appear
  directly beneath the command lens at its width, so query and results read as one
  instrument. Pickers opened without the lens (the palette) drop from the same
  place. A light scrim dims the editor; outside clicks dismiss.
- **Contextual floats** (completion, documentation, hover, signature,
  diagnostics) sit at the caret with a notch naming their source, fit their
  content, and never animate.
- **Help HUD**: `F1`'s key hints as a glass strip at the foot of the editor pane.
- **Notice capsule**: the missing-language-tools notice floats at the foot of the
  editor pane with `Manage…` and `Dismiss`.
- **Toasts**: glass cards inside the editor pane's top-right corner: a lit glyph
  plate in the category color, bold title, UTC time, close plate, message, and a
  glowing lifetime bar.

### Modals

- **Dialog**: modal glass over a scrim: heading with the decision glyph, a subject
  line, an explanation, optional details, a divider, then key hints and pill
  buttons. The existing default and order are kept; arrows move the focus ring.
- **Language Server Manager**: modal glass with the mark and a bold title, scope and
  trust badges, a glowing search well, the filter as a pill well with a lens,
  maintenance plates, a catalog/details split and a shortcut strip.

### Welcome

An untouched, unnamed, empty document shows the wordmark (`display`, bold) with
the version chip, a one-line tagline, a `BEGIN` list of actions with their keys,
and the project path. It takes no input: typing or clicking simply begins.

## 10. Interaction states

| State | Treatment |
|---|---|
| Hover | `hover` tint; text brightens one ink level. Never on the lens. |
| Pressed | `pressed` tint |
| Selected | Lens (gradient, lit rim, contact shadow), `text` ink |
| Keyboard focus | 1 px `accent` ring with a soft accent glow |
| Disabled | `faint` text, hairline outline, no plate, pointer or listener |
| Active document | Tab lens; explorer lens with the mode light |
| Unsaved | Accent light on the tab and footer, the word `modified` |
| Working | Work capsule with the sync glyph and a sentence; no spinners |
| Loading / empty | A muted glyph and a sentence |
| Error | Coral glyph and text; details stay in their owning panel |

Clicks on a float stop at the float; outside clicks dismiss non-modal popups;
modals block input beneath them. Styling never changes keyboard routing
(`popup::route` and the view own it).

## 11. Comfort and accessibility

- Contrast is enforced by the bake's clamp, not by asking users to pick a dark
  wallpaper. Ink is always opaque.
- Text scales with `font_size`; chrome never drops below 10 px.
- Every state has a glyph or a word; every action has a keyboard path, shown.
- Reduced motion is honored everywhere motion exists.
- No idle animation, no flashing, no moving backgrounds: long sessions stay calm.
- Known gaps: no tooltips on icon-only plates, no screen-reader bridge in GPUI
  0.2.2; IME, DPI and high-contrast behavior need manual checks.

## 12. Cost model

Lucent spends GPU and memory deliberately, and nothing at rest.

- **Per frame**: the world image, one textured quad per pane, a handful of shadows
  and quads for rims, sheens and lights. The stage is cached and re-renders only
  when it changes (mode light, sidebar width, resize, bake completion).
- **Per keystroke**: unchanged editor work plus indent-guide scanning of visible
  rows and a bounded rail pass (≤ 2048 search ranges, ≤ 1024 diagnostics).
- **Memory**: one baked texture per pane at device resolution (about 3–4 MiB for
  a 1260 × 750 pane at 100 % scale; capped at 4096 px per side), the decoded
  wallpaper, a reduced float copy of the world (≈ 1.5 MiB frosted), or a 6 MiB
  atmosphere. Replaced textures are released from the atlas.
- **Background work**: one decode per wallpaper change; one bake per pane per
  settled geometry or theme change.
- **Frames**: springs request frames only while active; reduced motion requests
  none. Deferred stage updates are sent only when their value changes.

## 13. Extending Lucent

Checklist for any new surface:

1. **Choose the material first.** Is it a pane, floating glass, modal glass, a
   capsule, a well, a lens or a plate? The material decides fill, edge, light,
   radius, shadow and motion.
2. **Compose from `design::`**; add a component (with a `///` contract and a row
   in §8) rather than restyling raw `div()`s.
3. **Roles and tokens only.** No hex values; no ad-hoc sizes where a token exists.
4. **Write every state** (§10), including empty and loading.
5. **Show the keys.**
6. **Light it from above**, and only once.
7. **Stay still and bounded.** Request frames only while a spring is active; keep
   heavy work off the UI thread; use `uniform_list` for long collections.
8. **Check** Lucent Dark, Muted and Light, Cyberdream, Rosé Pine Moon; frosted,
   clear and solid glass; a bright photo wallpaper, the user's wallpaper and the
   atmosphere; 14 px and 22 px code; maximized and windowed.

### How planned features fit

- **Editor splits**: each split is its own pane. Extend `Shell` to return the
  editor area as a list of pane bounds and give the stage one material slot per
  pane; the gap between splits is world. `Stage::set_layout` already lights only
  the focused pane's rim; extend it to a focused index. Unfocused panes keep
  `faint` numbers and no current-line band. Tabs belong to the focused pane; the tab well can show one
  well per pane group or follow focus.
- **Integrated terminal**: a pane beneath the editor pane (same material, its
  own footer strip), toggled by a capsule in the top row. Terminal text uses the
  code size and ink roles; its cursor uses the same caret light.
- **Git**: hunks are 3 px gutter bars (`success`, `warning`, `error`) and rail
  ticks of the same colors; line tints stay ≤ 12 %. A status and history view is a
  pane or a manager-style modal with a segmented well; branch state belongs in the
  identity capsule.
- **Project management**: recent projects and sessions use the picker hanging from
  the lens; the welcome grows a `RECENT` section using rows and keycaps.
- **Settings UI**: a manager-style modal: section labels, pill segmented wells for
  enumerations, plates for actions; changes apply live (the stage rebakes
  asynchronously).
- **Inlay hints and folding**: inlay hints are `muted`, `small`, on a `hover` pill,
  never brighter than code; fold chevrons are `faint` gutter glyphs.
- **Configurable keybindings**: rendered with `keycap`; editing them is a
  manager-style modal.
