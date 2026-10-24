# Filavim GPUI — Checkpoint 1

A native Rust/GPUI editor using Filavim's existing Ropey editing engine.
This branch replaces the terminal frontend. It does not embed a terminal,
WebView, browser, or backend server, and has no cursor effects.

```powershell
cd C:\Users\filet\Desktop\filavim-gpui
cargo run --locked -- path\to\file.rs
```

Or open `target/debug/filavim.exe` directly after `cargo build --locked`.
Windows release builds need the SDK shader compiler; use
`./scripts/windows-build.ps1 -Release` (see the checkpoint guide).

W/A/S/D move, I inserts, Esc returns to Normal, V selects, U undoes,
Ctrl-R redoes. Existing word motions, registers, operators and command input
are reused. Ctrl-O opens, Ctrl-S saves, Ctrl-Shift-S saves as. Use `:e path`,
`:w [path]`, `:wq`, `:q`, `:q!`, `:new`, `/search`, and line-number commands.
Unsaved changes block ordinary open/quit; window close offers Save/Discard/Cancel.

The viewport uses embedded **Agave Nerd Font Mono**, native text shaping,
line numbers, mouse positioning/Visual selection, two-axis scrolling and a
simple nonanimated cursor. The initial theme is Rosé Pine Moon.

See [GPUI_CHECKPOINT1.md](GPUI_CHECKPOINT1.md) for architecture, build/tooling,
verification, performance results and remaining manual checks. Font provenance
and licenses are in [assets/fonts](assets/fonts/README.md).

The original terminal editor remains at `../filavim` on `master`, baseline
`dbcfd3a`. This worktree is on `gpui-migration`. Historical terminal source and
reports remain in this branch for reference; they are not part of the native
build. This checkpoint exposes one document at a time and intentionally defers
LSP/Git UI, project panels, splits/tabs, syntax highlighting and effects.
