# Native editing and file safety

The GPUI application preserves the modal editor. The terminal-edition KEYMAP.md
and COMMANDS.md are historical references, not native feature promises.

## File safety

Opening a missing filename resolves its existing parent and creates an in-memory
buffer. Saving tracks SHA-256, file identity, size, creation and modification
metadata. Checks run on the background executor, including a check just before
atomic replacement. New destinations use atomic no-clobber creation.

A disk conflict offers Reload, Keep, Save copy, or Overwrite. Reload explicitly
discards the editor version. Keep leaves both versions intact. Overwrite requires
a second confirmation and is checked against the observed external version again.
The dialog shows the destination and disk/editor sizes or external deletion.
On Windows, a temporary read handle denies ordinary write access during final
verification and ReplaceFileW. Existing incompatible writers cause an actionable
save failure. Delete sharing permits replacement, so another process can still
replace the path. ReplaceFileW creates a backup; the displaced file is compared
with the expected baseline before cleanup. A detected boundary change retains the
external version at the reported backup path and leaves the editor dirty, even
though the editor snapshot may already be at the destination. Partial API failures
retain available snapshot/backup paths for inspection. These exceptional paths
require manual comparison; they are not a transactional rollback.

There is no general expected-identity compare-and-swap parameter in these APIs.
Other filesystems retain the check-to-rename race. Privileged/mapped/path-mutating
writers and filesystem-specific behavior are not perfectly prevented. A rewrite
that restores identical bytes and all tracked metadata/identity is indistinguishable
from no change. Same-timestamp content changes and same-content identity replacements
are detected. Ordinary typing never probes disk.

References: [CreateFile sharing](https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-createfilew),
[ReplaceFile behavior and partial failures](https://learn.microsoft.com/en-us/windows/win32/api/winbase/nf-winbase-replacefilew).

## Recovery

Snapshots are local, beside the native configuration under `recovery/session-*`.
Each session holds a filesystem lock. Active sessions are excluded from recovery.
Changed Rope roots are coalesced off-thread: 750 ms quiet debounce, at most two
seconds during sustained editing. One atomic, synced JSON snapshot per document
contains its original path, disk baseline, cursor and text. Each document is limited
to 16 MiB; exceeding this limit produces a native error. Closing/discarding and
successful clean saves remove obsolete entries. Shutdown flushes pending cleanup.

Startup offers Recover, Discard or Later for abandoned snapshots. Recovery opens
dirty buffers without writing the original files. If the original changed, saving
uses the normal conflict workflow. A path already open is recovered as an untitled
copy, preserving both editor documents. Original snapshots remain until their
recovered copy is persisted successfully or explicitly saved/discarded. Interrupted
snapshot writes leave the previous committed JSON intact. Malformed and interrupted
snapshots are moved to unique `.quarantine` files in their original session. The
native error identifies the source and retained location. If quarantine fails, the
original stays in place and the error explains that failure. Other valid entries
continue to recover. Error text excludes malformed field values/document contents.
The 16 MiB decoded limit stays in place to bound memory and storage work; the JSON
read bound allows six-byte escaping per text byte. Oversized writes keep previous
snapshots and identify the affected document and unprotected latest edits.

There is no automatic age-based deletion of unsaved content. Old snapshots remain
until a user decision; empty abandoned session directories are cleaned up.

## Search and replacement

- Ctrl-F opens `find ` in the existing native command bar; Ctrl-H opens `replace `.
- `:find text` searches literally. `:find-regex pattern` searches a regular expression.
- F3 / Shift-F3 or `:find-next` / `:find-prev` navigate with wraparound.
- `:search-case on|off` enables/disables case sensitivity.
- `:search-word on|off` toggles Unicode word boundaries.
- `:search-regex on|off` changes interpretation of the current query.
- `:replace text` replaces the current/next match. Repeating it advances after the
  replacement. `:replace-all text` replaces all matches in one undo operation.
- `:search-clear` removes the query/highlights. Escape closes command entry.

Regex replacement uses Rust regex semantics: `$1`, `${1}`, `$name`, `${name}` and
`$$` for a literal dollar. Use braces when a following letter belongs to literal
replacement text. Literal replacement does not expand dollar references. Regex
look-around and backreferences in patterns are unsupported by the regex engine.
Empty search text clears matching; explicit zero-width regexes are supported.

Jobs operate on document/revision/query/options snapshots with cancellation
generations. Edits trigger a 150ms quiet debounce; repeated input supersedes pending
scans. Search does not hold the file-operation busy flag, so saves remain available.
Automatic results update counts/highlights without changing cursor, mode or command
input. Small unaffected highlight sets are projected during debounce; intersecting
or excessively large stale sets wait for fresh results. Navigation and replacement
always compute valid snapshot results. Invalid patterns show native errors without
repeated idle rescans. Cancellation is checked between matches, not within one
regex engine call.
Search/replace uses the existing command bar rather than a separate form panel.

## Comments and indentation

Ctrl-/ or `:Comment` toggles current/selected lines. Rust, JS/JSX, TS/TSX, C/C++ and
Python have line comment support. Standard JSON is rejected; `.jsonc` explicitly
allows `//`. Blank lines are preserved. A mixed selection is commented throughout;
an entirely commented selection is uncommented, removing at most one separator
space per marker. A selection change is one undo operation.

Tab indents selected lines; Shift-Tab unindents current/selected lines. `:Indent`
and `:Unindent` expose the same operations. Existing file indentation is detected
on the background open job. Spaces use a conservative common-width estimate;
tabs retain the configured tab stop. Global `tab_width` and `expand_tabs` are the
fallbacks. Set `detect_indentation = false` to force those global settings.
Enter preserves leading indentation and adds one level after common open block
delimiters or Python colons. Existing owned-pair newline behavior is preserved.
This is conservative indentation, not a language formatter.

## Availability

`Action::unavailable_reason` exhaustively classifies the native action enum.
Unsupported defaults are not bound. User assignments to unsupported actions fail
configuration validation. Explicit named commands explain unsupported features.
Splits, terminals, recent-file history, snippet browsing and tab-page operations
remain unavailable; document buffer navigation remains supported. No advanced Vim
operators, counts, macros, text objects or repeat machinery were added.
