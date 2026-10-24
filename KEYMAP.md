# Filavim — Complete Default Keymap

This reference matches the implemented Stage 3 defaults. User configuration can override them. Commands have a separate [complete reference](COMMANDS.md).

## Notation

- Lowercase letters mean unshifted keys: `w` means W without holding Shift.
- `Shift-i` means hold Shift and press I. Symbols such as `:`, `$`, `?`, and `+` mean the character shown, using Shift if your keyboard requires it.
- `Space f f` means press Space, release it, then press F twice. Sequences have no timeout; `Esc` cancels a pending sequence.
- A buffer is an open document. A view is one pane displaying a buffer. A tab page contains a layout of views.

## Normal and Visual modes

All bindings in these tables work in both modes unless noted. In Visual mode, movement extends the inclusive grapheme selection.

### Modes and help

| Keys | Action |
| --- | --- |
| `i` | Enter Insert at the cursor; leaves Visual selection mode |
| `Shift-i` | Move to line end and enter Insert |
| `v` | Enter Visual; press again to return to Normal |
| `Esc` | Return to Normal, clear the selection, cancel a pending sequence |
| `:` | Enter command-line mode |
| `/` | Enter literal buffer-search input |
| `F1` or `Space ?` | Open the built-in guide |

### Movement

| Keys | Action |
| --- | --- |
| `w` or `↑` | Up one line |
| `a` or `←` | Left one grapheme |
| `s` or `↓` | Down one line |
| `d` or `→` | Right one grapheme |
| `q` | Previous word |
| `e` | Next word |
| `0` or `Home` | Start of current line |
| `$` or `End` | End of current line |
| `g g` or `Ctrl-Home` | First line, at its start |
| `Shift-g` or `Ctrl-End` | Last line, at its start |
| `Page Up` | Move up by the editor viewport height |
| `Page Down` | Move down by the editor viewport height |
| `n` | Next result of the last literal buffer search; wraps |

### Editing and clipboard

| Keys | Action |
| --- | --- |
| `x` or `Delete` | Delete the current grapheme; in Visual, delete the selection |
| `Backspace` | Delete the previous grapheme; in Visual, delete the selection |
| `Shift-x` | Delete the current line, storing it in the internal register |
| `o` | Create an indented line below and enter Insert |
| `y` | Copy the selection; otherwise copy the current line |
| `c` | Cut the selection; otherwise cut the current line |
| `p` | Paste the internal register; replace a Visual selection |
| `u` | Undo |
| `Ctrl-r` | Redo |
| `Ctrl-s` or `Space w` | Save the active buffer; unnamed buffers need `:w <path>` |
| `Space n` | Create an empty buffer, retaining existing buffers and their edits |
| `Space q` | Close the current view, or safely quit if it is the last view |

Copy/cut can also write the terminal clipboard using OSC 52 when enabled and supported. The internal register works across buffers. To paste from the system clipboard, enter Insert and use the terminal's paste shortcut.

### Finders and workspace navigation

| Keys | Action |
| --- | --- |
| `Space f f` | Fuzzy file finder |
| `Space f g` | Live project-wide regex grep |
| `Space f r` | Recent-file picker |
| `Space f p` | Recent-project picker |
| `Space b b` | Buffer picker |
| `Space p` | Command palette |
| `Space e` | Toggle explorer; opening it gives it keyboard focus |
| `Space b n` | Next buffer |
| `Space b p` | Previous buffer |
| `Space b x` | Close buffer; modified text requires typing `discard` to confirm |

### Splits and tab pages

| Keys | Action |
| --- | --- |
| `Space s v` | Split vertically: panes side by side |
| `Space s h` | Split horizontally: panes stacked |
| `Space s n` | Focus next pane in layout order |
| `Space s p` | Focus previous pane in layout order |
| `Space s +` | Grow the focused pane at its nearest split boundary |
| `Space s -` | Shrink the focused pane at its nearest split boundary |
| `Space s x` | Close pane; close tab if it has one pane; safely quit on the final view |
| `Space b t n` | New tab page viewing the current buffer |
| `Space b t d` | Next tab page |
| `Space b t a` | Previous tab page |

Closing a view does not close its buffer. Split views share text and undo history but retain independent cursor and scroll positions.

## Insert mode

Ordinary characters, including WASD, insert text.

| Keys | Action |
| --- | --- |
| `Esc` | Return to Normal |
| `↑` / `←` / `↓` / `→` | Move cursor |
| `Home` / `End` | Current line start / end |
| `Ctrl-Home` / `Ctrl-End` | First / last line, at its start |
| `Page Up` / `Page Down` | Move by one viewport |
| `Enter` | Insert a newline with configured auto-indentation |
| `Tab` | Insert indentation using configured tab stops or a literal tab |
| `Backspace` / `Delete` | Delete previous / current grapheme |
| `Ctrl-z` | Undo |
| `Ctrl-r` | Redo |
| `Ctrl-s` | Save |
| `F1` | Open guide |
| Terminal paste shortcut | Insert clipboard text; commonly `Ctrl-Shift-v` or `Cmd-v` |

A continuous Insert session is an undo group. Switching views ends the current group.

## Pickers

Applies to files, grep, recents, buffers, projects, and the command palette. Ordinary characters edit the query; WASD are query text here.

| Keys | Action |
| --- | --- |
| `↑`, `Ctrl-w`, or `Ctrl-p` | Previous result |
| `↓`, `Ctrl-s`, `Ctrl-n`, or `Tab` | Next result |
| `Page Up` or `Ctrl-u` | Up ten results |
| `Page Down` or `Ctrl-d` | Down ten results |
| `Backspace` | Remove the previous query grapheme |
| `Enter` | Accept the selected result |
| `Ctrl-r` | Refresh the project index |
| `Esc` | Close the picker |

Notice that `Ctrl-s` navigates results here; it does not save. Selecting a different result updates its preview automatically.

## File explorer

| Keys | Action |
| --- | --- |
| `w` or `↑` | Previous row |
| `s` or `↓` | Next row |
| `Page Up` / `Page Down` | Up / down ten rows |
| `a` or `←` | Collapse the current expanded directory, or move to/collapse its parent |
| `d`, `→`, or `Enter` | Expand/collapse directory, or open file |
| `i` | Create a file in the selected directory or selected file's parent, then open it |
| `Shift-i` | Create a directory |
| `r` | Rename selected entry |
| `x` | Request deletion; type `delete` in the dialog to confirm |
| `Ctrl-r` | Refresh |
| `Esc` | Focus the editor, keeping the explorer visible |
| `Space e` | Close explorer |
| `Space f f` | Open file finder |
| `Space p` | Open command palette |

## Command and buffer-search input

Enter with `:` for commands or `/` for literal in-buffer search.

| Keys | Action |
| --- | --- |
| Ordinary characters | Append to input |
| `Backspace` | Remove the previous grapheme |
| `Enter` | Execute command or search, then return to Normal |
| `Esc` | Cancel input and return to Normal |
| `F1` | Open guide |

Current command entry supports appending and backspacing. Cursor-navigation keys and `Ctrl-s` have no action in this mode, despite inheriting keymap entries.

## Dialogs

| Keys | Action |
| --- | --- |
| Ordinary characters | Append to the dialog input |
| `Backspace` | Remove the previous grapheme |
| `Enter` | Submit the name/path or confirmation text |
| `Esc` | Cancel |

Deletion requires exactly `delete`; closing a modified buffer requires exactly `discard`. Arrow and page-navigation bindings currently have no dialog action.

## Guide

| Keys | Action |
| --- | --- |
| `F1` or `Esc` | Close the guide |
| `Page Up`, `Page Down`, `←`, or `→` | Switch between editing and navigation pages |
| `a` or `d` | Also switch pages when the underlying mode is Normal or Visual |

Closing the guide returns to the underlying mode.

## Remapping

Set overrides in `config.toml`. Supported tables are `normal`, `visual`, `insert`, `command`, `picker`, `explorer`, and `dialog`.

```toml
[keybindings.normal]
"space z" = "find_files"

[keybindings.picker]
"ctrl-j" = "down"
"ctrl-k" = "up"
```

Use lowercase names, `shift-i` for uppercase letters, literal punctuation, and space-separated sequences. Modifier order is `ctrl-alt-shift-`. `noop` disables a default binding. An action bound directly to a leader prefix takes precedence over its longer sequences. Action names are defined in [src/input.rs](src/input.rs).


## Stage 3 IDE controls

| Mode / keys | Action |
| --- | --- |
| Normal/Visual: `Space c a` | Code actions / quick fixes |
| Normal/Visual: `Space c r` | Rename symbol |
| Normal/Visual: `Space c f` | Format document / visual selection |
| Normal/Visual: `Space c s` | Native snippets |
| Normal/Visual: `Space g d` / `Space g i` | Definition / implementation |
| Normal/Visual: `Space g r` | References |
| Normal/Visual: `Shift-k` | Hover documentation |
| Normal/Visual: `Space g s` / `Space g h` | Git status / hunks |
| Normal/Visual: `Space t` | Toggle/focus embedded terminal |
| Normal/Visual: `Space l` | LSP information |
| Normal/Visual: `Space ,` | Settings |
| Insert: `Ctrl-space` / `Ctrl-k` | Completion / signature help |
| Completion: `↑`/`↓` or `Ctrl-p`/`Ctrl-n` | Select previous/next suggestion |
| Completion: `Tab` or `Enter`, `Esc` | Accept; dismiss and return to Normal |
| Snippet fields: `Tab` | Advance to next placeholder |
| Dialog/Command: `Ctrl-u` | Clear input |
| Terminal input: `Ctrl-]` | Return to editor keybindings |
| Terminal input: `Ctrl-PageUp/Down` | Switch sessions |
| Terminal input: `Shift-PageUp/Down` | Scroll terminal history |
| Terminal input: `Ctrl-Shift-x` | Close active terminal session |

Tab page leaders are now `Space b t n/d/a`, reserving `Space t` for terminals.
IDE lists use the existing picker arrows/Ctrl-W/S and filtering controls.
Completion is positioned at the editing caret; signature hints appear above the
line when space permits and update automatically inside function calls. Ctrl-K
requests a signature explicitly. These hints do not take focus from Insert mode.
Settings dialogs support Ctrl-U to replace their prefilled value. Workspace edits
require typing `apply`; installations require typing `install`; Git commits have
a separate message confirmation. Full instructions: [STAGE3.md](STAGE3.md).
