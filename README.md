# Filavim

Complete references: [default keymap](KEYMAP.md) / [commands and aliases](COMMANDS.md) / [native IDE guide](STAGE3.md).

Cursor effects support `auto`, `pixiedust`, `smear`, and `off`. See the [Pixiedust guide](PIXIEDUST.md) for protocol checks, configuration, measurements, and terminal compatibility limits.

A native Rust terminal editor with WASD modal controls and a Rose Pine Moon interface. Stages 1/2 provide editing, Tree-sitter, project search, buffers, splits, tabs and cursor effects. Stage 3 adds a Rust LSP client, completion/diagnostics/navigation/refactoring, asynchronous Git integration, real PTY terminals, snippets, and persistent settings/setup panels. Language servers and Git are optional external executables; no scripting runtime is embedded. See [STAGE3.md](STAGE3.md) for configuration, supported installers, limitations and the manual checklist.

## Build and run

Use Rust stable and your platform's C/C++ build tools (Tree-sitter grammars compile C). Windows supports Rust MSVC with Visual Studio C++ Build Tools, or a configured Rust GNU toolchain.

```sh
cargo build --release --locked
cargo run --release
cargo run --release -- src/main.rs
cargo run --release -- src/main.rs src/lib.rs
cargo run --release -- path/to/project
```

The executable is `target/release/filavim` on Linux/macOS and `target/release/filavim.exe` on Windows. The same executable provides the CLI and editor; no launcher or extra dependency is required.

## Install and launch from any terminal

From the repository, use Rust stable and the build tools above:

```sh
cargo install --path . --locked
filavim --version
filavim --help
```

Cargo installs the executable as `filavim` (`filavim.exe` on Windows) into its installation root's `bin` directory, normally `~/.cargo/bin`. `CARGO_HOME`, `CARGO_INSTALL_ROOT`, Cargo's `install.root`, and `cargo install --root` can change that location; use the corresponding `bin` directory in PATH. See [Cargo's installation reference](https://doc.rust-lang.org/cargo/commands/cargo-install.html).

### Windows PowerShell

Rustup normally adds Cargo's bin directory to PATH. If `filavim` is not found, add it to this session and your persistent **user** PATH:

```powershell
$cargoDirectory = if ($env:CARGO_HOME) { $env:CARGO_HOME } else { Join-Path $env:USERPROFILE '.cargo' }
$cargoBin = Join-Path $cargoDirectory 'bin'
$env:Path = "$cargoBin;$env:Path"
$userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
if ($cargoBin -notin ($userPath -split ';')) {
    [Environment]::SetEnvironmentVariable('Path', "$cargoBin;$userPath", 'User')
}
filavim .
```

Open a new terminal for the persistent change. If using a custom installation root, set `$cargoBin` to its `bin` directory instead. To run without installing:

```powershell
cargo build --release
.\target\release\filavim.exe .
.\target\release\filavim.exe 'C:\Projects\my project\src\main.rs'
```

### Linux and macOS

If needed, add Cargo's bin directory to the current shell:

```sh
export PATH="${CARGO_HOME:-$HOME/.cargo}/bin:$PATH"
filavim .
```

Add that export to `~/.bashrc` or `~/.zshrc` for persistence, or the corresponding PATH setting for your shell. For a custom installation root, use its `bin` directory. To run without installing:

```sh
cargo build --release
./target/release/filavim .
./target/release/filavim './my project/src/main.rs'
```

### Launch behavior

| Invocation | Result |
| --- | --- |
| `filavim` | Dashboard using exactly the current working directory |
| `filavim .` | Current directory as project root, with explorer open |
| `filavim ./my-project` | Explicit project root, with explorer open |
| `filavim main.rs` / `filavim ./src/main.rs` | Open the file, or an empty named buffer if missing |
| `filavim file1.rs file2.rs` | Separate buffers, with the first focused |
| `filavim ./my-project ./src/main.rs` | Explicit project root; file path still relative to the launch directory |
| `filavim -- -draft.rs` | Open a filename starting with a dash |
| `filavim --help` / `filavim --version` | Print output and exit before loading config or entering the TUI |

Absolute paths, native Windows paths, spaces (quoted by your shell), and Unicode filenames are supported. Existing symlinks are resolved. Repeated file paths share one buffer. At most one distinct directory may be selected; conflicting directories, inaccessible paths, nonregular files, and invalid UTF-8 contents produce errors before terminal initialization. A missing file's parent must already exist and be accessible; opening never creates project files or directories. The file is created only when you save with `:w`.

For file launches without an explicit directory, Filavim discovers the nearest project upward from the first file's parent using `.git`, `Cargo.toml`, `package.json`, `pyproject.toml`, `go.mod`, or `.filavim`, falling back to the launch directory when none is found. An explicit directory always wins. The shared workspace root initializes the explorer, file/content search, Git, LSP and new integrated terminals. Recent files/projects use the existing history system; project launching does not change the process-wide working directory.

`:cd <path>` changes the workspace root used by navigation and relative editor paths; existing open buffers retain their absolute paths and unsaved text. `:pwd` shows the current root.

Use a modern terminal for true color and Unicode. **Fira Code** is recommended; **MonoLisa** is an alternative for users who own it. Set the font in your terminal. Fonts are downloaded/installed only after explicit confirmation in :Setup; proprietary fonts are never distributed. Standard Unicode indicators require no Nerd Font, and patched fonts also work. `unicode = false` supplies ASCII borders and a dashboard logo. `[visual] colors = 'ansi'` or `'indexed'` provides explicit color fallbacks; `auto` checks terminal environment hints. Emoji width and appearance still depend on the terminal and font.

`:SetupFont` inspects an optional private development font and offers confirmed per-user installation and Windows Terminal configuration. `:FontStatus` reports status; `:RemoveFont` removes only an unchanged Filavim-managed installation after confirmation. Ordinary builds contain no proprietary font. See [Font setup](FONT_SETUP.md) for supported platforms, terminal configuration, private asset/build instructions, and limitations.

## Project navigation

Letters mean unshifted presses unless Shift is specified. Leader sequences have no timeout; `Esc` cancels a pending sequence. The following defaults apply in Normal and Visual modes:

| Shortcut | Action |
| --- | --- |
| `Space f f` | Find files with fuzzy matching |
| `Space f g` | Live project-wide regex grep |
| `Space f r` | Recent files |
| `Space f p` | Recent projects / switch workspace |
| `Space b b` | Search open buffers, including modified buffers |
| `Space p` | Fuzzy command palette, with descriptions and shortcuts |
| `Space e` | Toggle and focus file explorer |
| `Space b n` / `Space b p` | Next / previous buffer |
| `Space b x` | Close buffer; modified text requires confirmation |
| `Space s v` / `Space s h` | Vertical / horizontal split |
| `Space s n` / `Space s p` | Focus next / previous split |
| `Space s +` / `Space s -` | Grow / shrink the nearest split containing the focused view |
| `Space s x` | Close focused split, or current tab; last view uses safe quit |
| `Space b t n` | New tab page viewing the current buffer |
| `Space b t d` / `Space b t a` | Next / previous tab page |
| `Space n` | New empty buffer, preserving existing buffers |
| `Space w` / `Ctrl-S` | Save current file |
| `Space q` | Close current view or quit |

In a picker, type your query normally, including WASD. Use arrows, `Ctrl-W`/`Ctrl-S`, or `Ctrl-P`/`Ctrl-N` to navigate; Page Up/Down or `Ctrl-U`/`Ctrl-D` moves ten results. `Tab` moves down, `Enter` accepts, `Esc` closes, and `Ctrl-R` refreshes the project index. Wide windows show a live preview; narrow windows prioritize the result list. Grep results show path, line, column, and highlighted matches. Opening a match moves to its location. Already-open files preview their unsaved contents; live grep itself searches **disk contents**.

File matching uses nucleo smart-case fuzzy patterns, including its pattern syntax, with an additional exact filename-stem preference. Stable ties preserve source order (MRU order for recents). Live grep uses smart-case regular expressions: a lowercase pattern ignores case; uppercase makes it case-sensitive. Invalid patterns display an error inside the picker.

The project index is built asynchronously at startup and reused across queries. Queries are debounced (60 ms by default), and stale work/results are cancelled or discarded. Filesystem traversal respects `.gitignore`, `.ignore`, global ignores, and hidden-file settings, even outside Git repositories. `.git` is always excluded. Directory symlinks are not followed. Use `Ctrl-R` in navigation or `:refresh` after external filesystem changes; explorer operations refresh automatically.

Defaults cap indexing at 200,000 entries, results at 300, grep files at 8 MiB, and previews at 512 KiB. The interface reports index/result limits and skipped entries. Grep skips binary, invalid UTF-8, oversized, and inaccessible files. Previews show up to 60 bounded lines around a result, highlight matching text, and display a message for binaries, read errors, or content beyond the preview limit. Initial indexing time depends on repository size; editing remains available throughout.

## Buffers, splits, and the explorer

Each document owns one Ropey buffer and undo history. Splits and tab pages hold lightweight views with independent grapheme cursors, desired columns, selections, modes, and scroll positions. Editing in one view updates the shared document; other views' positions are adjusted for insertions/deletions. Undo/redo clamps other views' positions to valid text. The copy/cut register is shared across buffers.

Opening a file or creating a new buffer retains the previous buffer, including unsaved text. Closing a view retains its document. Closing a modified buffer asks you to type `discard`, or use `:bd!` explicitly. Quit checks **all** buffers, including hidden ones. `:qa!` deliberately discards all unsaved buffers. Saves in progress finish before quitting or closing their buffer.

In the explorer:

| Key | Action |
| --- | --- |
| `W` / `S`, arrows | Previous / next row |
| `D`, Right, `Enter` | Expand/collapse directory or open file |
| `A`, Left | Collapse directory or move to parent |
| `I` | Create file inside selected directory (or file's parent), then open it |
| `Shift-I` | Create directory |
| `R` | Rename file or directory |
| `X` | Delete; type `delete` in the confirmation dialog |
| `Ctrl-R` | Refresh |
| `Esc` | Return focus to editor, leaving explorer visible |
| `Space e` | Close explorer |

Creation/rename dialogs accept a name or a relative path whose parent already exists. Enter confirms; Esc cancels. Renaming updates affected open buffers and recent-file paths. Deleting removes directory contents recursively and closes affected clean buffers. Filavim refuses to delete paths containing modified buffers, to overwrite a rename destination, to mutate its workspace root, or to mutate outside that root. Existing parent symlinks are resolved and checked by the I/O worker. File operations wait for pending saves to avoid racing an old pathname.

Editing can continue during a confirmed deletion. If newer text appears before deletion completes, Filavim retains it in an unnamed modified buffer. Saving waits for the filesystem operation to finish.

Recent files and projects persist separately from configuration: `%APPDATA%\filavim\history.toml` on Windows; `$XDG_STATE_HOME/filavim/history.toml` or `~/.local/state/filavim/history.toml` on Linux/macOS. Lists contain up to 100 unique entries. Missing recent paths remain available so a deleted file can be recreated; inaccessible paths report an opening error.

## Editing and commands

| Key | Normal / Visual mode |
| --- | --- |
| `W A S D` | Up, left, down, right |
| `I` / `Shift-I` | Insert at cursor / append at line end |
| `V` | Begin/end inclusive grapheme selection |
| `Esc` | Return to Normal |
| `Q` / `E` | Previous / next word |
| `0` / `$`, Home / End | Line start / end |
| `G G` / `Shift-G`, Ctrl-Home / Ctrl-End | First / last line |
| `X` / `Shift-X` | Delete grapheme or selection / delete line |
| `Backspace` | Delete previous grapheme or selection |
| `O` | Open an indented line below and enter Insert |
| `Y` / `C` / `P` | Copy / cut / paste selection, otherwise a whole line |
| `U` / `Ctrl-R` | Undo / redo |
| `/text`, Enter; `N` | Literal buffer search; next result, with wraparound |
| `:` | Command line |
| `F1`, `:help`, `Space ?` | Guide; Page Up/Down switches editing/navigation pages |

Insert types text normally. Enter preserves indentation, Tab uses configurable stops, and Backspace/Delete remove full graphemes. A continuous Insert session is an undo group; switching views ends that group. `Ctrl-Z`/`Ctrl-R` undo/redo in Insert. Arrows, Home/End, and Page Up/Down work in editing modes. Windows input preserves Unicode Alt-code release events and AltGr text.

Copy/cut writes to the internal register and, when enabled, the terminal clipboard using OSC 52 (up to 100 KB). Terminal/multiplexer support varies. Filavim does not read the OS clipboard: enter Insert and use your terminal's paste shortcut, usually Ctrl-Shift-V or Cmd-V. Bracketed-paste events insert text directly. Windows consoles can deliver paste as ordinary character events, so enter Insert before pasting.

| Command | Action |
| --- | --- |
| `:w [path]` | Save / save under a name |
| `:wq [path]`, `:x [path]` | Save, then close view or safely quit |
| `:e path` | Open or switch to a buffer, retaining current edits |
| `:e` / `:e! [path]` | Reload / reload and discard changes |
| `:new`, `:enew` | New buffer; previous buffers stay open |
| `:new!` | Discard the current buffer and create an empty one |
| `:bd` / `:bd!` | Close buffer / discard it |
| `:bn` / `:bp`, `:buffers` / `:ls` | Next / previous buffer; picker |
| `:split [path]` / `:vsplit [path]` | Horizontal / vertical split, optionally opening a file |
| `:tabnew [path]` | New tab page, optionally opening a file |
| `:tabnext` / `:tabprevious` | Switch tab page |
| `:q` / `:q!` | Close view; on last view quit safely / discard all and quit |
| `:qa` / `:qa!` | Quit all safely / discard all and quit |
| `:cd [path]`, `:pwd` | Change workspace (dialog without path); display root |
| `:files`, `:grep [regex]`, `:recent` | Navigation pickers |
| `:projects`, `:commands`, `:explore` | Project picker, palette, explorer |
| `:refresh` | Rescan the workspace |
| `:42`, `:help` | Go to line 42; guide |

Paths may contain spaces or be quoted, and are literal: no shell expansion, globbing, or `~` expansion. Saves use a same-directory temporary file, flush to disk, preserve existing permissions, and atomically replace the destination. Symlinks are followed; open/pending buffers cannot save over another buffer's destination. Atomic replacement changes file identity; hard links, extended attributes, and external modifications are not tracked. LF/CRLF, UTF-8 BOMs, and the presence/absence of a final newline are preserved. Inserted lines use the first detected newline style. Invalid UTF-8 is rejected.

Editing continues during saves. Completion marks only the saved snapshot clean, and `:wq` closes only if that snapshot is current and the originating view is still active. Reload cancels if the document changed while loading. Ordinary opening retains existing edits in their buffers.

## Appearance, animation, and configuration

No configuration is required. Copy [config.example.toml](config.example.toml) to `%APPDATA%\filavim\config.toml` on Windows, or `$XDG_CONFIG_HOME/filavim/config.toml` / `~/.config/filavim/config.toml` on Linux/macOS. `FILAVIM_CONFIG` overrides the path. Invalid configuration produces an error before entering terminal mode.

| Preset | Cursor effects | Viewport scrolling | Borders | Animation cap |
| --- | --- | --- | --- | --- |
| `beautiful` | Four-corner spring smear, full gradient | Smooth, 120 ms | Enhanced focused border | 60 FPS |
| `balanced` (default) | Same spring, shorter gradient | Instant | Rounded | 60 FPS |
| `minimal` | Native cursor only | Instant | Plain, no active-line shading | No animation frames |

The cursor is a native Rust port/adaptation of [smear-cursor.nvim](https://github.com/sphamba/smear-cursor.nvim)'s default spring and Unicode rasterization model. Four independently moving corners stretch and contract into a geometric smear, with sub-cell fractional blocks, quadrant coverage, and a gamma-adjusted Iris gradient. The native terminal cursor is hidden only while the overlay is active and returns on convergence. The logical cursor moves immediately.

**Only Normal-mode navigation animates.** Insert, Visual, and Command modes use the native cursor immediately. Entering these modes cancels any smear; returning to Normal seeds a new position without a synthetic transition. Text edits never start a smear. Buffer/view switches, overlays, resize, and layout changes reset it. Scrolling shifts active geometry in buffer space and clips it to the text viewport, excluding borders and line numbers. Smooth viewport scrolling also runs only in Normal mode. Nothing renders continuously when idle.

The source, adaptation details, and GPLv3 distribution implications are documented in [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).
Override settings independently of the preset:

```toml
preset = "beautiful"
tab_width = 4

[visual]
cursor_animation = true
smooth_scroll = false
scroll_duration_ms = 120
trail_intensity = 10 # upstream gradient; lower values shorten the visible tail
easing = "cubic" # viewport scrolling only: linear, quadratic, cubic
fps = 60 # 1..240; 120/144 are also supported
colors = "auto" # truecolor, indexed, ansi also available

[visual.smear]
# frame_interval_ms = 8 # optional ~125 FPS; overrides fps
stiffness = 0.6
trailing_stiffness = 0.45
damping = 0.85
anticipation = 0.2
trailing_exponent = 3.0
max_length = 25.0
gradient_exponent = 1.0
color = "#c4a7e7" # omit to follow theme Iris
quality = "blocks" # blocks | legacy | ascii | native

[navigation]
show_hidden = false
respect_ignore = true
max_files = 200000
max_results = 300
grep_max_bytes = 8388608
preview_max_bytes = 524288
debounce_ms = 60
explorer_width = 28

[keybindings.normal]
"space z" = "find_files"

[keybindings.picker]
"ctrl-j" = "down"
"ctrl-k" = "up"
```

Omitted visual options inherit the preset. Scroll duration accepts 16–500 ms, FPS 1–240, and trail intensity 0–10 (0 gives the shortest gradient; 10 matches upstream). Disable the effect with `cursor_animation = false` or `quality = "native"`. Minimal creates no spring or animation frames unless explicitly overridden. `cursor_duration_ms` remains accepted for old configurations but is deprecated and ignored: springs terminate by convergence, not a fixed duration. `easing` applies only to scrolling.

Spring stiffness, trailing stiffness, and damping must be strictly between 0 and 1; anticipation accepts 0–1, exponents 0.1–10, maximum length 2–100 cells, and frame interval 4–1000 ms. An explicit frame interval overrides preset FPS. The reference timestep is 17 ms, stop distance and speed are 0.1 cells, and colors use 16 levels with gamma 2.2. Elapsed time accumulates into the reference’s fixed 17 ms spring steps, with fractional samples between successive spring states. Display rate and extra input redraws do not change the trajectory. Delayed frames advance the simulation without replaying queued terminal frames.

`blocks` uses common Unicode fractional blocks and quadrants, with inverted foreground/background for top/right edges. `legacy` adds diagonal and eighth-block symbols from Symbols for Legacy Computing; enable it only with a font/terminal that renders these as one cell. Glyph coverage cannot be reliably autodetected. Use `ascii` if blocks render poorly, or `native` for the hardware cursor alone. `unicode = false` selects ASCII fallback automatically. Indexed/ANSI color modes quantize the gradient to their available palettes.

Theme overrides accept official Rosé Pine Moon token names with `#rrggbb` values. `syntax = false`, `line_numbers = false`, and `syntax_max_bytes` offer additional performance control. Syntax highlighting defaults to a 2 MiB limit and falls back to plain text above it.

Keybinding tables are `normal`, `insert`, `visual`, `command`, `picker`, `explorer`, and `dialog`. Defaults are merged with overrides; `noop` disables a binding. Named keys include `space`, `esc`, `enter`, `tab`, `backspace`, `delete`, arrows, `home`, `end`, `pageup`, `pagedown`, and `f1`. Use prefixes in `ctrl-alt-shift-` order; shifted letters use `shift-i`, while punctuation uses the literal symbol. Separate sequence keys with spaces. A complete action takes precedence over longer sequences, so do not bind an action directly to a leader prefix such as `space f` unless you intend to replace its subcommands. Insert/picker/dialog custom bindings may consume typed text.

Navigation action names are `find_files`, `live_grep`, `recent_files`, `buffers`, `palette`, `projects`, `explorer`, `split_vertical`, `split_horizontal`, `focus_next`, `focus_previous`, `grow_split`, `shrink_split`, `close_view`, `close_buffer`, `next_buffer`, `previous_buffer`, `new_tab`, `next_tab`, `previous_tab`, `refresh`, `create_file`, `create_directory`, `rename_file`, `delete_file`, `parent_directory`. Editing actions remain in `src/input.rs`: movement, modes, insert/delete, undo/redo, clipboard, save/quit/new/help/submit/search, and `noop`. Actions are interpreted within their mode; filesystem actions apply in the explorer, for example.

## Architecture and verification

- `buffer` and `editor`: terminal-independent editing, graphemes, persistent undo snapshots, selections, and commands.
- `workspace`: shared document ownership, independent views, recursive split geometry, tab pages, and lifecycle rules.
- `input`: mode-aware remappable actions and cross-platform key normalization.
- `navigation` and `background`: cached ignore traversal, nucleo ranking, grep workers, tree rows, bounded previews, and replaceable/cancellable jobs.
- `storage` and `files`: serialized asynchronous load/save, root validation, confirmed mutations, streaming buffer search, and persistent recents.
- `syntax`: per-document coalesced Tree-sitter jobs and a bounded incremental-tree cache for Rust, Python, JavaScript, and JSON.
- `lsp`, `ide`, `ide_ui`: JSON-RPC lifecycle, versioned edits, completion/refactoring and IDE surfaces.
- `contextual`, `contextual_ui`: viewport-aware caret anchors, compact completion and parameter tooltips, Unicode signature ranges.
- `git`, `terminal`, `setup`, `snippet`: background Git, PTY/ANSI sessions, confirmed installations/settings and native snippet fields.
- `app` and `runtime`: state transitions, stale-result checks, pending saves, terminal events, and scheduled rendering.
- `ui`, `workspace_ui`, `theme`, and `animation`: shared surfaces, workspace composition, color fallbacks, and cell-based effects.

All interactive disk work, traversal, matching, grep, previews, and parsing run off the input thread. Startup loads initial files/configuration/history before terminal entry. Ratatui emits changed cells; frames are scheduled for state changes and active effects, with no idle redraw loop. Extremely long individual lines still take linear display-column/rendering work. Further profiling and large-file optimization remain Stage 4 work.

```sh
cargo fmt --check
cargo clippy --all-targets --locked -- -D warnings
cargo test --locked
cargo build --release --locked
cargo run --example snapshots
```

Tests cover editing regressions, Unicode across rope chunks, fuzzy scores/highlights, ignored/hidden files, regex case behavior and binaries, preview bounds, cancellation, buffer lifecycle, split geometry/cursors, shared registers, root/history persistence, remapping/presets, filesystem confirmations, and tiny-window rendering. Only the available Windows environment was tested for this Stage 3 implementation. `snapshots` renders actual widgets into dashboard/editor/workspace/picker SVGs under `target/preview/` for visual review.

Stage 3 IDE features and their verified limits are described in [STAGE3.md](STAGE3.md). Debugging, merge/rebase workflows, and Stage 4 distribution work are outside this implementation. Sessions retain recent paths, not unsaved text or split layouts after application exit.

Animation delivery uses monotonic, phase-aligned deadlines independent of keyboard input. Input and worker results redraw immediately without postponing the next animation frame. Cursor-only frames reuse the clean editor surface; Ratatui diffs the old/new overlay so only changed cells reach the backend. Text, layout, syntax, and scrolling changes rebuild the surface. Output is buffered in 64 KiB chunks and flushed by each terminal draw. Windows requests 1 ms timer resolution only while animations are active and releases it on settling, cancellation, or exit. Fractional poll timeouts round upward to avoid zero-timeout spin loops. Beautiful and Balanced now default to 60 FPS; explicit older FPS/frame-interval overrides still apply.

## Cursor verification

`tests/cursor.rs` and the rasterizer unit tests cover reference physics values, convergence in all directions, rapid retargeting, geometry clipping, fractional/diagonal glyphs, gamma colors, native-cursor visibility, mode cancellation, editing, viewport shifts, presets, and invalid configuration. Existing Stage 1/2 regression suites remain enabled. `cargo run --example cursor_frames` exports deterministic actual-widget animation frames under `target/cursor-preview/` for visual review.

Manual terminal verification is still needed for subjective fidelity and font-dependent glyph seams: try short/rapid WASD moves, word and document jumps, diagonal retargeting, scrolling, Unicode/wide text, split/tab switches and resize. Interrupt motion with `I`, type/delete/newline, then `Esc`; Insert must show only the immediate native cursor and returning to Normal must not smear. Check `legacy` on each intended font and terminal; use `blocks`, `ascii`, or `native` if unsupported. Stage 3 preserves these Normal-only cursor rules.

Smoothness changes are verified by deterministic clock/physics tests, in-memory frame comparisons, output-batching tests, and standard build checks. These tests do not measure the host terminal’s achieved FPS; visual smoothness and terminal-specific throughput remain for user verification.


### Responsiveness verification

Input drains only already-queued events, at most 32 or 2 ms per batch; single keys
add no batching wait. Background result drains are bounded. Completion selection
runs on a cancellable worker, and incremental LSP sync uses a bounded edit journal.
Terminal snapshots contain visible cells/modes, leaving scrollback on the worker;
hidden session publications do not dirty the editor. Existing cursor physics and
cached cursor-only frames are preserved.

See [PERFORMANCE.md](PERFORMANCE.md) for noninteractive benchmarks, evidence limits,
and manual verification suggestions. No live UI automation is used.
