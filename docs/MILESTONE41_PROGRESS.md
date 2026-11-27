# Milestone 4.1 — workspace edits and file safety

This report covers the workspace-edit and file-safety acceptance work on native FilaVim.
The earlier checkpoint implemented ordered text edits and retained closed-file
baselines. The current client also advertises `workspace/applyEdit` and handles
ordered create, rename, and delete operations.

## Behavior and safety

- Both planners reject malformed edit envelopes and ambiguous requests containing
  `changes` and `documentChanges`. Text ranges and annotations use shared validation.
  Ordered edits use the coordinates of the preceding result and reject stale versions.
- Destructive operations, out-of-workspace paths, and annotated changes requiring
  consent use the native plan review. Rejection changes neither buffers nor files.
  The planner checks path components, file identities, and disk baselines again
  before execution. Resource operations cannot modify a filesystem root, the active
  workspace root, or its ancestors.
- Text edits remain unsaved and undoable as one group per document. A successful
  resource step has already changed the filesystem. Open buffers affected by a
  deletion or replacement are preserved as dirty untitled documents where applicable.
- Execution stops on cancellation or failure. The client reports the failed index
  as zero-based `failedChange` for ordered edits and identifies retained disk
  effects separately from unsaved text. It reconciles completed resource steps
  even if a text buffer cannot be updated.
- Moves to backup retain the original and a synced `SOURCE.txt` manifest before
  subsequent verification. Backups stay outside directories moved by the plan.
  To recover, read `SOURCE.txt` and inspect `original` in the reported backup
  directory. Copy or move it to a vacant path and compare it with the current file
  before deciding which version to keep.
- A tracked open verifies that the loaded text matches its disk baseline. A tracked
  save verifies the staged file identity and expected bytes before acknowledging
  the save. An injected storage-full error after a partial staging write verifies
  the original and its baseline survive without leaving a partial save file.

## Acceptance

Windows x64 Full passed with 221 tests, 0 failures and 15 opt-in tests ignored.
It includes formatting, warnings-denied Clippy, core and headless GPUI tests.
The Windows desktop smoke passed with the debug executable and verified that the
opened fixture remained unchanged. The workspace fixtures cover ordered mixed operations, case-only renames,
stale plans, junction escapes, permission failures, committed prefixes, backup
recovery, and dirty buffer reconciliation. File and recovery tests cover timestamp-
preserving external updates, same-content replacements, sharing violations, interrupted
writes, version conflicts, process termination, and the 16 MiB recovery limit.

Opt-in real rust-analyzer 1.93.1 acceptance on Windows x64 exercised native editor
services. The create-module action created `src/helper.rs`, and the running server
resolved the module definition without changing the source file or buffer identity.
A subsequent symbol rename updated both occurrences in the editor, left disk
unchanged, and reverted in one undo. Run it with:

```powershell
cargo test --locked --features gpui-tests,perf --test checkpoint2 real_rust_analyzer_create_module_resource_action -- --ignored --nocapture --test-threads=1
```

See [Testing FilaVim](TESTING.md) for the standard verification profiles.

## Limits

Workspace edits are abort-on-failure operations. A completed disk change is not
reversed by editor undo. Recovery from retained backups is manual. The controlled
storage-full test simulates a write error without filling a volume. Power loss,
all filesystem variants, and arbitrary races between a path check and an OS
operation remain outside this evidence. Windows sharing and replacement checks
reduce that window. Other platforms retain the filesystem limitations described in
[Editing and file safety](EDITING_AND_SAFETY.md).

Headless GPUI tests and the real-server acceptance cover application integration.
Physical keyboard, IME, and accessibility interactions require hands-on review.
Linux and macOS run core CI. Native desktop acceptance is scoped to Windows x64.
Server generation rollback, dependency-aware cleanup, runtime repair, and automatic
crash retries are outside this workspace-edit and file-safety acceptance scope.
