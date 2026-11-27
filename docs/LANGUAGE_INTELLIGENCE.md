# Language intelligence and managed servers

Open `:LspManager` (also in Commands), type to filter, and select Install.
Install resolves the stable version, reports it before downloading, prepares an
isolated generation, verifies it, and performs an actual LSP initialize/shutdown
probe before activation. Installation does not change the editor selection.
Updates use the same pipeline. Failed updates retain the active receipt.
Uninstall requires confirmation and only removes marked FilaVim-owned generations.
Close other FilaVim instances before uninstalling tools they may still be using.

## Required catalog

| ID | Languages | Stable version source | Artifact verification |
|---|---|---|---|
| rust-analyzer | Rust | Official GitHub releases/latest | Published asset SHA-256 |
| clangd | C, C++ | Official clangd GitHub releases/latest | Published asset SHA-256 |
| typescript-language-server | JS, JSX, TS, TSX | Approved npm package/latest | Locked npm SHA-512 integrity |
| pyright | Python | Approved npm package/latest | Locked npm SHA-512 integrity |
| json-language-server | JSON, JSONC | vscode-json-languageserver/latest | Locked npm SHA-512 integrity |

Existing vtsls, TypeScript native, Basedpyright and Ruff entries remain available.
Explicit primary and custom executable preferences are preserved. An explicitly
configured custom primary takes precedence over the new defaults. Ruff remains
complementary; two primary servers are not deliberately activated together.

The catalog in `src/lsp_catalog.rs` is shipped with the application. It defines
sources, languages, extensions, root markers, platforms, entrypoints and arguments.
Remote metadata supplies versions, not executable recipes. Install always resolves
fresh metadata; the last successful update-check version is only a cached hint.
Offline catalog display and installed tools work; installation requires connectivity.

## Storage and runtimes

Windows storage is `%LOCALAPPDATA%/filavim/lsp`; `FILAVIM_LSP_HOME` overrides it.
Other platforms use the existing platform data-directory selection.
`state.json` holds installation receipts and global/workspace preferences.
`packages/<id>/generation-*` isolates versions; `runtimes/node` holds shared Node
generations and an atomic current pointer. Per-package runtime associations survive
restart. Transient progress and processes are not persisted.

A compatible system Node with npm is reused; otherwise the installer downloads
official Node 24 LTS, verifies SHASUMS256, and invokes its npm CLI explicitly.
The current conservative compatibility floor is Node 22.22.2. No global npm
installation, PATH modification, administrator elevation or project installation
is performed. npm configuration/cache are isolated; lifecycle scripts are disabled.
Dependencies are locked and checked for registry.npmjs.org artifact URLs and SHA-512
integrity before `npm ci`. TypeScript Language Server receives a managed TypeScript
6 fallback; project-local SDK selection remains server-controlled. TypeScript 7's
native implementation is a separate catalog alternative, not an interchangeable
TypeScript 6 API dependency.

`:LspPath server-id C:\path\to\server.exe` configures an external executable.
`:LspSettings server-id {"section":{...}}` persists workspace-scoped server settings.
External tools are identified separately and never uninstalled by FilaVim.
Settings are supplied through workspace/configuration and didChangeConfiguration.

Rust analysis needs Cargo/toolchains and potentially rust-src. clangd uses its
normal compile_commands.json/compile_flags.txt discovery; absent build flags can
produce incomplete analysis. Python third-party imports need an appropriate
project interpreter. JSON schema behavior depends on server configuration and
schema availability. Installing a server does not install project toolchains.

## Security and lifecycle boundaries

The curl downloader streams to staging, enforces HTTPS and an explicit host/path
policy on each redirect, checks exit status/size, reports bytes and known totals,
and terminates owned work on cancellation/timeouts. ZIP/tar extraction rejects
traversal, platform escape syntax, links/special files, duplicates, and excessive
entry counts or expanded sizes. Node convenience links are skipped. npm performs
its own integrity verification and archive handling. Published checksums protect
artifact consistency; they are not independent signatures against upstream account
compromise. Node's manifest is obtained over trusted HTTPS, not signature-verified.

A cross-process lock serializes mutations. Activation publishes a receipt only
after launch validation and initialization. Older generations are retained because
another process may be using them; uninstall is currently the cleanup mechanism.
There is no automatic rollback selector, runtime garbage collector, or bounded
old-generation retention policy yet. Download caches used by npm are temporary.

## Features and current limits

Incremental syntax parsing includes Rust, Python, JS/JSX, TS/TSX, C/C++, JSON/JSONC
independently of LSP installation. Completion retains revision-aware popup sessions.
Snippets support numbered/default/nested fields, mirrors, forward/reverse Tab and
final stop. Variables, choices and transformations are explicitly unsupported;
IME composition and conflicting edits cancel sessions safely.

Commands include Definition, Declaration, TypeDefinition, Implementation,
References, DocumentSymbols, WorkspaceSymbols (optional query), DiagnosticNext,
DiagnosticPrevious, CodeActions, Rename, Format and FormatRange.
Code actions can resolve, apply validated text edits, and execute advertised
commands after successful edits. Multi-file text edits remain unsaved editor changes.
Ordered text document changes are evaluated sequentially, including repeated edits
to one document, then applied as one undoable change per document. Previously closed
files retain their verified disk baseline when opened by a workspace edit, so later
saves still detect external changes.

The client advertises and handles `workspace/applyEdit` and ordered file create,
rename, and delete operations. Destructive operations, paths outside the workspace,
and annotations requiring consent use a native approval dialog. The planner checks paths and disk state before
execution. Approved resource changes can reach disk during execution. Text edits
remain unsaved and undoable in the affected buffers. If execution fails or is
cancelled mid-plan, completed operations can remain on disk. The client reconciles
the applied prefix and reports failures and any retained backup paths. The mock
and filesystem suites exercise these paths. The earlier five-tool real-server run
predates this implementation.

Dynamic registration is limited to completion. Crashes require explicit restart.
The Milestone 4 and 4.1 reports record earlier capability and validation states.

## Maintenance, tests and licensing

Add recipes to the typed catalog and shared installer, never rendering code.
Review project ownership, registry identity, supported release assets, integrity,
runtime requirements and licenses before adding a trusted source. Test unsupported
hosts/platforms and malicious archives, then run an opt-in actual-server acceptance
fixture. Default suites use isolated directories, mock LSP processes and local HTTP
fixtures; they never install servers from the internet. See [TESTING.md](TESTING.md).

New Rust dependencies are tar (MIT/Apache-2.0) and maintained tree-sitter TypeScript,
C and C++ grammars (MIT). Downloaded tools retain their upstream licenses and
notices: rust-analyzer MIT/Apache-2.0, LLVM/clangd Apache-2.0 with LLVM exceptions,
TypeScript Language Server MIT, Pyright/TypeScript MIT, Node MIT plus bundled
third-party notices. Inspect each resolved package/archive's LICENSE/NOTICE files.
On-demand download does not remove attribution or redistribution obligations;
bundling these artifacts in a future release requires a separate license review.
