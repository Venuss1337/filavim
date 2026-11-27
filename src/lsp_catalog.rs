//! Curated executable software sources, not an executable external registry.
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Method {
    GithubRust,
    GithubClang,
    Npm(&'static str),
    Python(&'static str),
}
#[derive(Clone, Copy, Debug)]
pub struct Entry {
    pub id: &'static str,
    pub name: &'static str,
    pub languages: &'static [&'static str],
    pub extensions: &'static [&'static str],
    pub roots: &'static [&'static str],
    pub platforms: &'static [&'static str],
    pub architectures: &'static [&'static str],
    pub update_source: &'static str,
    pub source: &'static str,
    pub method: Method,
    pub executable: &'static str,
    pub version_executable: &'static str,
    pub args: &'static [&'static str],
    pub recommended: bool,
    pub complementary: bool,
    pub runtime: &'static str,
    pub notes: &'static str,
}
const TS: &[&str] = &[
    "typescript",
    "javascript",
    "typescriptreact",
    "javascriptreact",
];
const TS_ROOTS: &[&str] = &["tsconfig.json", "jsconfig.json", "package.json", ".git"];
pub const CATALOG: &[Entry] = &[
    Entry {
        id: "rust-analyzer",
        name: "Rust · rust-analyzer",
        languages: &["rust"],
        extensions: &["rs"],
        roots: &["Cargo.toml", "rust-project.json", ".git"],
        platforms: &["windows", "macos", "linux"],
        architectures: &["x86_64", "aarch64"],
        update_source: "https://api.github.com/repos/rust-lang/rust-analyzer/releases/latest",
        source: "https://github.com/rust-lang/rust-analyzer",
        method: Method::GithubRust,
        executable: "rust-analyzer",
        version_executable: "rust-analyzer",
        args: &[],
        recommended: true,
        complementary: false,
        runtime: "Rust toolchain / Cargo and rust-src for standard-library analysis; curl for installation",
        notes: "Official release executable, SHA-256 verified against GitHub's asset digest. Windows/macOS/Linux x86_64 and aarch64; Linux GNU. No automatic rustup changes.",
    },
    Entry {
        id: "vtsls",
        name: "TypeScript / JavaScript · vtsls",
        languages: TS,
        extensions: &["ts", "tsx", "mts", "cts", "js", "jsx", "mjs", "cjs"],
        roots: TS_ROOTS,
        platforms: &["windows", "macos", "linux"],
        architectures: &["x86_64", "aarch64"],
        update_source: "https://registry.npmjs.org/@vtsls%2flanguage-server/latest",
        source: "https://github.com/yioneko/vtsls",
        method: Method::Npm("@vtsls/language-server"),
        executable: "vtsls",
        version_executable: "vtsls",
        args: &["--stdio"],
        recommended: false,
        complementary: false,
        runtime: "Node.js >= 20 and npm; supported maintained Node LTS recommended",
        notes: "Compatibility-first default: wraps VS Code's TypeScript service. Uses its bundled SDK by default; existing TypeScript 6/plugin projects can keep their SDK. Not an objective performance ranking.",
    },
    Entry {
        id: "typescript-native",
        name: "TypeScript / JavaScript · TypeScript 7 native",
        languages: TS,
        extensions: &["ts", "tsx", "mts", "cts", "js", "jsx", "mjs", "cjs"],
        roots: TS_ROOTS,
        platforms: &["windows", "macos", "linux"],
        architectures: &["x86_64", "aarch64"],
        update_source: "https://registry.npmjs.org/typescript/latest",
        source: "https://www.typescriptlang.org/download/",
        method: Method::Npm("typescript"),
        executable: "tsc",
        version_executable: "tsc",
        args: &["--lsp", "--stdio"],
        recommended: false,
        complementary: false,
        runtime: "Node.js >= 20 and npm to install/launch the stable platform package",
        notes: "Performance alternative: stable typescript >= 7, never native-preview. Native LSP does not replace the TypeScript 6 programmatic API required by some Vue/Angular/plugins. Selecting this replaces vtsls for this scope.",
    },
    Entry {
        id: "basedpyright",
        name: "Python · basedpyright",
        languages: &["python"],
        extensions: &["py", "pyi"],
        roots: &[
            "pyrightconfig.json",
            "pyproject.toml",
            "setup.cfg",
            "setup.py",
            ".git",
        ],
        platforms: &["windows", "macos", "linux"],
        architectures: &["x86_64", "aarch64"],
        update_source: "https://registry.npmjs.org/basedpyright/latest",
        source: "https://docs.basedpyright.com/latest/installation/command-line-and-language-server/",
        method: Method::Npm("basedpyright"),
        executable: "basedpyright-langserver",
        version_executable: "basedpyright",
        args: &["--stdio"],
        recommended: false,
        complementary: false,
        runtime: "Node.js >= 20 and npm; project Python interpreter for environment-specific imports",
        notes: "Type analysis, completion and navigation. Official npm distribution in an isolated prefix. Ruff supplies complementary lint diagnostics, formatting and quick fixes.",
    },
    Entry {
        id: "ruff",
        name: "Python · Ruff",
        languages: &["python"],
        extensions: &["py", "pyi"],
        roots: &["pyproject.toml", "ruff.toml", ".ruff.toml", ".git"],
        platforms: &["windows", "macos", "linux"],
        architectures: &["x86_64", "aarch64"],
        update_source: "https://api.github.com/repos/astral-sh/ruff/releases/latest",
        source: "https://docs.astral.sh/ruff/installation/",
        method: Method::Python("ruff"),
        executable: "ruff",
        version_executable: "ruff",
        args: &["server"],
        recommended: true,
        complementary: true,
        runtime: "Python >= 3.8 with venv and pip for installation; Ruff's executable is native",
        notes: "Lint diagnostics, code actions and formatting alongside basedpyright. Binary wheels only: no source-build scripts. Does not replace the Python type-analysis server.",
    },
    Entry {
        id: "clangd",
        name: "C / C++ · clangd",
        languages: &["c", "cpp"],
        extensions: &["c", "h", "cc", "cpp", "cxx", "hpp", "hh"],
        roots: &[
            "compile_commands.json",
            "compile_flags.txt",
            "CMakeLists.txt",
            ".git",
        ],
        platforms: &["windows", "macos", "linux"],
        architectures: &["x86_64"],
        update_source: "https://api.github.com/repos/clangd/clangd/releases/latest",
        source: "https://github.com/clangd/clangd",
        method: Method::GithubClang,
        executable: "clangd",
        version_executable: "clangd",
        args: &[],
        recommended: true,
        complementary: false,
        runtime: "Native executable; compilation database recommended",
        notes: "Official ZIP release with published asset SHA-256. Missing compile_commands.json limits project-specific semantic results.",
    },
    Entry {
        id: "typescript-language-server",
        name: "TypeScript / JavaScript · TypeScript Language Server",
        languages: TS,
        extensions: &["ts", "tsx", "mts", "cts", "js", "jsx", "mjs", "cjs"],
        roots: TS_ROOTS,
        platforms: &["windows", "macos", "linux"],
        architectures: &["x86_64", "aarch64"],
        update_source: "https://registry.npmjs.org/typescript-language-server/latest",
        source: "https://github.com/typescript-language-server/typescript-language-server",
        method: Method::Npm("typescript-language-server"),
        executable: "typescript-language-server",
        version_executable: "typescript-language-server",
        args: &["--stdio"],
        recommended: true,
        complementary: false,
        runtime: "Managed Node LTS and isolated npm; project TypeScript preferred",
        notes: "A managed TypeScript 6 fallback is provided; project-local tsserver remains preferred.",
    },
    Entry {
        id: "pyright",
        name: "Python · Pyright",
        languages: &["python"],
        extensions: &["py", "pyi", "pyw"],
        roots: &["pyrightconfig.json", "pyproject.toml", "setup.cfg", ".git"],
        platforms: &["windows", "macos", "linux"],
        architectures: &["x86_64", "aarch64"],
        update_source: "https://registry.npmjs.org/pyright/latest",
        source: "https://github.com/microsoft/pyright",
        method: Method::Npm("pyright"),
        executable: "pyright-langserver",
        version_executable: "pyright",
        args: &["--stdio"],
        recommended: true,
        complementary: false,
        runtime: "Managed Node LTS; project Python for environment-specific imports",
        notes: "Type analysis without installing Python globally. Select a project interpreter for third-party imports.",
    },
    Entry {
        id: "json-language-server",
        name: "JSON / JSONC · JSON Language Server",
        languages: &["json", "jsonc"],
        extensions: &["json", "jsonc"],
        roots: &["package.json", ".git"],
        platforms: &["windows", "macos", "linux"],
        architectures: &["x86_64", "aarch64"],
        update_source: "https://registry.npmjs.org/vscode-json-languageserver/latest",
        source: "https://github.com/microsoft/vscode/tree/main/extensions/json-language-features/server",
        method: Method::Npm("vscode-json-languageserver"),
        executable: "vscode-json-languageserver",
        version_executable: "vscode-json-languageserver",
        args: &["--stdio"],
        recommended: true,
        complementary: false,
        runtime: "Managed Node LTS",
        notes: "Published vscode-json-languageserver distribution. Remote schema retrieval is controlled by server settings.",
    },
];
pub fn entry(id: &str) -> Option<&'static Entry> {
    CATALOG.iter().find(|e| e.id == id)
}
pub fn root(path: &Path, entry: &Entry) -> PathBuf {
    let start = path.parent().unwrap_or(path);
    start
        .ancestors()
        .find(|p| entry.roots.iter().any(|m| p.join(m).exists()))
        .unwrap_or(start)
        .to_path_buf()
}
pub fn primary(language: &str) -> Option<&'static Entry> {
    CATALOG
        .iter()
        .find(|e| e.recommended && !e.complementary && e.languages.contains(&language))
}

impl Entry {
    pub fn download_page(&self) -> String {
        match self.method {
            Method::GithubRust | Method::GithubClang => format!("{}/releases", self.source),
            Method::Npm(package) => format!("https://www.npmjs.com/package/{package}"),
            Method::Python(package) => format!("https://pypi.org/project/{package}/#files"),
        }
    }

    pub fn supports_host(&self) -> bool {
        self.supports(std::env::consts::OS, std::env::consts::ARCH)
    }
    pub fn supports(&self, os: &str, architecture: &str) -> bool {
        self.platforms.contains(&os) && self.architectures.contains(&architecture)
    }
    pub fn initialization_options(&self) -> serde_json::Value {
        if self.id == "ruff" {
            serde_json::json!({"settings":{}})
        } else {
            serde_json::Value::Null
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn catalog_ids_defaults_and_language_associations_are_consistent() {
        let mut ids = std::collections::BTreeSet::new();
        let mut defaults = std::collections::BTreeSet::new();
        for entry in CATALOG {
            assert!(ids.insert(entry.id));
            assert!(!entry.executable.is_empty());
            assert!(!entry.extensions.is_empty());
            let source = url::Url::parse(entry.source).unwrap();
            assert_eq!(source.scheme(), "https");
            assert!(source.username().is_empty());
            assert!(!entry.supports("unknown", "x86_64"));
            assert!(!entry.supports("windows", "unknown"));
            if entry.recommended && !entry.complementary {
                for language in entry.languages {
                    assert!(defaults.insert(language), "duplicate primary: {language}");
                }
            }
            for extension in entry.extensions {
                assert!(
                    crate::lsp::language(Path::new(&format!("file.{extension}"))).is_some(),
                    "{extension}"
                );
            }
        }
        assert_eq!(
            primary("typescript").unwrap().id,
            "typescript-language-server"
        );
        assert_eq!(primary("python").unwrap().id, "pyright");
        assert!(!entry("clangd").unwrap().supports("windows", "aarch64"));
    }
    #[test]
    #[ignore = "opt-in release catalog filtering and snippet parsing timings"]
    fn language_intelligence_operation_timings() {
        use std::{hint::black_box, time::Instant};
        let start = Instant::now();
        for _ in 0..10000 {
            let matches: Vec<_> = CATALOG
                .iter()
                .filter(|e| {
                    format!("{} {}", e.name, e.languages.join(" "))
                        .to_lowercase()
                        .contains(black_box("type"))
                })
                .collect();
            black_box(matches);
        }
        println!(
            "Catalog: {} entries, 10000 filters, {} us total",
            CATALOG.len(),
            start.elapsed().as_micros()
        );
        let source = "fn ${1:name}(${2:arg}) {\n    $1($2);\n    $0\n}";
        let start = Instant::now();
        for _ in 0..10000 {
            black_box(crate::snippet::expand(black_box(source)).unwrap());
        }
        println!(
            "Snippet: {} bytes, 10000 parses, {} us total",
            source.len(),
            start.elapsed().as_micros()
        );
    }
}
