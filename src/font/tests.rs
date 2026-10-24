use super::*;
use std::cell::Cell;

// Minimal, original test font: no proprietary data is needed by ordinary tests.
fn fixture() -> Vec<u8> {
    fn put(b: &mut [u8], at: usize, value: u16) {
        b[at..at + 2].copy_from_slice(&value.to_be_bytes());
    }
    let mut head = vec![0; 54];
    put(&mut head, 18, 1000);
    let mut hhea = vec![0; 36];
    hhea[0] = 1;
    put(&mut hhea, 34, 1);
    let mut maxp = vec![0; 6];
    maxp[..4].copy_from_slice(&0x5000u32.to_be_bytes());
    put(&mut maxp, 4, 1);
    let mut os2 = vec![0; 78];
    put(&mut os2, 4, 350);
    let names = [
        (1, "Fixture Legacy"),
        (2, "Medium"),
        (4, "Fixture Full"),
        (6, "Fixture-Medium"),
        (16, "Fixture Nerd Font"),
        (17, "Medium"),
    ];
    let offset = 6 + names.len() * 12;
    let mut name = vec![0; offset];
    put(&mut name, 2, names.len() as u16);
    put(&mut name, 4, offset as u16);
    for (i, (id, s)) in names.iter().enumerate() {
        let bytes: Vec<u8> = s.encode_utf16().flat_map(u16::to_be_bytes).collect();
        let at = 6 + i * 12;
        for (j, v) in [
            3,
            1,
            1033,
            *id,
            bytes.len() as u16,
            (name.len() - offset) as u16,
        ]
        .iter()
        .enumerate()
        {
            put(&mut name, at + j * 2, *v);
        }
        name.extend(bytes);
    }
    let tables = [
        (*b"head", head),
        (*b"hhea", hhea),
        (*b"maxp", maxp),
        (*b"OS/2", os2),
        (*b"name", name),
        (*b"loca", vec![0; 4]),
        (*b"glyf", vec![0; 12]),
        (*b"hmtx", vec![0; 4]),
    ];
    let mut data = vec![0; 12 + tables.len() * 16];
    data[..4].copy_from_slice(&0x10000u32.to_be_bytes());
    put(&mut data, 4, tables.len() as u16);
    for (i, (tag, bytes)) in tables.iter().enumerate() {
        let at = 12 + i * 16;
        data[at..at + 4].copy_from_slice(tag);
        let offset = data.len() as u32;
        data[at + 8..at + 12].copy_from_slice(&offset.to_be_bytes());
        data[at + 12..at + 16].copy_from_slice(&(bytes.len() as u32).to_be_bytes());
        data.extend(bytes);
    }
    data
}
fn sandbox(root: &Path) -> Paths {
    let fonts = root.join("fonts");
    fs::create_dir_all(&fonts).unwrap();
    Paths {
        fonts: fonts.clone(),
        state: root.join("state"),
        search: vec![fonts],
    }
}
#[derive(Default)]
struct Mock {
    registrations: Cell<usize>,
    removals: Cell<usize>,
    fail: bool,
    fail_remove: bool,
}
impl Registration for Mock {
    fn register(&self, _: &Path, _: &Metadata) -> Result<String> {
        self.registrations.set(self.registrations.get() + 1);
        ensure!(!self.fail, "injected registry permission error");
        Ok("mock registration".into())
    }
    fn unregister(&self, _: &Path, _: &Metadata) -> Result<String> {
        self.removals.set(self.removals.get() + 1);
        ensure!(!self.fail_remove, "injected unregister failure");
        Ok(String::new())
    }
}
#[test]
fn metadata_uses_typographic_names_and_rejects_corruption() {
    let m = metadata(&fixture()).unwrap();
    assert_eq!(m.family, "Fixture Nerd Font");
    assert_eq!(m.legacy_family, "Fixture Legacy");
    assert_eq!(m.style, "Medium");
    assert_eq!(m.weight, 350);
    assert_eq!(m.postscript, "Fixture-Medium");
    assert!(!m.nerd_glyphs);
    for bytes in [
        b"".as_slice(),
        b"<html>not a font</html>",
        b"OTTO",
        &fixture()[..100],
    ] {
        assert!(metadata(bytes).is_err());
    }
}
#[cfg(not(feature = "dev-operator-font"))]
#[test]
fn default_provider_requires_no_asset() {
    assert!(bundled().is_none());
}
#[cfg(feature = "dev-operator-font")]
#[test]
fn real_private_asset_matches_verified_metadata() {
    let bytes = bundled().unwrap();
    assert!(bytes.starts_with(b"OTTO"));
    assert_eq!(bytes.len(), 927952);
    let m = metadata(bytes).unwrap();
    let mut expected = reference();
    expected.nerd_glyphs = m.nerd_glyphs;
    assert_eq!(m, expected);
    eprintln!("Private asset metadata: {m:?}; SHA256 {}", hash(bytes));
}
#[test]
fn installation_is_idempotent_and_owned_removal_is_verified() {
    let dir = tempfile::tempdir().unwrap();
    let paths = sandbox(dir.path());
    let registration = Mock::default();
    let bytes = fixture();
    install(&paths, &bytes, &registration).unwrap();
    let file = paths.fonts.join(FILE);
    let before = fs::metadata(&file).unwrap().modified().unwrap();
    assert!(
        install(&paths, &bytes, &registration)
            .unwrap()
            .contains("already installed")
    );
    assert_eq!(before, fs::metadata(&file).unwrap().modified().unwrap());
    assert!(
        find_installed(&paths, &metadata(&bytes).unwrap())
            .unwrap()
            .is_some()
    );
    remove(&paths, &registration).unwrap();
    assert!(!file.exists());
    assert!(load(&paths).unwrap().is_none());
    assert!(remove(&paths, &registration).is_err());
}
#[test]
fn preexisting_installation_is_not_adopted_or_removed() {
    let dir = tempfile::tempdir().unwrap();
    let paths = sandbox(dir.path());
    let file = paths.fonts.join("existing.otf");
    fs::write(&file, fixture()).unwrap();
    let registration = Mock::default();
    assert!(
        install(&paths, &fixture(), &registration)
            .unwrap()
            .contains("pre-existing")
    );
    assert_eq!(registration.registrations.get(), 0);
    assert!(load(&paths).unwrap().is_none());
    assert!(remove(&paths, &registration).is_err());
    assert!(file.exists());
}
#[test]
fn modified_managed_font_is_never_overwritten_or_removed() {
    let dir = tempfile::tempdir().unwrap();
    let paths = sandbox(dir.path());
    let registration = Mock::default();
    install(&paths, &fixture(), &registration).unwrap();
    fs::write(paths.fonts.join(FILE), b"user replacement").unwrap();
    assert!(install(&paths, &fixture(), &registration).is_err());
    assert!(remove(&paths, &registration).is_err());
    assert_eq!(registration.removals.get(), 0);
}
#[test]
fn registration_failure_rolls_back_and_incomplete_rollback_keeps_ownership() {
    let dir = tempfile::tempdir().unwrap();
    let paths = sandbox(dir.path());
    let registration = Mock {
        fail: true,
        ..Default::default()
    };
    assert!(install(&paths, &fixture(), &registration).is_err());
    assert!(!paths.fonts.join(FILE).exists());
    assert!(load(&paths).unwrap().is_none());
    let registration = Mock {
        fail: true,
        fail_remove: true,
        ..Default::default()
    };
    assert!(install(&paths, &fixture(), &registration).is_err());
    assert!(paths.fonts.join(FILE).exists());
    assert!(load(&paths).unwrap().is_some());
}
#[test]
fn interrupted_managed_installation_and_removal_can_recover_missing_files() {
    let dir = tempfile::tempdir().unwrap();
    let paths = sandbox(dir.path());
    let registration = Mock::default();
    install(&paths, &fixture(), &registration).unwrap();
    fs::remove_file(paths.fonts.join(FILE)).unwrap();
    assert!(
        install(&paths, &fixture(), &registration)
            .unwrap()
            .contains("Repaired")
    );
    assert!(paths.fonts.join(FILE).exists());
    fs::remove_file(paths.fonts.join(FILE)).unwrap();
    remove(&paths, &registration).unwrap();
    assert!(load(&paths).unwrap().is_none());
}
#[test]
fn corrupt_or_unwritable_targets_are_actionable_errors() {
    let dir = tempfile::tempdir().unwrap();
    let paths = sandbox(dir.path());
    assert!(install(&paths, b"invalid", &Mock::default()).is_err());
    assert!(!paths.state.exists());
    fs::create_dir(paths.fonts.join(FILE)).unwrap();
    assert!(
        install(&paths, &fixture(), &Mock::default())
            .unwrap_err()
            .to_string()
            .contains("already exists")
    );
    let path = dir.path().join("settings.json");
    fs::write(&path, "{}").unwrap();
    let original_permissions = fs::metadata(&path).unwrap().permissions();
    let mut permissions = original_permissions.clone();
    permissions.set_readonly(true);
    fs::set_permissions(&path, permissions).unwrap();
    assert!(
        configure_file(&paths, &path, &terminal::Scope::Defaults, "Test")
            .unwrap_err()
            .to_string()
            .contains("read-only")
    );
    fs::set_permissions(&path, original_permissions).unwrap();
}
fn environment(entries: &[(&str, &str)]) -> Environment {
    Environment(
        entries
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect(),
    )
}
#[test]
fn platform_paths_are_per_user_and_reject_relative_roots() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_str().unwrap();
    let env = environment(&[
        ("LOCALAPPDATA", root),
        ("USERPROFILE", root),
        ("HOME", root),
        ("XDG_DATA_HOME", root),
        ("XDG_STATE_HOME", root),
    ]);
    assert_eq!(
        paths(Platform::Windows, &env).unwrap().fonts,
        dir.path().join("Microsoft/Windows/Fonts")
    );
    assert_eq!(
        paths(Platform::Linux, &env).unwrap().fonts,
        dir.path().join("fonts/filavim")
    );
    assert_eq!(
        paths(Platform::MacOS, &env).unwrap().fonts,
        dir.path().join("Library/Fonts")
    );
    assert!(
        paths(
            Platform::Windows,
            &environment(&[("LOCALAPPDATA", "relative")])
        )
        .is_err()
    );
    assert!(paths(Platform::Linux, &Environment::default()).is_err());
}
#[test]
fn terminal_identity_requires_reliable_signals_and_remote_sessions_are_manual() {
    use terminal::Kind::*;
    for (env, expected) in [
        (
            environment(&[("TERM", "xterm-kitty")]),
            Other("unknown".into()),
        ),
        (environment(&[("WT_SESSION", "id")]), WindowsTerminal),
        (environment(&[("WEZTERM_PANE", "1")]), WezTerm),
        (environment(&[("KITTY_WINDOW_ID", "1")]), Kitty),
        (environment(&[("ALACRITTY_WINDOW_ID", "1")]), Alacritty),
    ] {
        assert_eq!(terminal::detect(&env), expected);
    }
    assert!(matches!(
        terminal::detect(&environment(&[
            ("WT_SESSION", "id"),
            ("SSH_CONNECTION", "host")
        ])),
        Other(_)
    ));
    for kind in [WezTerm, Kitty, Alacritty, Other("unknown".into())] {
        assert!(terminal::manual(&kind, "Test Family").contains("Test Family"));
    }
}
const ID: &str = "{11111111-2222-3333-4444-555555555555}";
const JSONC: &str = r#"{
    // user theme stays here
    "theme": "dark", /* a block comment */
    "profiles": {
        "defaults": {"font": {"size": 17, "face": "Old",},},
        "list": [{"guid":"{11111111-2222-3333-4444-555555555555}", "name":"Shell", "font":{"size":19}},],
    },
}"#;
#[test]
fn jsonc_edit_preserves_comments_preferences_and_profile_scope() {
    let scope = terminal::Scope::Profile(ID.into());
    let result = terminal::edit(JSONC, &scope, "New Family").unwrap();
    assert!(result.contains("// user theme stays here"));
    assert!(result.contains("/* a block comment */"));
    assert!(result.contains("\"size\":19"));
    assert!(result.contains("\"size\": 17, \"face\": \"Old\""));
    assert!(result.contains("\"theme\": \"dark\""));
    assert_eq!(
        terminal::configured(&result, &scope).unwrap().as_deref(),
        Some("New Family")
    );
    assert_eq!(
        terminal::configured(&result, &terminal::Scope::Defaults)
            .unwrap()
            .as_deref(),
        Some("Old")
    );
    let defaults = terminal::edit(JSONC, &terminal::Scope::Defaults, "New").unwrap();
    assert!(defaults.contains("\"face\": \"New\""));
    let crlf = JSONC.replace('\n', "\r\n");
    assert!(
        !terminal::edit(&crlf, &scope, "New")
            .unwrap()
            .replace("\r\n", "")
            .contains('\n')
    );
}
#[test]
fn jsonc_creation_bom_escapes_and_ambiguous_inputs() {
    let input = "\u{feff}{\"profiles\":{/*keep*/\"list\":[]},\"url\":\"https://host//x\",}";
    let output = terminal::edit(input, &terminal::Scope::Defaults, "Quote \" Family").unwrap();
    assert!(output.starts_with('\u{feff}'));
    assert!(output.contains("/*keep*/"));
    assert!(output.contains("https://host//x"));
    assert_eq!(
        terminal::configured(&output, &terminal::Scope::Defaults)
            .unwrap()
            .as_deref(),
        Some("Quote \" Family")
    );
    for invalid in [
        "{",
        "{\"a\":1,\"a\":2}",
        "{/*broken",
        "{\"profiles\":null}",
        "{\"profiles\":{\"defaults\":false}}",
    ] {
        assert!(
            terminal::edit(invalid, &terminal::Scope::Defaults, "Test").is_err(),
            "{invalid}"
        );
    }
    assert!(terminal::edit(JSONC, &terminal::Scope::Profile("not-guid".into()), "Test").is_err());
    assert!(
        terminal::edit(
            JSONC,
            &terminal::Scope::Profile("{00000000-0000-0000-0000-000000000000}".into()),
            "Test"
        )
        .is_err()
    );
}
#[test]
fn settings_backup_journal_verification_and_repeat_configuration() {
    let dir = tempfile::tempdir().unwrap();
    let paths = sandbox(dir.path());
    let file = dir.path().join("settings.json");
    fs::write(&file, JSONC).unwrap();
    let scope = terminal::Scope::Profile(ID.into());
    configure_file(&paths, &file, &scope, "Test").unwrap();
    let entries: Vec<_> = fs::read_dir(&paths.state)
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    assert_eq!(entries.len(), 2);
    let backup = entries
        .iter()
        .find(|p| p.extension().is_some_and(|e| e == "bak"))
        .unwrap();
    assert_eq!(fs::read_to_string(backup).unwrap(), JSONC);
    let journal = entries
        .iter()
        .find(|p| p.extension().is_some_and(|e| e == "json"))
        .unwrap();
    let change: ConfigChange = serde_json::from_slice(&fs::read(journal).unwrap()).unwrap();
    assert_eq!(change.before_hash, hash(JSONC.as_bytes()));
    assert_eq!(change.after_hash, hash(&fs::read(&file).unwrap()));
    assert!(
        configure_file(&paths, &file, &scope, "Test")
            .unwrap()
            .contains("already uses")
    );
    assert_eq!(fs::read_dir(&paths.state).unwrap().count(), 2);
    // Removal cannot overwrite a terminal edit: configuration and ownership are
    // deliberately separate, and the journal is never replayed during removal.
    let registration = Mock::default();
    install(&paths, &fixture(), &registration).unwrap();
    let user = "{\"user\":true}";
    fs::write(&file, user).unwrap();
    remove(&paths, &registration).unwrap();
    assert_eq!(fs::read_to_string(&file).unwrap(), user);
    assert!(backup.exists());
}
#[test]
fn windows_terminal_editions_are_explicit_when_multiple_settings_exist() {
    let dir = tempfile::tempdir().unwrap();
    let env = environment(&[
        ("LOCALAPPDATA", dir.path().to_str().unwrap()),
        ("WT_SESSION", "session"),
        ("WT_PROFILE_ID", ID),
    ]);
    let candidates = terminal::settings_paths(&env);
    assert_eq!(candidates.len(), 3);
    for path in &candidates {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, JSONC).unwrap();
    }
    let terminal = terminal::inspect(&env, Platform::Windows);
    assert_eq!(terminal.targets.len(), 3);
    assert!(terminal.note.contains("Several settings"));
    assert!(
        terminal
            .targets
            .iter()
            .all(|t| t.scope == terminal::Scope::Profile(ID.into()))
    );
    assert!(terminal::validate_target(&env, &dir.path().join("unrelated.json")).is_err());
    let mut unknown = env.clone();
    unknown.0.remove("WT_PROFILE_ID");
    let terminal = terminal::inspect(&unknown, Platform::Windows);
    assert_eq!(terminal.targets.len(), 6);
    assert!(
        terminal
            .targets
            .iter()
            .any(|t| t.scope == terminal::Scope::Defaults)
    );
}
#[test]
fn missing_unsupported_and_invalid_settings_are_not_claimed_as_configured() {
    let dir = tempfile::tempdir().unwrap();
    let env = environment(&[
        ("LOCALAPPDATA", dir.path().to_str().unwrap()),
        ("WT_SESSION", "s"),
    ]);
    let terminal = terminal::inspect(&env, Platform::Windows);
    assert!(terminal.targets.is_empty());
    assert!(terminal.note.contains("not found"));
    let path = terminal::settings_paths(&env).remove(0);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, b"broken").unwrap();
    let terminal = terminal::inspect(&env, Platform::Windows);
    assert!(terminal.targets.is_empty());
    assert!(!terminal.note.is_empty());
    assert!(
        terminal::inspect(&Environment::default(), Platform::Windows)
            .targets
            .is_empty()
    );
}

struct FaultySettings {
    reads: Cell<usize>,
    writes: Cell<usize>,
    mode: u8,
}
impl SettingsIo for FaultySettings {
    fn read(&self, path: &Path) -> Result<String> {
        let count = self.reads.get() + 1;
        self.reads.set(count);
        if self.mode == 1 && count == 2 {
            fs::write(path, "{\"concurrent\":true}")?;
        }
        if self.mode == 3 && count == 3 {
            bail!("injected verification read failure");
        }
        DiskSettings.read(path)
    }
    fn write(&self, path: &Path, bytes: &[u8]) -> Result<()> {
        let count = self.writes.get() + 1;
        self.writes.set(count);
        DiskSettings.write(path, bytes)?;
        if count == 1 {
            if self.mode == 2 {
                bail!("injected post-replace durability failure");
            }
            if self.mode == 4 {
                fs::write(path, "{\"concurrent\":true}")?;
            }
        }
        Ok(())
    }
}
#[test]
fn configuration_rollback_only_replaces_our_own_bytes() {
    for mode in 1..=4 {
        let dir = tempfile::tempdir().unwrap();
        let paths = sandbox(dir.path());
        let path = dir.path().join("settings.json");
        fs::write(&path, JSONC).unwrap();
        let io = FaultySettings {
            reads: Cell::new(0),
            writes: Cell::new(0),
            mode,
        };
        let error =
            configure_with(&paths, &path, &terminal::Scope::Defaults, "New Font", &io).unwrap_err();
        let result = fs::read_to_string(&path).unwrap();
        match mode {
            1 | 4 => assert_eq!(result, "{\"concurrent\":true}"),
            2 => {
                assert_eq!(result, JSONC);
                assert!(error.to_string().contains("original settings restored"));
                assert_eq!(io.writes.get(), 2);
            }
            3 => {
                assert_eq!(
                    terminal::configured(&result, &terminal::Scope::Defaults)
                        .unwrap()
                        .as_deref(),
                    Some("New Font")
                );
                assert_eq!(io.writes.get(), 1);
            }
            _ => unreachable!(),
        }
        let backup = fs::read_dir(&paths.state)
            .unwrap()
            .filter_map(|e| e.ok().map(|e| e.path()))
            .find(|p| p.extension().is_some_and(|e| e == "bak"))
            .unwrap();
        assert_eq!(fs::read_to_string(backup).unwrap(), JSONC);
    }
}
