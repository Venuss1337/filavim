# Native Language Server Manager

> Historical manager checkpoint. For current controls, catalog, and protocol behavior, see [Language intelligence and managed servers](docs/LANGUAGE_INTELLIGENCE.md).

Open with `Esc`, `:LspManager`, `Enter` (also listed in `:Commands`).
It is an in-window GPUI overlay using the existing Moon theme, font and popup layer.

## Implemented vertical slice

| Server | Installation | Default role | Windows version verified |
|---|---|---|---|
| rust-analyzer | Official GitHub binary, publisher SHA-256 digest | Rust primary | 2026-10-19 |
| vtsls | Isolated npm prefix, `@vtsls/language-server` | JS/TS compatibility-first primary | 0.3.0 |
| TypeScript native | Isolated npm prefix, stable `typescript` | JS/TS performance alternative | 7.0.2 |
| basedpyright | Isolated npm prefix, `basedpyright` | Python type analysis and navigation | 1.40.2 |
| Ruff | Private Python venv, SHA-256-verified binary wheel | Complementary Python lint/format/actions | 0.17.2 |

TypeScript native runs `tsc --lsp --stdio`; the preview package is never installed.
Selecting it replaces vtsls for the scope. vtsls keeps its compatible bundled SDK;
TypeScript 6/API/plugin workflows are not forced onto TypeScript 7.

The first catalog deliberately contains these five tested installers. Existing
custom/legacy C/C++, Go and JSON configurations still work through the original
client. Managed clangd, gopls, Lua, HTML/CSS/JSON, YAML, Taplo, Bash, JDTLS and Vue
installers are deferred; the UI does not advertise them as implemented.

## Controls

- Type to search; Up/Down select; Esc closes. Enter proposes installation;
  another Enter confirms downloading/running executable software. Esc cancels.
- F1/F2/F3/F4: All / Installed / Available / Updates. F5: local refresh.
- F6: explicitly fetch current stable versions. No automatic update polling.
- Ctrl-I / Ctrl-U: propose install-update / uninstall, with confirmation.
- Ctrl-E: enable/disable. Ctrl-P: use as primary. Ctrl-S: global/workspace scope.
- Ctrl-G / Ctrl-R / Ctrl-X: start / restart / stop relevant open-document instances.
- Ctrl-D: enable/disable LSP for the selected scope. F7: native executable picker.
- All these actions also have native buttons. Custom paths can be assigned with
  `:LspPath server-id absolute-executable-path`; default server arguments apply.
  Advanced custom argument arrays are recorded under `custom` in `state.json`.
  Windows `.cmd`/`.bat` launchers are not accepted as custom native executables.

Runtime state, installation/version state, and active installation progress are
separate. Details include resolved command/arguments, roots, initialize duration,
last failure, advertised capabilities and bounded recent stderr. An advertised
capability is not presented as a verified UI feature. Existing native completion,
hover, diagnostic, reference and code-action popups are reused.

## Ownership, activation and persistence

Windows state and private packages are under `%LOCALAPPDATA%\filavim\lsp`.
macOS uses `~/Library/Application Support/filavim/lsp`; Linux uses
`$XDG_DATA_HOME/filavim/lsp` or `~/.local/share/filavim/lsp`.
`FILAVIM_LSP_HOME` overrides this location for isolated verification.

`state.json` atomically stores owned-install receipts, global preferences, and
workspace overrides keyed by normalized workspace path. Existing `config.toml`
is preserved; explicitly configured custom servers remain supported until an
explicit manager primary choice for that language overrides them. Configuration
must be resolved before starting processes, so a persisted disable is respected.

On file open, a short worker job resolves matching tools and language-specific root
markers. Processes are keyed by server AND root and reused across matching files.
One primary is selected per language; Ruff may run alongside basedpyright. Formatting
and quick fixes prefer Ruff where advertised; completion/navigation prefer the
primary. Missing recommended tools produce a dismissible native notice. Dismissals
persist per workspace. Nothing is downloaded merely by opening a file or manager.

Custom paths and compatible project-local packages take priority where available;
owned copies and PATH resolution are supported. All filesystem resolution, package
downloads, installation and version probes run outside GPUI rendering/input code.
Only a single manager operation runs at once; OS file locking prevents concurrent
Filavim processes from overwriting tool metadata. Completed jobs wake GPUI; idle
operation does not use a polling timer or permanent installer thread.

Install/update uses a new generation directory. npm lifecycle scripts are disabled;
pip uses only binary wheels with explicit published SHA-256 hashes. npm/pip caches
are isolated too. A real initialize/shutdown probe must succeed before the atomic
metadata pointer changes. Old generations remain usable until explicit uninstall,
so a running process is not disrupted or replaced underneath itself. Failed or
cancelled preparation retains the preceding active installation. Uninstall verifies
both an owned receipt and canonical generation location and never removes external
or project-local executables. Locked-file failures remain explicit and retryable.

No runtime installation, PATH changes, global npm/pip installs, or administrator
operations are performed. Node >=20 plus npm is required for the Node installers;
Python with venv/pip is required for Ruff installation. Rust analysis needs a suitable
Rust toolchain; the manager does not silently install Cargo/rust-src.

## Validation and limits

Approved temporary-directory checks installed, initialized, updated/replaced and
removed real tools. All five installers passed; a vtsls replacement retained its
old generation until uninstall. Stable-version checks passed for every entry.
vtsls and TypeScript 7 returned `property` completion through native Services;
basedpyright returned `upper` completion while Ruff published diagnostics.
The existing real rust-analyzer test passed completion acceptance, hover and
diagnostics. These are protocol/editor integration checks, not graphical validation.

Mock tests cover two complementary servers, two files sharing an instance,
independent roots, completion/formatting routing, stop/start and crash handling.
Unit coverage includes recommendation exclusivity, workspace persistence, receipts,
path safety, cancellation, missing dependencies, malformed metadata and preservation
of prior installation metadata. No permission/elevation settings were changed.

Client limitations preserved from the existing implementation: server-command-only
code actions and unsolicited `workspace/applyEdit` are rejected; direct-edit code
actions, rename and formatting use the existing revision-checked edit planner.
Automatic crash restart is deliberately disabled; Start/Restart retries explicitly.
Version checks are explicit, and update availability is a cached observation for
the current manager snapshot. There is no version pin/rollback chooser UI; retained
generations support safe replacement, not a user-facing rollback browser.
Manager search is key-based; IME composition in that search field is not implemented.
Only Windows x86_64 was exercised. macOS/Linux/ARM64 paths and recipes are unverified;
the rust-analyzer Linux binary recipe targets GNU, not musl.

## Manual review

1. Launch `target\release\filavim.exe`, open a Rust file, run `:LspManager`.
2. Search `rust`, inspect the real executable/status/logs. Esc returns to editing.
3. Select a missing tool, Install, inspect the confirmation, then confirm or cancel.
4. Use F6 and the Updates filter; check that versions are reported independently
   of Running/Stopped and no installation occurs without confirmation.
5. Open two Python files in a project. Enable basedpyright/Ruff and verify both
   instances, completion, diagnostics, formatting, and Stop/Restart.
6. Switch JS/TS primary between vtsls and TypeScript native for workspace scope;
   verify one primary runs, and an unrelated workspace keeps its own preference.
7. Close/reopen the manager, resize the window, use keyboard filters and buttons;
   inspect focus, scrolling, confirmation layout and narrow-window behavior.

Build/test outcomes are summarized in the task report. Visual appearance and
physical keyboard/mouse interaction require user review.

## Sources reviewed

- https://github.com/mason-org/mason-registry
- https://github.com/mason-org/mason.nvim
- https://github.com/neovim/nvim-lspconfig
- https://zed.dev/docs/configuring-languages
- https://rust-analyzer.github.io/book/installation.html
- https://github.com/yioneko/vtsls
- https://devblogs.microsoft.com/typescript/announcing-typescript-7-0/
- https://docs.basedpyright.com/latest/installation/command-line-and-language-server/
- https://docs.astral.sh/ruff/installation/

### Final verification commands (Windows x86_64)

- `cargo check --locked`: passed (initial checks also used offline resolution).
- Scoped `rustfmt --check --edition 2024` on catalog, manager, native manager view,
  services, transport and manager integration tests: passed. Full-repository
  formatting was not used to rewrite unrelated pending migration work.
- `cargo clippy --locked --all-targets -- -D warnings`: passed.
- `cargo test --locked --lib lsp_manager::tests -- --test-threads=1`: 8 passed,
  download/execute test ignored by default.
- `cargo test --locked --test lsp_manager -- --test-threads=1`: 3 passed.
- Checkpoint 2 integration tests: 9 passed; ordinary floating tests: 2 passed.
- With explicit consent, `confirmed_five_installers_round_trip --ignored` passed:
  all five real installs/initialize/shutdown/removals, stable-version checks,
  vtsls replacement update, and native Services completion/diagnostic checks.
- Existing real rust-analyzer completion/hover/diagnostic test, explicitly selected
  with `--ignored`, passed. The two real-server tests are separate from the 22
  ordinary focused/regression tests and are not run automatically.
- `cargo build --locked` and `cargo build --release --locked`: passed.
- Release startup-only launch: native main window detected; left open for review.

No graphical interaction or visual verification is claimed. No commit, push or merge
was made. Existing terminal-worktree files, including its unrelated untracked file,
were left untouched. Temporary installation fixtures were removed.

Root selection currently uses the nearest declared project marker. Independent
roots are tested, but there is no shared multi-root LSP session or Cargo workspace
membership resolver: opening separate crate/package roots can create separate
instances. More advanced monorepo consolidation and configurable root rules remain
future work. Missing runtime executables are reported; runtime installation is manual.
