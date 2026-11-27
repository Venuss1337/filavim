//! The full suite builds the tiny fixture once through Cargo. Direct cargo test
//! without test-tools retains a self-contained, temporary rustc fallback.
pub fn tempdir() -> tempfile::TempDir {
    // macOS TMPDIR can be under /var, an alias for /private/var. Use the
    // canonical parent just as opening a project resolves its on-disk path.
    let parent = std::env::temp_dir().canonicalize().unwrap();
    tempfile::tempdir_in(parent).unwrap()
}

mod trusted_manager;
pub use trusted_manager::trusted_manager;

pub fn mock_executable(directory: &std::path::Path) -> std::path::PathBuf {
    #[cfg(feature = "test-tools")]
    {
        let _ = directory;
        std::path::PathBuf::from(env!("CARGO_BIN_EXE_filavim-mock-lsp"))
    }
    #[cfg(not(feature = "test-tools"))]
    {
        let executable = directory.join(if cfg!(windows) { "mock.exe" } else { "mock" });
        let output = std::process::Command::new("rustc")
            .args(["--edition=2024", "tests/fixtures/mock_lsp.rs", "-o"])
            .arg(&executable)
            .output()
            .expect("rustc is required for the mock LSP fixture");
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        executable
    }
}
