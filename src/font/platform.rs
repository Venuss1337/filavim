use super::{Metadata, Platform, Registration};
use anyhow::Result;
use std::path::Path;
pub struct Native(pub Platform);
impl Registration for Native {
    fn existing(&self, metadata: &Metadata) -> Result<Option<std::path::PathBuf>> {
        if self.0 == Platform::Windows {
            windows::existing(metadata)
        } else {
            Ok(None)
        }
    }
    fn register(&self, path: &Path, metadata: &Metadata) -> Result<String> {
        match self.0 {
            Platform::Windows => windows::register(path,metadata),
            Platform::Linux => cache(path.parent().unwrap_or(path)),
            Platform::MacOS => Ok("Installed in ~/Library/Fonts; macOS discovers per-user fonts. Restart the terminal if necessary.".into()),
        }
    }
    fn unregister(&self, path: &Path, metadata: &Metadata) -> Result<String> {
        match self.0 {
            Platform::Windows => windows::unregister(path, metadata),
            _ => Ok(String::new()),
        }
    }
    fn removed(&self, directory: &Path) -> Result<String> {
        if self.0 == Platform::Linux {
            cache(directory)
        } else {
            Ok(String::new())
        }
    }
}
fn cache(directory: &Path) -> Result<String> {
    if let Some(executable) = crate::setup::executable("fc-cache") {
        let mut command = std::process::Command::new(executable);
        command.arg("-f").arg(directory);
        crate::setup::run(command, std::time::Duration::from_secs(60))?;
        Ok("Per-user font cache refreshed with fc-cache.".into())
    } else {
        Ok("fc-cache is unavailable. Font file placement verified; restart the terminal to refresh font discovery.".into())
    }
}
#[cfg(not(windows))]
mod windows {
    use super::*;
    pub fn existing(_: &Metadata) -> Result<Option<std::path::PathBuf>> {
        Ok(None)
    }
    pub fn register(_: &Path, _: &Metadata) -> Result<String> {
        anyhow::bail!("Windows registration is unavailable on this host")
    }
    pub fn unregister(_: &Path, _: &Metadata) -> Result<String> {
        anyhow::bail!("Windows registration is unavailable on this host")
    }
}
#[cfg(windows)]
mod windows {
    use super::*;
    use anyhow::{Context, ensure};
    use std::{
        os::windows::ffi::{OsStrExt, OsStringExt},
        ptr,
    };
    use windows_sys::Win32::{
        Graphics::Gdi::{AddFontResourceExW, RemoveFontResourceExW},
        System::Registry::*,
        UI::WindowsAndMessaging::*,
    };
    const KEY: &str = "Software\\Microsoft\\Windows NT\\CurrentVersion\\Fonts";
    fn wide(s: &std::ffi::OsStr) -> Result<Vec<u16>> {
        let mut result: Vec<_> = s.encode_wide().collect();
        ensure!(
            !result.contains(&0),
            "Font path or registry name contains a NUL character"
        );
        result.push(0);
        Ok(result)
    }
    struct Key(HKEY);
    impl Drop for Key {
        fn drop(&mut self) {
            unsafe {
                RegCloseKey(self.0);
            }
        }
    }
    fn checked(code: u32, action: &str) -> Result<()> {
        ensure!(
            code == 0,
            "{action}: {}. Check current-user registry permissions; administrator access is not required.",
            std::io::Error::from_raw_os_error(code as i32)
        );
        Ok(())
    }
    fn open() -> Result<Key> {
        let mut key = ptr::null_mut();
        let name = wide(std::ffi::OsStr::new(KEY))?;
        // SAFETY: all strings are NUL terminated; output pointers are valid and
        // the owned handle is closed by Key. Only HKCU is ever modified.
        unsafe {
            checked(
                RegCreateKeyExW(
                    HKEY_CURRENT_USER,
                    name.as_ptr(),
                    0,
                    ptr::null(),
                    0,
                    KEY_QUERY_VALUE | KEY_SET_VALUE,
                    ptr::null(),
                    &mut key,
                    ptr::null_mut(),
                ),
                "Cannot open per-user font registry",
            )?;
        }
        Ok(Key(key))
    }
    fn value(key: &Key, name: &[u16]) -> Result<Option<Vec<u16>>> {
        let mut length = 0;
        let mut kind = 0;
        let code = unsafe {
            RegQueryValueExW(
                key.0,
                name.as_ptr(),
                ptr::null(),
                &mut kind,
                ptr::null_mut(),
                &mut length,
            )
        };
        if code == 2 {
            return Ok(None);
        }
        checked(code, "Cannot read font registration")?;
        ensure!(
            kind == REG_SZ && length <= 65536 && length % 2 == 0,
            "Unexpected font registry value; refusing to replace it"
        );
        let mut buffer = vec![0u16; length as usize / 2];
        unsafe {
            checked(
                RegQueryValueExW(
                    key.0,
                    name.as_ptr(),
                    ptr::null(),
                    &mut kind,
                    buffer.as_mut_ptr().cast(),
                    &mut length,
                ),
                "Cannot read font registration",
            )?;
        }
        buffer.truncate(length as usize / 2);
        Ok(Some(buffer))
    }
    fn notify() -> String {
        // Bounded notification: an unresponsive application cannot hang setup.
        let result = unsafe {
            SendMessageTimeoutW(
                HWND_BROADCAST,
                WM_FONTCHANGE,
                0,
                0,
                SMTO_ABORTIFHUNG,
                100,
                ptr::null_mut(),
            )
        };
        if result == 0 {
            "Font registered; some applications did not acknowledge WM_FONTCHANGE. Restart the terminal if needed.".into()
        } else {
            "Applications notified with WM_FONTCHANGE; restart terminals that cache font lists."
                .into()
        }
    }
    fn name(m: &Metadata) -> Result<Vec<u16>> {
        wide(std::ffi::OsStr::new(&format!("{} (TrueType)", m.full_name)))
    }
    pub fn existing(m: &Metadata) -> Result<Option<std::path::PathBuf>> {
        let subkey = wide(std::ffi::OsStr::new(KEY))?;
        for hive in [HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE] {
            let mut handle = ptr::null_mut();
            let code =
                unsafe { RegOpenKeyExW(hive, subkey.as_ptr(), 0, KEY_QUERY_VALUE, &mut handle) };
            if code == 2 {
                continue;
            }
            checked(code, "Cannot inspect installed font registrations")?;
            let key = Key(handle);
            for suffix in [" (TrueType)", " (OpenType)", ""] {
                let name = wide(std::ffi::OsStr::new(&format!("{}{suffix}", m.full_name)))?;
                if let Some(mut data) = value(&key, &name)? {
                    while data.last() == Some(&0) {
                        data.pop();
                    }
                    let path = std::path::PathBuf::from(std::ffi::OsString::from_wide(&data));
                    let path = if path.is_absolute() {
                        path
                    } else {
                        anyhow::ensure!(
                            path.components().count() == 1,
                            "Unexpected relative font registry path"
                        );
                        std::path::PathBuf::from(
                            std::env::var_os("WINDIR").context("WINDIR missing")?,
                        )
                        .join("Fonts")
                        .join(path)
                    };
                    let actual = super::super::metadata(&super::super::read_bytes(
                        &path,
                        super::super::MAX_FONT,
                    )?)?;
                    anyhow::ensure!(
                        actual.postscript == m.postscript,
                        "An existing registration with the same name points to a different font: {}",
                        path.display()
                    );
                    return Ok(Some(path));
                }
            }
        }
        Ok(None)
    }
    pub fn register(path: &Path, m: &Metadata) -> Result<String> {
        let key = open()?;
        let name = name(m)?;
        let path = wide(path.as_os_str())?;
        if let Some(old) = value(&key, &name)? {
            ensure!(
                old == path,
                "A font registration with this name already points elsewhere; refusing to overwrite it"
            );
            return Ok(
                "Existing HKCU registration verified; no duplicate resource was loaded.".into(),
            );
        }
        unsafe {
            checked(
                RegSetValueExW(
                    key.0,
                    name.as_ptr(),
                    0,
                    REG_SZ,
                    path.as_ptr().cast(),
                    (path.len() * 2) as u32,
                ),
                "Cannot register per-user font persistently",
            )?;
            ensure!(
                AddFontResourceExW(path.as_ptr(), 0, ptr::null()) > 0,
                "Windows could not load the installed font. Registration will be rolled back; check the font data and file permissions."
            );
        }
        ensure!(
            value(&key, &name)?.as_ref() == Some(&path),
            "Font registry verification failed"
        );
        Ok(notify())
    }
    pub fn unregister(path: &Path, m: &Metadata) -> Result<String> {
        let key = open()?;
        let name = name(m)?;
        let path = wide(path.as_os_str())?;
        if let Some(old) = value(&key, &name)? {
            ensure!(
                old == path,
                "Font registration was changed by another application; refusing to remove it"
            );
            unsafe {
                // A font may not be loaded in this session; registry/file
                // ownership, rather than GDI's return value, controls removal.
                checked(
                    RegDeleteValueW(key.0, name.as_ptr()),
                    "Cannot remove per-user font registration",
                )?;
                RemoveFontResourceExW(path.as_ptr(), 0, ptr::null());
            }
        }
        Ok(notify())
    }
}
