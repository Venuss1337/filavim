# Filavim — Complete Command Reference

These are the commands currently implemented in the Stage 3 application, including aliases. For keyboard shortcuts, see the [complete keymap](KEYMAP.md).

Press `:` from Normal or Visual mode, type a command, and press `Enter`. `Esc` cancels. Names are case-sensitive; only aliases explicitly listed below are supported. `[argument]` means optional; `<argument>` means required.

## Files and saving

| Command | Aliases | Behavior |
| --- | --- | --- |
| `:open` | — | Open a dialog for a file path |
| `:e <path>` | `:edit <path>` | Open a file, or switch to its existing buffer; retain other buffers and their edits |
| `:e` | `:edit` | Reload the current file; reject if it has unsaved changes |
| `:e! [path]` | — | Reload the specified already-open file, or current file without a path, discarding its unsaved changes; an unopened path is opened normally |
| `:w [path]` | `:write [path]` | Save the active buffer; an optional path saves under that name |
| `:wq [path]` | `:x [path]` | Save, then close the originating view or safely quit |

Unnamed buffers need a path on their first save. `:x` currently saves even if the buffer is unchanged. If newer edits appear during saving, they remain modified and `:wq` does not close the view. Reload is cancelled if the buffer changes while loading. A missing file can be opened as a named empty buffer if its parent directory exists.

## Buffer management

| Command | Aliases | Behavior |
| --- | --- | --- |
| `:new` | `:enew` | New unnamed buffer; keep existing buffers and unsaved text |
| `:new!` | — | Discard the current buffer and create an empty one |
| `:bd` | `:bdelete` | Close current buffer; modified text requires typing `discard` in a dialog |
| `:bd!` | — | Close current buffer and explicitly discard its changes |
| `:bn` | `:bnext` | Next open buffer |
| `:bp` | `:bprevious` | Previous open buffer |
| `:buffers` | `:ls` | Open the fuzzy buffer picker |

Closing a buffer updates all views displaying it. If it is the final buffer, an empty replacement is created. A buffer with a save in progress cannot be closed or discarded until that save completes.

## Splits and tabs

| Command | Aliases | Behavior |
| --- | --- | --- |
| `:split [path]` | — | Horizontal split: panes stacked; optionally open a file in the new pane |
| `:vsplit [path]` | — | Vertical split: panes side by side; optionally open a file in the new pane |
| `:tabnew [path]` | — | New tab page; without a path it views the current buffer, otherwise opens the requested file |
| `:tabnext` | — | Next tab page, wrapping |
| `:tabprevious` | — | Previous tab page, wrapping |

Without a path, splits share the current buffer while maintaining separate cursor and scroll positions. Pane focus and resizing use the shortcuts in [KEYMAP.md](KEYMAP.md); there are no separate focus/resize commands yet.

## Closing and quitting

| Command | Aliases | Behavior |
| --- | --- | --- |
| `:q` | `:quit` | Close focused pane/tab if another view remains; on the last view, quit only if every buffer is saved |
| `:q!` | — | Close focused pane/tab if another view remains; on the last view, discard all unsaved buffers and quit |
| `:qa` | — | Quit all views, only if every buffer is saved |
| `:qa!` | — | Discard all unsaved buffers and quit all views |

Closing a view keeps its buffer open. Consequently, `:q!` does **not** exit the whole application while other views remain; use `:qa!` for that. Quit-all requests are refused while saves or filesystem mutations are pending. Recent-history writes finish before application exit.

## Workspace and navigation

| Command | Aliases | Behavior |
| --- | --- | --- |
| `:cd <path>` | `:lcd <path>` | Change the workspace root; keep open documents and their absolute paths |
| `:cd` | `:lcd` | Open a directory-path dialog |
| `:pwd` | — | Display the workspace root |
| `:files` | — | Fuzzy project-file picker |
| `:grep [regex]` | — | Live project-wide grep, optionally starting with a regex |
| `:recent` | — | Recent-file picker |
| `:projects` | — | Recent-project picker |
| `:commands` | — | Fuzzy command palette |
| `:explore` | — | Toggle/focus file explorer |
| `:refresh` | — | Rebuild the project index |

`:lcd` currently changes the same workspace-wide root as `:cd`; it is not local to a pane. Grep searches disk contents with smart-case regular expressions. File, buffer, recent, project, and command pickers use fuzzy matching.

## Other commands

| Command | Behavior |
| --- | --- |
| `:<number>` | Go to the start of that line; for example, `:42` |
| `:help` | Open the built-in two-page guide |
| Empty command | Do nothing |

Line numbers are one-based. `:0` goes to the first line; a number beyond the document length goes to the last line.

## Path syntax and examples

Relative editor paths resolve against the current workspace root. Paths may contain spaces, with or without matching outer single or double quotes. Paths are literal: no wildcard, shell, environment-variable, or `~` expansion.

```text
:e src/main.rs
:w "notes/design draft.md"
:vsplit src/lib.rs
:split README.md
:tabnew tests/editor.rs
:grep pub\s+fn
:cd ../another-project
:42
:wq
```

Only the spellings in the tables are accepted. For example, use `:bd!`, not `:bdelete!`, and `:e!`, not `:edit!`.

File/directory creation, rename, and deletion are explorer actions with dialogs, rather than colon commands. Use `Space e`, then `i`, `Shift-i`, `r`, or `x`.

Implementation sources: [src/app.rs](src/app.rs) handles workspace commands; [src/editor.rs](src/editor.rs) handles save, help, and numeric-line commands.


## Stage 3 commands

Stage 3 command names are case-insensitive.

| Command | Action |
| --- | --- |
| `:Settings` | Persistent, filterable settings panel |
| `:Setup` | Server/font discovery, prerequisites and confirmed installation |
| `:LspInfo` / `:LspRestart` | Server status/capabilities / retry startup |
| `:LspInstall <id>` / `:LspUpdate <id>` | Confirm install/update: rust, python, typescript, clangd, go, json |
| `:Complete` / `:Signature` / `:Hover` | Completion / parameter hints / documentation |
| `:Definition` / `:Implementation` / `:References` | Symbol navigation |
| `:Rename [name]` | Rename prompt or requested name; edits require confirmation |
| `:CodeActions` | Quick fixes and refactorings |
| `:Format` / `:RangeFormat` | Document / selection formatting; edits require confirmation |
| `:GitStatus` | Repository/file status |
| `:GitDiff [staged]` | Unstaged/staged hunk picker; Enter previews, Enter stages/unstages |
| `:GitStage` / `:GitUnstage` | Stage/unstage active editor file (save first) |
| `:GitPrevious` | Read-only HEAD version of active file |
| `:GitCommit [message]` | Commit staged changes after separate confirmation |
| `:Terminal [vertical]` | Horizontal/vertical PTY panel |
| `:TerminalNew` / `:TerminalNext` / `:TerminalClose` | Create/switch/terminate session |
| `:Snippets` | Expand a native snippet with Tab placeholder navigation |
| `:Set <key> <value>` | Validate, persist and hot-apply a TOML setting |
| `:SetKey <mode> <key> <action>` | Set a single key binding |
| `:SetupFont` | Inspect private development font availability; confirmed per-user installation / Windows Terminal configuration |
| `:FontStatus` | Read-only font installation, terminal configuration and sample glyph-coverage status |
| `:RemoveFont` | Confirm removal of only an unchanged Filavim-managed font; terminal settings and backups are retained |

Examples: `:Set visual.fps 60`, `:Set line_numbers false`,
`:Set server.rust.command C:/tools/rust-analyzer.exe`,
`:Set server.python.enabled false`, `:SetKey normal w up`.
Use the TOML file for multi-key leader sequences and advanced server arguments.
See [STAGE3.md](STAGE3.md) for limitations, installer sources and manual checks.
See [FONT_SETUP.md](FONT_SETUP.md) for private font builds and terminal support.
