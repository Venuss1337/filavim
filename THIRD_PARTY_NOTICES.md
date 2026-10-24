# Third-party notices and cursor port

Filavim's cursor effect contains a native Rust adaptation of
[smear-cursor.nvim](https://github.com/sphamba/smear-cursor.nvim), by
**Son Pham-Ba (sphamba) and contributors**, licensed under the GNU General Public
License, version 3. The upstream license is reproduced without modification in
[LICENSES/smear-cursor.nvim-GPL-3.0.txt](LICENSES/smear-cursor.nvim-GPL-3.0.txt).
Filavim is distributed under GPL-3.0-only; see [LICENSE](LICENSE).

Source inspected and adapted: commit
`f6cb57c342268177f719acc3968b9d8de3ecc969` (2026-10-22 checkout).

Derivative code lives in `src/smear.rs`: the four-corner spring integrator,
distance-dependent stiffness, anticipation, damping correction, maximum length,
perpendicular volume reduction, head/tail gradient, edge classification,
fractional-block and quadrant coverage, legacy diagonal symbol tables, and
16-level gamma-adjusted color interpolation are adapted from upstream
`lua/smear_cursor/{animation,draw,color,config}.lua`. Default constants in
`src/config.rs` also derive from upstream. Rust adaptation and integration were
made for Filavim on 2026-10-22. No Lua code executes at runtime.

Adaptations: Normal-only navigation; immediate cancellation on mode/focus changes;
Ratatui cell overlays instead of Neovim floating windows/highlight groups;
clipping to the active text viewport; buffer-space scroll adjustment; Rosé Pine
Moon Iris as default ink; configurable gradient length/intensity; fixed reference
timestep with fractional display sampling; terminal color/glyph fallbacks. The reference's
particle effects, bar-cursor modes, Neovim event workarounds, and cross-buffer
smearing are deliberately excluded. Default rendering uses standard fractional
blocks and quadrants, as upstream does without legacy-symbol support; legacy
diagonal symbols are opt-in.

## Distribution implications

Personal use does not require publishing source. Distributing Filavim binaries
with this derivative implementation requires complying with GPLv3 for the
combined application, including providing Corresponding Source, the license,
and retained attribution/notices. A future commercial release may use GPLv3,
but cannot treat this port as permissively licensed or closed-source code.
Changing that requires an independently implemented replacement or separate
permission from the relevant upstream copyright holders. Review distribution
and dependency obligations before packaging releases.

Other Cargo dependencies retain their respective licenses; this notice does
not replace their license texts or attribution requirements.

## Neovide Pixiedust

The particle trajectory emission, downward-biased randomized velocity, curl,
distance-based density, lifetime variation, and linear fading in
`src/pixiedust.rs` adapt Neovide's `ParticleTrail` / `TrailMode::PixieDust` in
`src/renderer/cursor_renderer/cursor_vfx.rs` at commit
`4caa65a968ce03624978a45c3605424a56f26d90`, inspected on 2026-10-24.
Copyright (c) 2023 Neovide Contributors, MIT licensed. The complete upstream
license is retained in [LICENSES/neovide-MIT.txt](LICENSES/neovide-MIT.txt).
Filavim adds bounded emission/storage, stratified interpolation, exact curl
integration, jump/stall cancellation, sprite-atlas rendering and terminal
capability validation. No Skia/GUI dependency is used.

## Native checkpoint assets and framework

The GPUI frontend uses the official `gpui` 0.2.2 crate under Apache-2.0;
its license is retained in `LICENSES/GPUI-Apache-2.0.txt`. Cargo dependencies
retain their respective copyright/license terms.

The bundled, unmodified Agave Nerd Font Mono regular face is redistributable
under SIL OFL 1.1. Font copyright, license, Nerd Fonts notices, source revision
and SHA-256 are retained in `assets/fonts/`. This font is embedded only for
application text rendering, with no system installation. Its font license
remains distinct from the application's GPL-3.0-only license.

Historical smear/Neovide notices above apply to their retained source files;
those modules are not compiled into the native Checkpoint 1 application.
