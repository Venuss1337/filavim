# Native IDE guide

Stage 3 adds a native Rust JSON-RPC/LSP client, background Git jobs, real PTY
sessions, snippets, and settings/setup panels. No server, package manager, font,
or scripting runtime is bundled. Your existing cursor configuration is retained;
smear remains limited to Normal-mode document navigation.

## Language servers

Open a supported file. Filavim starts the configured server lazily, reuses it
within the workspace, and reports its status in `:LspInfo`. Missing/crashed servers
do not prevent editing. `:LspRestart` retries after fixing an installation/path.

| Language | Default executable | Setup action / prerequisite |
| --- | --- | --- |
| Rust | `rust-analyzer` | `rustup component add rust-analyzer rust-src`; requires rustup |
| Python | `pyright-langserver --stdio` | npm global `pyright`; requires Node.js/npm |
| TypeScript/JavaScript/TSX/JSX | `typescript-language-server --stdio` | npm global `typescript-language-server typescript` |
| C/C++ | `clangd` | Guided LLVM/clangd installation; configure its executable path |
| Go | `gopls` | `go install golang.org/x/tools/gopls@latest`; requires Go |
| JSON/JSONC | `vscode-json-language-server --stdio` | npm global `vscode-langservers-extracted` |

Distribution references: [rust-analyzer](https://rust-analyzer.github.io/book/rust_analyzer_binary.html),
[Pyright](https://github.com/microsoft/pyright/blob/main/docs/installation.md),
[TypeScript language server](https://github.com/typescript-language-server/typescript-language-server),
[gopls](https://go.dev/gopls/), and the community-maintained
[VS Code server extraction package](https://github.com/hrsh7th/vscode-langservers-extracted).

Use `:Setup`, select a package, press Enter, type `install`, then Enter. Nothing
installs before that confirmation. Prerequisites are checked on the worker;
failures and captured installer output appear in a result panel. The running
task is indicated in the status area. `:LspInstall python` and `:LspUpdate python`
use the same confirmation flow. npm/Go installations update to their package's
current release; Rust analyzer follows its installed rustup toolchain. Updating
the Rust toolchain itself remains an explicit rustup operation outside Filavim.
Clangd uses guidance rather than invoking an administrator package manager.

Set `server.rust.command`, `server.python.command`, etc. in `:Settings` to use
an absolute executable path. The corresponding `enabled` and comma-separated
`languages` settings control startup/associations. These virtual settings persist
into `[[lsp.servers]]` tables. You can also edit those tables directly to set
arguments and initialization options. Defining a server array replaces the
default registry; include every server you want. Restarting a server does not
require restarting the editor.

Completion requests use a separate Insert-mode quiet period capped at 60 ms; document sync retains the configured debounce. `Ctrl-Space` requests one
immediately. Completion is a compact caret-anchored dropdown aligned to the word
being completed, below the caret when possible and above it near the viewport's
bottom. Arrows or Ctrl-N/Ctrl-P select, Tab/Enter accepts, and Esc closes it and
returns to Normal. Kind icons use Nerd Fonts when enabled, with readable Unicode
or ASCII fallbacks. Details/documentation are resolved when supported and shown
beside the menu when the active split has room. Cached suggestions remain visible
while new results arrive; identifier-only changes can accept a safely rebased cached item immediately. Items with additional edits, commands, server data, or uncertain ranges still wait for a current response.

Signature help is a separate compact tooltip, normally above the editing line.
It triggers inside calls, updates as arguments change, highlights the server's
active parameter in Gold, and shows the active overload number when applicable.
Ctrl-K in Insert mode or `:Signature` requests it explicitly. Leaving the call or
Insert mode dismisses automatic hints. Context detection uses a bounded lexical
scan; the server provides the actual signature and parameter information. Long
signatures wrap to up to three rows, keeping the active parameter visible.
Completion takes placement priority; hints are suppressed when there is no
nonoverlapping space. Both overlays use the rendered caret and active text
viewport, respecting gutters, horizontal scrolling, splits, explorer/terminal
panels and resize. The editor currently uses horizontal scrolling, not soft wraps.

Symbol navigation,
hover, signatures, formatting and actions check server capabilities. Diagnostics
show severity markers in the gutter and the current line's message below the
editor. Versioned diagnostics and stale request replies are discarded.

Rename, formatting and code actions prepare a validated transaction off-thread.
Type `apply` in the confirmation to edit buffers. All affected documents become
open, unsaved buffers for review; save each one explicitly. Invalid positions,
overlapping edits, changed buffer versions, and resource create/rename/delete
operations are rejected before any buffer changes. UTF-8/UTF-16/UTF-32 columns are
negotiated; full and incremental document-sync servers are supported. Incremental
sync sends recorded edits in the negotiated encoding. Unsent changes coalesce without crossing document-open or request/notification barriers. Undo, reload, or an expired edit journal safely falls back to a whole-document update.

## Git

Git must be installed on PATH. Filavim invokes Git on a serialized background
worker rather than linking a second Git implementation. It shows the branch,
file statuses in the explorer, and line changes relative to the index, including
unsaved editor changes after debounce. Repository refreshes are rate-limited;
unchanged results do not redraw the UI. After a Git failure, retry explicitly with
`:GitStatus` rather than repeatedly spawning failing subprocesses.

`Space g s` / `:GitStatus` lists changed files. Enter opens a file's diff/hunk
list, Enter previews the selected hunk, and another Enter stages it. Use
`:GitDiff staged` for staged hunks; Enter then unstages the selected hunk. A hunk
that changed since preview is rejected. `:GitStage` / `:GitUnstage` act on the
active editor file; save its unsaved changes first. Binary/untracked files can be
staged as whole files. `:GitPrevious` shows the HEAD version in a read-only panel.
`:GitCommit` asks for a message and then a separate confirmation before committing
the current index. Merge/rebase/history management is outside this stage.

## Embedded terminals

`Space t` opens/focuses/toggles a terminal. `:Terminal vertical` places it beside
the editor; `:Terminal` uses a horizontal panel. `:TerminalNew` creates a session.
The configured shell and arguments run in a real PTY with ANSI parsing,
application cursor keys, device status replies, scrollback and resize support.
Sessions survive buffer/tab switches. Closing a session or exiting the editor
terminates and reaps its shell; output parsing and PTY I/O stay off the UI thread.

While terminal input has focus, keys go to the shell, including Esc and Ctrl-C.
`Ctrl-]` returns to editor controls. `Ctrl-PageUp/Down` switches sessions,
`Shift-PageUp/Down` scrolls terminal history, and `Ctrl-Shift-X` closes the active
session. `:TerminalNext` and `:TerminalClose` are available from editor focus.
`terminal.shell` and `terminal.args` apply to newly created sessions.

## Settings, fonts and snippets

`:Settings` / `Space ,` opens a filterable setting list. Enter edits a value;
Ctrl-U clears the old value. Enter validates, saves atomically and applies the
change. Esc cancels. `:Set visual.fps 60` is the equivalent direct command.
Theme names are `rose-pine-moon` and `high-contrast`; individual palette tokens
remain editable under `[theme]`. Editor controls, borders, status bar, line
numbers, explorer settings, animations, servers, Git and terminal options are
included. Existing TOML values are retained, but comments are not retained when saving.
Invalid settings leave both the file and running configuration unchanged.

`:SetKey normal w up` changes a single key binding; edit `[keybindings.*]` in the
config for leader sequences, or use `:Set keybindings.normal.KEY ACTION`.
Changes rebuild the active keymap without restarting. Tab-page leaders moved
from `Space t n/d/a` to `Space b t n/d/a` to reserve `Space t` for the terminal.

`:Setup` offers a confirmed installation of the OFL-licensed
[Fira Code 6.2 archive](https://github.com/tonsky/FiraCode/releases/tag/6.2).
Windows uses a per-user font directory/registry entry; Linux/macOS use per-user
font directories and available OS tools. Download/install operations were not
executed as part of automated verification. Choose the font in your terminal
settings afterward; Filavim cannot change the host terminal font. MonoLisa is
strictly user-supplied under its owner's license. `nerd_fonts = true` enables
optional explorer icons; default Unicode/ASCII indicators require no patched font.

`Space c s` / `:Snippets` lists native snippets for the current language. Enter
expands one, typing replaces the selected placeholder, and Tab advances fields.
User entries live in `[snippets.rust]`, `[snippets.python]`, etc. Numbered fields,
default text and `$0` are supported; nested/mirrored fields, variables, choices
and transformations are rejected rather than inserted incorrectly.

## Automated verification and limits

`tests/ide.rs` uses a compiled Rust mock server, temporary Git repositories,
headless Ratatui buffers, and noninteractive PTY processes. `tests/contextual.rs`
also checks caret anchoring, edge placement, clipping, Unicode widths, split
isolation, stable completion dimensions, scrolling and active-parameter styles.
Mock-server tests exercise automatic signature triggers and accepting refreshed
completion edits. It covers transport
framing, initialization/shutdown, cancellation/timeouts/crashes, incremental sync,
Unicode columns, stale responses, completion/snippets, multi-file transactions,
rename/format/actions, settings preservation, staging and PTY cleanup. The
existing Stage 1/2 and cursor suites remain enabled.

Only the available Windows environment is tested. Real installed language
servers, installers, font installation and subjective terminal UI behavior need
manual verification. Markdown documentation is shown as wrapped text. Terminal
graphics/mouse/extended keyboard protocols are not implemented. Diagnostics from
servers that omit versions cannot be attributed as strongly as versioned ones.
No debugging or Stage 4 packaging/performance work is included.

## Manual checklist (instructions only)

1. Open a supported project/file; run `:LspInfo`. Insert text, try Ctrl-Space,
   arrows and Tab, and introduce an error to check diagnostics/gutter markers.
2. In Normal mode use `Space g d`, `Space g i`, `Space g r`, Shift-K and
   `:Signature`. Try `Space c r`, `Space c a`, and `Space c f`; review their
   confirmations and undo behavior. Save changed buffers explicitly.
3. In a Git repository edit/save a file; check branch/explorer/gutter indicators.
   Use `Space g s`, `Space g h`, Enter/Enter for a hunk, and `:GitDiff staged`
   to unstage it. Test `:GitPrevious` and the two-step `:GitCommit` confirmation.
4. Open `Space t`, run a shell command, resize, use Ctrl-] to return, and create
   another session with `:TerminalNew`. Check session switching and cleanup.
5. Open `:Setup`, select a missing server, Enter, type `install`, Enter. Check
   prerequisite guidance/progress/errors and retry `:LspRestart` / `:LspInfo`.
6. Open `:Settings`, select `visual.fps`, Enter, Ctrl-U, enter a value and Enter.
   Check immediate application and persistence after restarting. Verify the
   existing smear still runs only in Normal mode. Test snippets with Space c s.
