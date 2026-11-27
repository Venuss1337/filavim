# FilaVim design system

This is the visual and interaction foundation for every FilaVim interface. It
describes what the native GPUI application implements today. The contract lives in
code: tokens and components in [`src/gui/design.rs`](src/gui/design.rs), glyphs in
[`src/gui/icons.rs`](src/gui/icons.rs), and semantic colors in
[`src/gui/theme.rs`](src/gui/theme.rs). If this document and those files disagree,
the code is authoritative and this document needs fixing.

Engineering boundaries (input latency, ownership, coordinate units) are in
[AGENTS.md](AGENTS.md) and [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md). Theme and
wallpaper configuration is in [CYBERDREAM.md](CYBERDREAM.md).

---

## 1. Direction: Night Glass

FilaVim is a precision instrument laid over the user's own wallpaper. The document
is the subject. Around it, chrome is a quiet frame of hairlines and smoked glass,
and color appears only where it carries meaning.

Three ideas shape every screen:

- **The plane and the glass.** The editor and its docked chrome are transparent
  over the wallpaper (the *plane*). Anything that rises above the plane, such as a
  completion menu, a picker, a toast or a dialog, is *smoked glass*: nearly opaque,
  rounded, outlined with a hairline and lifted by a soft shadow. Code never shows
  through glass, so floating content stays readable over any wallpaper.
- **Color is signal.** Neon palette colors mean something: the current mode, a
  search match, an error, an unsaved change, keyboard focus. Neutral greys carry
  everything else. File-type and symbol-kind hues are the single exception, and they
  identify things; they never report state.
- **The mode is the pulse.** A modal editor's most important state is its mode.
  One color per mode drives three places at the edges of attention: the status
  badge, the active tab's underline and the current line number. You can read the
  mode from any of them without looking at the status bar.

What FilaVim is not: a web dashboard, a terminal imitation, a collection of cards,
or a showcase of effects. There are no gradients, glows, blurs, decorative icons
or idle animations in the chrome.

## 2. Principles

1. **Code is the brightest, largest text on screen.** Chrome steps down in size
   and luminance. When in doubt, make chrome quieter.
2. **One idea per color.** Use a semantic role, never a palette literal. If a new
   state has no role, add one to `Theme`; do not reuse an unrelated one.
3. **Depth is structural.** Layer order, radius, outline and shadow always change
   together. A surface's job determines its layer, not its appearance.
4. **Keys are visible.** Every interactive surface shows the keys that operate it,
   drawn as keycaps. A mouse affordance never replaces a keyboard path.
5. **Never color alone.** State pairs color with a glyph, a word or a position
   (an error count has an icon, a modified document says "modified").
6. **Stillness by default.** Only the cursor and notifications move. Hover and
   selection changes are immediate. Nothing in the chrome schedules frames.
7. **Fit the content.** Floating surfaces size to what they show. Empty and
   loading states are written in words, not left blank.

## 3. Layers and composition

Layers from back to front. Each has exactly one surface treatment.

| Layer | Surfaces | Fill | Edge | Radius | Shadow |
|---|---|---|---|---|---|
| Wallpaper | Window background | `base`, then image and readability tint | none | 0 | none |
| Plane | Editor viewport, gutter | `editor` (transparent by default) | none | 0 | none |
| Docked chrome | Title row, explorer, status bar, command bar | `chrome` / `sidebar` (transparent by default) | 1 px `hairline` | 0 | none |
| Floating glass | Completion, documentation, hover, signature, pickers, toasts | `overlay` at `popup_opacity` | 1 px `popup_border` @ 60% | `radius::FLOAT` 6 | `Elevation::Floating` |
| Modal glass | Confirmation dialogs, Language Server Manager | `overlay` over `scrim` | 1 px `popup_border` @ 60% | `radius::MODAL` 8 | `Elevation::Modal` |
| Cursor | Cursor entity | `cursor` | none | n/a | none |

Docked regions are separated by hairlines, never by fills. Floating and modal
surfaces are built with `design::glass(theme, level)`; modal ones sit on
`design::scrim(theme, viewport)`. Centered pickers dim the editor with a lighter
scrim, at 60% of `scrim`, that has no listeners, so a click outside still dismisses.

### Shell anatomy

```text
┌──────────────┬──────────────────────────────────────────────┬──┬──┬──┐
│ ◧ PROJECT    │ ▣ main.rs  ×  ▣ lib.rs ●                     │ ─│ □│ ×│ title row: Scale::title_bar()
├──────────────┼──────────────────────────────────────────────┴──┴──┴──┤ hairline
│ › ▢ src      │  12  fn main() {                                       │
│   ▣ main.rs  │  13      let x = 1;          ← cursor line band        │ plane
│   ▣ lib.rs   │  14  }                                                  │
│ explorer     │ gutter (numbers right-aligned, ● severity marks)       │
├──────────────┴────────────────────────────────────────────────────────┤ hairline
│ NORMAL  ▣ src/main.rs modified · ⟳ Working…          ⌕ 2/9  ⊗0 ⚠1  12:5 │ status: Scale::status_bar()
│ :command input or feedback                                            │ command: code line height
└───────────────────────────────────────────────────────────────────────┘
```

- The explorer column runs from the title row to the status bar. The title row's
  leading segment is exactly as wide as the explorer, so one vertical hairline
  divides the window. With the explorer hidden, the segment collapses to the
  sidebar toggle.
- The active tab has a translucent fill and a 2 px underline in the mode color.
  Inactive tabs are text only; the close control appears on hover, and an unsaved
  document shows an accent dot that the close control replaces under the pointer.
- On Windows and Linux the title row ends in 46 px caption controls. On Windows they
  use native hit-testing (`WindowControlArea`), which provides Snap Layouts. On
  macOS the row reserves space for the system traffic lights and draws none.
- The status bar reads from left to right: mode badge, document identity
  (directory muted, name bright, "modified" in accent), transient work, then a
  flexible gap, then search position, diagnostics (only when a server is attached
  or diagnostics exist) and the cursor position.
- The command bar is exactly one code line tall because the command-line caret
  shares its geometry. Outside Command mode it shows feedback: secondary text, or
  `error` text with an error glyph.

## 4. Color

All colors are resolved once per settings change in `Theme::new`. Renderers read
roles; they never hold hex values.

### Semantic roles

| Role | Use |
|---|---|
| `text` | Primary content, the active item, titles |
| `secondary` | Supporting text, inactive tabs and rows, labels |
| `muted` | Tertiary text: meta, hints, empty states, disabled controls |
| `accent` | Keyboard focus ring, primary actions, unsaved marker, IME underline |
| `matched` | Matched characters in completion; documentation and signature signals |
| `success` / `warning` / `error` / `info` / `hint` | Outcome and severity only |
| `mode(mode)` | Normal, Insert, Visual and Command identity (see below) |
| `base` | Window background and scrim tint |
| `editor`, `chrome`, `sidebar` | Plane and docked surfaces (transparent over wallpaper) |
| `overlay` | Glass fill for floating and modal surfaces |
| `border` / `hairline` / `popup_border` | Strong border, docked separators, glass outline |
| `hover` / `pressed` | Interaction tints derived from `text` (6–12% alpha) |
| `popup_selection` | Selected row in any list |
| `selection`, `active_line`, `active_tab` | Text selection, cursor line band, active tab fill |
| `search` / `search_text` | Search-match fill and its text |
| `gutter`, `cursor`, `punctuation`, syntax tokens | Editor rendering |
| `scrim` / `shadow` | Modal dimming and elevation shadows only |
| `hue(Hue)` | Identity colors: file types and symbol kinds. Never state. |

Interaction tints are derived from the foreground, so the same tint reads correctly
over an opaque surface and over the wallpaper. The cursor line band, active tab and
list selection are translucent in every mode for the same reason.

### Mode colors

| Mode | Cyberdream | Rosé Pine Moon |
|---|---|---|
| Normal | blue | pine |
| Insert | green | foam |
| Visual | purple | iris |
| Command and others | cyan | foam |

### Themes and transparency

Cyberdream Dark is the default, with Muted and Light variants, and Rosé Pine Moon
is available as an alternative. Every component must be checked in Dark, Light and
Rosé Pine Moon, with and without `theme.transparent`.

`theme.surfaces.popup_opacity` defaults to **0.92**. Glass must stay nearly opaque
because floating surfaces cover code; at lower values the text beneath competes with
the glass content. Docked chrome stays fully transparent, so the wallpaper remains
continuous across the window. Users can still lower the value explicitly.

## 5. Typography

FilaVim uses one face: the bundled **Agave Nerd Font Mono**, regular weight only.
Hierarchy comes from size, luminance and position, never from bold or italic
chrome. (Italics are reserved for the optional `italic_comments` syntax setting.)
Everything is monospaced, including chrome, which gives the interface its
instrument-panel character and makes widths predictable.

`design::Scale::new(font_size)` derives the whole scale from the configured code
size:

| Step | Formula | At the default 18 px | Use |
|---|---|---|---|
| `code` | `font_size` | 18 | Document text, completion labels, hover and signature content, command input |
| `ui` | `round(code × 0.84)`, clamped 12–17 | 15 | Tabs, explorer, pickers, dialogs, panel titles |
| `small` | `ui − 2` (min 11) | 13 | Status bar, buttons, meta, secondary paragraphs |
| `micro` | `ui − 3` (min 10) | 12 | Keycaps, section labels, counts, timestamps |

Section labels are `micro`, `muted` and UPPERCASE (`design::section`). Paths in
chrome are shown relative to the project root, with the Windows verbatim prefix
removed (`design::display_path`, presentation only).

## 6. Spacing, sizing and radius

The spacing scale is a 2/4 px grid in `design::space`:

| Token | px | Typical use |
|---|---|---|
| `XXS` | 2 | Gap between tabs, segmented-control inset |
| `XS` | 4 | List side padding, icon–label gaps in dense rows |
| `SM` | 6 | Icon–text gap, keycap groups |
| `MD` | 8 | Row padding, toolbar gaps |
| `LG` | 12 | Panel side padding, status bar segments, dialog sections |
| `XL` | 16 | Dialog and manager padding |
| `XXL` | 24 | Margin between modals and the window edge |

Component heights scale with the type (`Scale` methods): `title_bar()` (36 px at
default), `status_bar()` (29), `row()` (26) and `control()` (27). The completion
row height follows the code size instead: `ceil(code × 1.5) + 4`.

Radii in `design::radius`: docked chrome is square, `CONTROL` 4 for buttons, rows,
tabs, badges and keycaps, `FLOAT` 6 for floating glass and `MODAL` 8 for dialogs.
Depth is legible from the corners alone.

## 7. Components

Components are free functions returning unattached GPUI elements. The caller owns
ids, listeners, focus and state, so one implementation serves the editor, popups
and the manager. Compose these components rather than restyling raw `div()`s.

| Component | Purpose and rules |
|---|---|
| `glass(theme, Elevation)` | Floating or modal surface: fill, outline, radius and shadow. |
| `scrim(theme, viewport)` | Full-window dim beneath a modal. It handles no input itself. |
| `typeface(div, theme, size)` | Root font, size and text color for an independent overlay entity. |
| `icon(glyph, size, color)` | A glyph in a square, centered slot so icons align in columns. |
| `icon_button(theme, id, glyph, size, extent)` | Chrome-only square control (sidebar toggle, close). |
| `button(theme, scale, id, label, Tone, enabled, focused)` | Text action. `Primary` marks the recommended choice (at most one per group), `Secondary` is ordinary, `Ghost` is low emphasis, and `Danger` destroys or discards data and is never the default. `focused` draws the accent focus ring. Disabled buttons keep their layout, lose hover and pointer, and get no listener. |
| `row(theme, id, selected)` | Full-width selectable list row with rounded selection and hover. Its list gets `space::XS` side padding so the selection reads as inset. |
| `badge(text, color, size)` | Tinted state pill (16% fill): the mode, server state, scope, trust. Not clickable. |
| `keycap(theme, scale, key)` and `hints(theme, scale, &[(keys, meaning)])` | Keyboard hints. Every panel footer uses `hints`. |
| `section(theme, scale, text)` | Uppercase group label. |
| `heading(theme, scale, glyph, color, title)` | Panel title with its signal glyph. |
| `divider(theme)` | Horizontal hairline. |
| `notch(theme, glyph, color, size)` | The *signal notch*, a small glyph seated on the top edge of a minimal floating surface that names its source: ⚡ completion (warning), ⓘ documentation or hover (matched), a symbol glyph for signature help (matched), ⚠ diagnostics. |
| `severity(theme, n)` | LSP severity → (glyph, color). |
| `shadow(theme, Elevation)` | Elevation shadows; only `glass` should need it. |

### Composed patterns

- **Minimal float** (completion, hover, signature, documentation): `floating::frame`
  plus a notch. It has no header or footer and is positioned at the caret by
  `popup::place` and `popup::assistance`. Its height fits the content: the row count
  for lists, and a monospace wrap estimate for text, capped at 320 px. Below the
  caret they shrink-wrap, using the estimate only as a ceiling.
- **Documentation text** (loating::documentation): line-level structure for
  what servers send (ATX headings in 	ext, --- as a hairline, bullets with a
  muted dot, fenced and inline code in 	ext, prose in secondary). A
  function-like first line keeps signature coloring. It is not a Markdown engine.
- **Diagnostics float:** each item is a top-aligned severity glyph, the message in
  	ext and a micro muted server · line:col line.
- **Picker / centered panel** (commands, files, grep, buffers, symbols, info): a
  frame, a header (`heading` plus an `n / total` count), a divider, a list of `row`s
  with leading icons, a divider, then a `hints` footer. It sits over a light scrim.
  An empty panel says "Nothing to show yet."
- **List row content:** leading `icon` (file type, symbol kind or muted chevron for
  commands), a label that ellipsizes, then optional right-aligned `muted` meta (the
  completion kind). Matched characters use `matched`.
- **Dialog** (`confirmation_dialog`): a scrim, then modal glass up to 600 px wide
  that fits its content. It contains a heading with the decision glyph (warning for
  data risk, error for destructive overwrite, history for recovery), a subject line
  (file icon plus a relative path), a secondary explanation, an optional well of
  details, a divider, and a footer with key hints on the left and toned buttons on
  the right. Choices keep their order and indices; arrow keys move the focus ring.
- **Manager** (Language Server Manager): modal glass with a header (identity plus
  scope and trust badges and a close control), a toolbar (a search field drawn as an
  input with an accent caret, a segmented filter control showing F-keys, ghost
  refresh actions), a maintenance row, a catalog | details split, then the operation,
  error and confirmation banners and a shortcut strip. Details use
  `section`-labelled groups and a key/value grid.
- **Banner:** a tinted fill (6–8%) and a 30–60% border in the severity color, with
  a glyph and text. Used for warnings, errors and in-panel confirmations, and for the
  missing-tools notice above the status bar.
- **Toast:** glass card with a category glyph, the title in `text`, a `micro`
  timestamp and a close icon button, the message in `secondary` `small`, and the
  remaining lifetime as a 2 px bar on the bottom edge.

## 8. Iconography

Glyphs come from the bundled Nerd Font, so there are no image assets, icon fonts or
new dependencies. Every glyph is named in `icons.rs` and verified present in
`assets/fonts/AgaveNerdFontMono-Regular.ttf`. Never write a codepoint literal at a
call site.

- **Chrome actions and states** use Codicons: close, minimize, maximize and restore,
  sidebar, chevrons, folder, file, error, warning, info, hint, check, zap, search,
  server, sync, history, edit, terminal, package, signature and dot.
- **File types** use Seti glyphs through `icons::file(name) -> (glyph, Option<Hue>)`.
  Unknown types fall back to a neutral glyph in `secondary`. Inactive tabs show
  their hue at 70% opacity.
- **Completion kinds** use symbol glyphs through `icons::completion_kind(n)`,
  colored by `theme.completion_kind(n)`.
- Icons accompany words; they do not replace them. Folder chevrons and command
  chevrons are `muted`. Icons that report state take that state's semantic color.
- Before adding a glyph, verify it is in the font (for example with fontTools'
  `getBestCmap()`), add a named constant, and document its meaning there.

## 9. Interaction and focus states

| State | Treatment |
|---|---|
| Hover | `hover` fill; text brightens to `text`. Never on the selected row. |
| Pressed or active control | `pressed` fill |
| Selected list item | `popup_selection` fill, `text` color |
| Keyboard focus (buttons, dialog choices) | 1 px `accent` ring |
| Disabled | `muted` text, faint outline, no hover, pointer or listener |
| Active document | Translucent tab fill and mode-colored underline; open file highlighted in the explorer |
| Unsaved | `accent` dot on the tab, "modified" in the status bar |
| Focus region | The explorer's project label turns `accent` while the explorer has keyboard focus |
| Working | `sync` glyph with muted text in the status bar or manager progress row |
| Loading or empty | Muted glyph and a sentence ("Indexing project…", "No files to show", "Nothing to show yet.") |
| Error | `error` glyph and text; detailed errors stay in their owning panel |

Clicks on a float stop at the float. Clicks outside a non-modal popup dismiss it.
Modals block all input beneath them. Keyboard routing is unchanged by styling: popup
policy is in `popup::route`, and the view owns focus.

## 10. Motion

Motion is reserved for two things, both owned by dedicated entities so they never
rebuild the editor:

- **Cursor.** It uses a four-corner, time-based simulation (`cursor.rs` and
  `cursor_view.rs`) configured by the `cursor.*` settings. Logical edits always come
  first; animation is presentation only.
- **Toasts.** A 180 ms entrance slide and fade and a 220 ms exit, with lifetime
  progress drawn as an edge bar. One owned timer runs at about 60 Hz only while
  something moves, and stops otherwise.

`cursor.reduced_motion = true` removes the toast slide and fade (lifetime progress
remains, at about 30 Hz) and reduces cursor animation according to the cursor
settings. Hover, selection, popups, dialogs and panels appear and disappear
immediately. New interfaces must not add transitions, spinners, pulsing or idle
timers. To show that work is in progress, use the static `sync` glyph and words.

## 11. Accessibility

- Text sizes scale with the user's `font_size`, and chrome never drops below 10 px.
- Text opacity is independent of surface opacity: text is always drawn at full
  alpha, even on translucent surfaces.
- State never depends on color alone (see principle 5). Severity, outcome and mode
  always have a glyph or a word.
- Every action has a keyboard path, and panels show it. Dialogs keep the existing
  default choice, and a destructive choice is never `Primary`.
- Glass at 0.92 keeps floating text legible over arbitrary wallpapers. Contrast over
  the plane still depends on the user's image and `background.overlay_opacity`.
- Reduced motion is honored as described above.
- Known gaps: there are no tooltips on icon-only controls, no screen-reader bridge
  in GPUI 0.2.2, and IME, DPI and high-contrast behavior need manual checks.

## 12. Building new interfaces

Checklist:

1. **Pick the layer first.** Is the new surface docked, floating or modal? That
   decides its fill, edge, radius and shadow. Do not invent a fifth layer.
2. **Compose from `design::`.** Use `glass`, `row`, `button` with an explicit `Tone`,
   `badge`, `hints`, `section` and `heading`. If you need a new building block, add
   it to `design.rs` with a `///` contract and a row in section 7.
3. **Use roles and tokens only.** No hex values and no ad-hoc pixel values where a
   `space`, `radius` or `Scale` step exists.
4. **Write every state.** Default, hover, selected, focused, disabled, loading,
   empty and error.
5. **Show the keys.** Add a `hints` footer or keycaps.
6. **Check all themes**: Dark, Light and Rosé Pine Moon, with and without
   transparency and wallpaper, at 14 px and 22 px code sizes.
7. **Stay still and bounded.** No timers, no per-frame shaping, no unbounded lists:
   use `uniform_list` for long collections.

### Guidance for planned features

These notes say how upcoming milestones should fit the system. They describe
intended mappings, not implemented features.

- **Editor splits.** Each pane is its own plane with its own gutter and is divided
  from its neighbours by a `hairline`. Only the focused pane shows the mode-colored
  current line number and cursor line band; unfocused panes use `gutter` numbers.
  Tabs belong to the focused pane group.
- **Integrated terminal panel.** A docked layer below the plane, separated by a
  hairline, with code-size text, a header row at `Scale::row()` and a `terminal`
  glyph. It is not glass.
- **Session restoration and recent projects.** These use the picker pattern, with
  the folder or history glyph and `display_path` meta; startup offers use the dialog
  pattern.
- **Advanced explorer operations.** Inline rename and new file use the search-field
  treatment as an input row. Destructive operations use the dialog with a `Danger`
  button and a non-destructive default.
- **Project-wide find and replace.** Use the picker anatomy with results grouped
  under `section` file headers, matched text in `matched` and replacements previewed
  with `success` and `error` tints at 12% or less.
- **Folding and inlay hints.** Fold chevrons are `muted` gutter glyphs. Inlay hints
  are `muted`, at `small` size, on a `hover` pill, and are never brighter than code.
- **Git diff views and hunks.** Gutter bars 3 px wide in `success`, `warning` and
  `error` for added, changed and removed lines. Line backgrounds are tints at 12% or
  less. Hunk actions are `Ghost` buttons.
- **Configurable keybindings.** Render bindings with `keycap`. A keybinding editor
  is a manager-style modal.

## 13. Where things live

| Concern | File |
|---|---|
| Tokens, scale, elevation, components | `src/gui/design.rs` |
| Glyph names, file-type and kind mappings | `src/gui/icons.rs` |
| Semantic roles, hues, mode colors | `src/gui/theme.rs` (palettes in `src/palette.rs`) |
| Appearance settings and defaults | `src/appearance.rs` |
| Shell: title row, tabs, explorer, status and command bars | `EditorView` shell section in `src/gui/view.rs` |
| Popups, pickers, dialogs | `popup_row`, `popups` and `confirmation_dialog` in `src/gui/view.rs`; placement in `src/gui/popup.rs`; float frame and text styling in `src/gui/floating.rs` |
| Language Server Manager | `src/gui/manager_view.rs` |
| Toasts | `src/gui/toast_view.rs` (lifetimes in `toast.rs`) |
| Cursor | `src/gui/cursor.rs`, `src/gui/cursor_view.rs` |
| Wallpaper | `src/gui/wallpaper.rs` |
