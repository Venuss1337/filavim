# Themes, glass and wallpaper

FilaVim's visual system is **Lucent**, described in [DESIGN.md](DESIGN.md). This
guide covers the user-facing settings: palettes, glass qualities and the world
image (wallpaper or the procedural atmosphere), plus their lifecycle and limits.

## Palettes and sources

- **Lucent** (default): FilaVim's own palette, Dark, Muted and Light variants,
  defined in `src/palette.rs`. Pastel, evenly bright syntax hues on luminous glass.
- **Cyberdream**: roles follow
  [Cyberdream](https://github.com/scottmckendry/cyberdream.nvim)
  (`lua/cyberdream/colors.lua`, `extensions/base.lua`, `treesitter.lua`,
  `blinkcmp.lua`, `cmp.lua`, `telescope.lua`, `util.lua`). Upstream's MIT notice is
  retained in `assets/licenses/cyberdream-MIT.txt` (Scott McKendry). Dark, Muted and
  Light variants.
- **Rosé Pine Moon**: the official 15-color palette; legacy `[theme]` Moon color
  overrides remain readable.

Every palette gets the same Lucent materials, composition and light: glass tints,
rims and shadows are derived from each palette's own background colors. No
wallpaper artwork is bundled; the application accepts a user-supplied local file,
and draws a procedural atmosphere when none is configured.

## Configuration

The configuration file is `%APPDATA%/filavim-gpui/config.toml` on Windows, or the
`FILAVIM_GPUI_CONFIG` override. `theme.name` takes precedence over the legacy
`theme_name` selector.

```toml
[theme]
name = "lucent"             # lucent | cyberdream | rose-pine-moon
variant = "dark"            # dark | muted | light
glass = "frosted"           # frosted | clear | solid
transparent = true          # false: solid glass and a plain world
saturation = 1.0            # 0..1, HSL desaturation of palette colors
italic_comments = false

[theme.surfaces]
popup_opacity = 0.92        # floating glass; text stays opaque
editor_opacity = 0.0        # minimum pane glass density (0..1)
sidebar_opacity = 0.0       # same; the larger of the two applies

[background]
enabled = true              # false: no world image, plain night
image = "C:/Pictures/wallpaper.webp"   # unset: procedural atmosphere
fit = "cover"               # cover | contain | stretch
opacity = 1.0               # image over the night color
overlay_opacity = 0.15      # tint over the whole world
```

All opacity/saturation values must be finite and within 0..1. Invalid enum values,
unknown options and malformed values fail validation before anything is written.
Text is always fully opaque.

Glass qualities:

- **frosted** (default): panes show the world blurred, tinted and grained.
- **clear**: panes show the sharp world under a slightly denser tint.
- **solid**: panes are opaque gradients; the world shows only around them.

Contrast does not depend on the artwork: the frosted/clear bake clamps luminance
under glass (dark glass caps brightness, light glass lifts darkness) while keeping
the world's hue. `overlay_opacity` only shapes how vivid the world is around the
panes. Floating glass stays near-opaque at the default `popup_opacity`.

Examples using the command lens:

```text
:set theme.name=lucent
:set theme.variant=light
:set theme.glass=clear
:set theme.name=cyberdream
:set background.image=C:/Pictures/wallpaper.png
:set background.fit=contain
:set background.overlay_opacity=0.3
:set background.enabled=false
:BackgroundReload
```

Relative image paths resolve against the configuration directory, independently of
project changes. `:BackgroundReload` re-reads the same file after an external edit.
There is no filesystem polling or image watching. Theme and background settings
apply live without restarting language servers.

## Image lifecycle and performance

Layer order: night color → world image (fitted, static) → world tint → pane
shadows → pane materials (baked frosted textures or gradients) → rims and sheens
→ editor content → floating glass → caret. The world never scrolls with content.

The stage (`src/gui/stage.rs`) is a cached root view beneath the editor. Only an
enabled/path/glass/transparency change or `:BackgroundReload` reloads the world.
Decoding, the reduced world copy (480 px blurred for frosted glass, 1280 px sharp
for clear glass) and every pane bake run on GPUI's background executor; a
superseded result cannot publish over a newer generation. Pane bakes are keyed by
geometry, scale and theme, debounced 90 ms during live resize, and replaced
textures are released from the sprite atlas. Animated containers render a single
static frame. Nothing here runs during typing or at rest.

Decode limits: a regular local file, at most 64 MiB compressed, 32 megapixels,
16,384 pixels per dimension and a 128 MiB decoder allocation limit (best-effort in
the image library; dimension limits are strict). Missing, corrupt, unsupported
and oversized files fall back to the plain world and raise an error toast. Bakes
are capped at 4096 device pixels per side. See DESIGN.md §12 for the memory model.

## Mappings and native adaptations

`src/gui/theme.rs` resolves semantic roles once per settings change: four ink
levels, the signal (`accent`), mode lights, severities, materials and syntax
tokens. Rendering never uses hex literals. Search matches keep their syntax colors
under a tint; the current match is filled more strongly and outlined, so navigation
never reshapes text. Signature help emphasizes the active parameter, converting LSP
UTF-16 label offsets to validated UTF-8 ranges. Existing Tree-sitter classification
(booleans, operators, statements, preprocessor constructs, parameters, properties)
is unchanged; there is no LSP semantic-token pipeline.

## Manual checks

1. Open a Rust or TypeScript file. Check syntax roles, Normal/Insert/Visual/Command
   mode lights (mark, caret, line number, chip, rim tint), save and undo/redo.
2. Set `background.image` to a bright photo. Code and comments must stay legible
   through frosted and clear glass; the world stays vivid around the panes.
3. Switch `background.fit` between cover/contain/stretch; resize and maximize. The
   pane material re-bakes once after the resize settles.
4. Switch `theme.variant` through dark/muted/light, `theme.glass` through
   frosted/clear/solid, and the palette to cyberdream and rose-pine-moon and back.
5. Point at a missing or corrupt image: expect an error toast and a plain world;
   editing stays responsive. Unset the image to see the atmosphere.
