# FilaVim

FilaVim is a native modal code editor built with Rust, GPUI, and Ropey.
Development takes place on the `main` branch.

For development, start with [AGENTS.md](AGENTS.md) and the active source/ownership
map in [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md). Verification commands and
coverage boundaries are in [TESTING.md](TESTING.md).

Transient application events use transparent top-right toasts with hover-paused
lifetime lines; command feedback stays in the command bar. See the
[notification contract](docs/ARCHITECTURE.md#toast-notification-contract) for routing,
durations, reduced motion and manual checks.

```powershell
cargo run --locked -- path\to\file.rs
```

Or open `target/debug/filavim.exe` directly after `cargo build --locked`.
Windows release builds need the SDK shader compiler; use
`./scripts/windows-build.ps1 -Release` (see the checkpoint guide).

W/A/S/D move, I inserts, Esc returns to Normal, V selects, U undoes,
Ctrl-R redoes. Existing word motions, registers, operators and command input
are reused. Ctrl-O opens, Ctrl-S saves, Ctrl-Shift-S saves as. Use `:e path`,
`:w [path]`, `:wq`, `:q`, `:q!`, `:new`, `/search`, and line-number commands.
Opening another file preserves unsaved documents in independent tabs. Document
and window closure offer Save/Discard/Cancel. Ctrl-Tab switches documents; Ctrl-W
closes one. Ctrl-Shift-O opens a project and Ctrl-E focuses its explorer.

The viewport uses embedded **Agave Nerd Font Mono**, native text shaping,
line numbers, mouse positioning/Visual selection, two-axis scrolling and a
native block/bar cursor with optional Neovide-inspired animated cursor smear. The default theme is Cyberdream Dark; Muted, Light and Rosé Pine Moon remain selectable.

FilaVim includes document tabs, an ignore-aware explorer, background syntax
highlighting, project search, language intelligence, and Git commands.
`:Commands` lists available commands. Try `:Files query`, `:Grep regex`,
`:Diagnostics`, `:LspInfo`, or `:GitStatus`.

Splits, recent-file history, snippet browsing, and Git hunk controls remain
outstanding. Font provenance and licenses are in [assets/fonts](assets/fonts/README.md).
Historical checkpoints [1](GPUI_CHECKPOINT1.md), [2](GPUI_CHECKPOINT2.md), and
[3–4](GPUI_CHECKPOINTS_3_4.md) record earlier development and validation.
The retired terminal edition is preserved in Git history at `dbcfd3a`.

LSP completion uses a caret-anchored floating menu: type in Insert mode or press
Ctrl-Space; use arrows/Ctrl-N/P, Enter/Tab, and Escape. Documentation appears beside
it, or press F1 for a documentation view in narrow windows. `:Hover`, `:Diagnostics`
(current line), and `:DiagnosticsAll` expose floating information/navigation.
`:Commands`, `:Files query`, `:Buffers`, and `:References` open floating pickers.
Configure `[completion]` with `auto_trigger`, `accept_enter`, and `accept_tab`.
Real rust-analyzer completion/hover/diagnostics are protocol- and native-prepaint-tested;
manual interaction and appearance checks remain necessary. See the checkpoint report.


The cursor defaults to smooth movement with a single elastic smear shape.
There are no particle effects. Use `:set cursor.smear=false` for rigid smooth movement.
Use `:set cursor.enabled=false` for an immediate static cursor, or configure
`[cursor]` in `%APPDATA%/filavim-gpui/config.toml`. See
[GPUI_CHECKPOINTS_3_4.md](GPUI_CHECKPOINTS_3_4.md) for all settings, measurements,
implementation limits and the numbered manual walkthrough.


### Native language tools

Use `:LspManager` for native installation, server preferences and workspace process controls.
See [Language intelligence and managed servers](docs/LANGUAGE_INTELLIGENCE.md) for the catalog, controls, and validation limits.

Completion now uses persistent, locally filtered sessions rather than a typing-pause timer.
Language-aware auto-pairs use owned-closer provenance and bypass paste/IME input.
See [COMPLETION_AND_PAIRS.md](COMPLETION_AND_PAIRS.md) for settings, measurements,
validation, limitations, and manual checks.


See [CYBERDREAM.md](CYBERDREAM.md) for official palettes, live theme settings, native cached wallpapers, validation and manual checks.

See [docs/TESTING.md](docs/TESTING.md) for contributor commands (Fast, Full, LSP, Performance), safe fixtures, deterministic asynchronous tests and headless GPUI coverage.
