# Milestone 4.1 — partial implementation

Historical report. The checkout details, capabilities, and validation below describe that checkpoint.

This is a progress record, not a completion report. Milestone 4.1 is unfinished.

## Changes

The workspace-edit planner now retains verified disk state for previously closed
files. Native application transfers this state to the new document before editing.
The document remains dirty, disk remains unchanged, and subsequent saving uses the
original conflict baseline. A regression exercises successful saving and rejection
of external changes using real temporary files.

Ordered text documentChanges now evaluates repeated edits against preceding
results, validates versions against the originating open snapshot, and publishes
one undoable edit per affected document. A Unicode regression checks sequential
coordinates and undo. WorkspaceEdit cannot specify both changes and documentChanges.
Unknown annotations and annotations requiring confirmation are rejected explicitly
until an approval workflow exists; they are not silently applied.

Changed implementation: src/lsp.rs and src/gui/services.rs. Regression coverage:
tests/checkpoint2.rs. Existing ownership, async preparation and save primitives
remain in use. No dependencies or catalog entries were added.

## Validation

- Baseline `pwsh -NoProfile -File scripts/test.ps1 -Mode Fast`: passed.
- Focused `cargo test --locked --no-default-features --test checkpoint2 workspace_edit -- --test-threads=2`:
  2 passed, 0 failed, 12 filtered out (before adding the ordered-edit regression).
- `pwsh -NoProfile -File scripts/test.ps1 -Mode Full`: 189 passed, 0 failed,
  13 ignored. Formatting, warnings-denied Clippy and headless GPUI are included.
  Report: target/test-reports/20261116-213015-816-full/report.json.
- Scoped `git diff --check -- src/lsp.rs src/gui/services.rs tests/checkpoint2.rs`:
  passed; Git emitted line-ending notices only.
- Final `pwsh -NoProfile -File scripts/test.ps1 -Mode Fast`: 131 passed,
  0 failed, 9 ignored. Final `-Mode LSP`: 21 passed, 0 failed, 1 ignored.
- `cargo build --locked`: native debug build and link passed.

No Computer Use or real graphical application interaction was performed. No new
real-server acceptance or performance measurements were performed in this increment.
The previous five-server results remain historical evidence only.

## Outstanding acceptance work

Resource CreateFile/RenameFile/DeleteFile, destructive/out-of-workspace approval,
annotation approval, ordered mixed transactions and partial failure reconciliation
remain unimplemented. workspace/applyEdit remains unadvertised and rejected.
Known-good version history, explicit rollback, dependency-aware cleanup, managed
runtime repair, bounded crash retries and native manager controls remain unfinished.
The expanded capability-aware real-server acceptance matrix is not implemented.
Milestone 5 readiness has therefore not been established.
