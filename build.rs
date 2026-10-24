fn main() {
    println!("cargo:rerun-if-env-changed=CARGO_FEATURE_DEV_OPERATOR_FONT");
    if std::env::var_os("CARGO_FEATURE_DEV_OPERATOR_FONT").is_some() {
        for name in [
            "CI",
            "GITHUB_ACTIONS",
            "TF_BUILD",
            "RUSTC_WRAPPER",
            "RUSTC_WORKSPACE_WRAPPER",
        ] {
            println!("cargo:rerun-if-env-changed={name}");
            assert!(
                std::env::var_os(name).is_none_or(|v| v.is_empty()
                    || (name == "RUSTC_WORKSPACE_WRAPPER"
                        && std::path::Path::new(&v)
                            .file_stem()
                            .is_some_and(|s| s == "clippy-driver"))),
                "dev-operator-font is private development only: unset {name} and use an uncached local build. CI and caching compiler wrappers must not receive proprietary embedded data."
            );
        }
        let asset = ".local/fonts/Operator Mono Medium Nerd Font Complete.otf";
        println!("cargo:rerun-if-changed={asset}");
        let bytes = std::fs::read(asset).unwrap_or_else(|e| {
            panic!("dev-operator-font requires the private local asset at {asset}: {e}. See FONT_SETUP.md. Disable this feature for ordinary builds; never distribute binaries built with it.")
        });
        assert!(
            bytes.starts_with(b"OTTO"),
            "dev-operator-font: asset is not an OpenType OTF font; see FONT_SETUP.md"
        );
    }
}
