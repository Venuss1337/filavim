# Font setup

> Historical terminal-edition reference. See the [FilaVim README](README.md) for the current native application.

`:SetupFont` opens an inspect-only setup panel. Up/Down selects an action,
PageUp/PageDown scrolls the description, Enter requests confirmation, and Esc
cancels. Type `apply` to install/configure, or `remove` for removal. Filesystem,
registry, discovery, and cache operations run on the existing setup worker.
No font operations or network requests run during normal startup or rendering.

`:FontStatus` reports installation, bundle availability, terminal identity,
configured font when identifiable, and sample Nerd Font cmap coverage.
`:RemoveFont` confirms removal of **only** a Filavim-owned installation whose
SHA-256 still matches. It preserves terminal settings and configuration backups,
including settings changed by the user since setup. Choose another font in your
terminal before removing a font currently in use. On Windows, close applications
using a locked font and retry removal if necessary.

## Private development asset

Operator Mono is proprietary. Availability on GitHub is not permission to embed
or redistribute it. This feature is a private experiment, not a release asset.
Do not publish development executables, target directories, or compiler caches.
Public builds/CI must use the default features (optionally `perf`), **never**
`--all-features`. The build script rejects the development feature in common CI
environments and with compiler-wrapper environment variables set. Do not disable
these guards in a public build. Local build outputs remain under ignored `target/`.

Download the original file into the ignored `.local/fonts/` directory:

```powershell
New-Item -ItemType Directory -Force .local/fonts
Invoke-WebRequest 'https://raw.githubusercontent.com/ajaybhatia/OperatorMonoLig-Fonts/master/Operator%20Mono%20Medium%20Nerd%20Font%20Complete.otf' -OutFile '.local/fonts/Operator Mono Medium Nerd Font Complete.otf'
cargo build --release --features dev-operator-font
```

The feature embeds the bytes directly with `include_bytes!`. No extraction is
needed until installation is explicitly confirmed. Missing/invalid OTF headers
produce actionable build diagnostics; full metadata and outline parsing is
performed before installation. Normal `cargo build --release` works without the
asset. `.gitignore` and Cargo's package exclusions omit `.local/`. Do not add this
font to version control, release uploads, public CI, or shared build caches.

Verified source asset:

| Field | Value |
|---|---|
| Container | OpenType / CFF (`OTTO`), 927,952 bytes |
| Typographic family used for configuration | `OperatorMono Nerd Font` |
| Legacy family | `Operator Mono Medium` |
| Style | `Medium` |
| OS/2 weight | `350` (preserved as recorded; not inferred from the filename) |
| Full name | `Operator Mono Medium Nerd Font Complete` |
| PostScript name | `OperatorMonoNerdFontComplete - Medium` |
| SHA-256 | `8b94c47706d0cdaa5453c11d15e932caa876c25766777ddda5375590f9ba8bde` |

Sample glyphs E0B0, E5FF, F120, and F07B exist in its cmap. This does not prove
host-terminal rendering, fallback selection, or complete coverage of every Nerd
Font version. No second Nerd Font installation is required. Filavim's existing
Unicode/ASCII fallbacks are unchanged.

The provider (`bundled`, reference identity, and display label in `src/font`) is
independent of the installer/terminal editor. A licensed redistributable font can
replace it without changing platform registration or settings transactions.

## Platform and terminal support

| Platform | Installation |
|---|---|
| Windows | `%LOCALAPPDATA%/Microsoft/Windows/Fonts`, HKCU persistent registration, GDI loading, bounded WM_FONTCHANGE notification. No elevation. |
| Linux | `$XDG_DATA_HOME/fonts/filavim` or `~/.local/share/fonts/filavim`; `fc-cache -f` when available. |
| macOS | `~/Library/Fonts`; restart applications that cache font discovery. |

Ownership/change journals live in `%LOCALAPPDATA%/filavim/font-setup` on Windows,
`$XDG_STATE_HOME/filavim/font-setup` (default `~/.local/state/filavim/font-setup`)
on Linux, and `~/Library/Application Support/filavim/font-setup` on macOS.
Existing matching font files are not adopted, overwritten, or removed. Windows
also checks named HKCU/HKLM registrations, including registrations outside the
standard font directories. Discovery is limited to ordinary OTF/TTF files in
standard directories (collections/custom Linux fontconfig locations are not
exhaustively searched); inability to inspect a directory is an error, not a
claim that the font is absent.

Windows Terminal automatic configuration supports stable, preview, and
unpackaged settings locations. `WT_PROFILE_ID` selects the current profile when
it is present in the file. Otherwise the user selects a profile or profile
defaults. If several editions have settings files, their full paths are offered;
Filavim does not guess which edition is rendering the session. Portable/custom
settings and legacy pre-1.10 schemas require manual setup. Runtime command-line
appearance overrides cannot be detected: status reports **configured** font,
not a claim about the font currently rendered by the terminal.

The JSONC editor preserves comments, trailing commas, unrelated settings, font
size and other appearance preferences. Ambiguous duplicate keys, malformed
documents, symlink files, readonly files and unexpected schema shapes are
rejected. Before modifying `font.face`, it writes a durable original backup and
change journal, compares the current file against its snapshot, atomically
replaces the file, and verifies the result. On a publication failure it rolls
back only if the destination still contains its own exact output. Concurrent
edits/unknown contents are preserved; the original backup path is reported.
The compare/write window cannot exclude a simultaneous write by an unrelated
program; avoid editing Terminal Settings while confirming setup.

WezTerm, Kitty, Alacritty, remote/multiplexed and other terminals receive precise
manual instructions in the panel. Their executable configuration, includes,
imports and remote instances are not rewritten. Detection uses dedicated
environment signals, never `TERM` alone. Reopen a tab/restart the terminal if a
new font is not discovered. Installation and terminal configuration are reported
separately, including partial success when installation succeeds but configuration
fails.

## Automated verification

Tests use an original synthetic font, temporary directories, injected registry
operations and configuration I/O faults. They do not install fonts, change actual
terminal settings, or launch a live editor. The development feature additionally
verifies the exact private font metadata and representative cmap coverage.
Linux/macOS native behavior and actual terminal rendering require manual host
verification; no visual verification is claimed by the automated tests.

Run default tests with `cargo test --all-targets`. On a private local machine
with the asset, run `cargo test --features dev-operator-font --all-targets`.
