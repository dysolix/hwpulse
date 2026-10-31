//! "Start with Windows" via the per-user `Run` key. The registry is the only source of
//! truth, so the installer's checkbox and the switch in Settings always agree.
//!
//! The entry names one exe. Portable copies get moved (Downloads → somewhere
//! permanent), so [`check`] repairs an entry whose exe no longer exists and tells a
//! second copy apart from this one.

use std::path::{Path, PathBuf};

use windows::Win32::Foundation::ERROR_FILE_NOT_FOUND;
use windows::Win32::System::Registry::{
    HKEY_CURRENT_USER, REG_SZ, RRF_RT_REG_SZ, RegDeleteKeyValueW, RegGetValueW, RegSetKeyValueW,
};
use windows::core::{HSTRING, w};

const RUN_KEY: windows::core::PCWSTR = w!(r"Software\Microsoft\Windows\CurrentVersion\Run");
/// Same value name as in `installer/hwmon.iss`.
const VALUE: windows::core::PCWSTR = w!("hwmon");

#[derive(Clone, Debug, PartialEq)]
pub enum State {
    Off,
    /// This exe starts with Windows.
    On,
    /// Another copy that still exists starts with Windows (e.g. the installed one).
    OtherCopy(PathBuf),
}

/// Reads the entry and, if it points to an exe that no longer exists (this copy
/// was moved), points it to this exe. Never creates an entry. Call once at startup.
pub fn check() -> State {
    let Some(registered) = registered_path() else { return State::Off };
    let Ok(exe) = std::env::current_exe() else { return State::Off };
    if same_file(&registered, &exe) {
        State::On
    } else if registered.exists() {
        State::OtherCopy(registered)
    } else if set(true).is_ok() {
        State::On
    } else {
        State::Off
    }
}

/// Registers the running exe (wherever it lives now) or removes the entry.
pub fn set(on: bool) -> windows::core::Result<()> {
    if on {
        let exe = std::env::current_exe()
            .map_err(|e| windows::core::Error::new(windows::core::HRESULT(-1), e.to_string()))?;
        let command = HSTRING::from(format!("\"{}\"", exe.display()));
        let bytes = (command.len() + 1) * 2; // UTF-16 including the terminating NUL
        unsafe {
            RegSetKeyValueW(HKEY_CURRENT_USER, RUN_KEY, VALUE, REG_SZ.0, Some(command.as_ptr().cast()), bytes as u32)
        }
        .ok()
    } else {
        match unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, RUN_KEY, VALUE) } {
            ERROR_FILE_NOT_FOUND => Ok(()),
            e => e.ok(),
        }
    }
}

/// The exe path in the entry, without quotes or arguments.
fn registered_path() -> Option<PathBuf> {
    let mut bytes = 0u32;
    unsafe { RegGetValueW(HKEY_CURRENT_USER, RUN_KEY, VALUE, RRF_RT_REG_SZ, None, None, Some(&mut bytes)) }
        .ok()
        .ok()?;
    let mut buffer = vec![0u16; (bytes as usize).div_ceil(2)];
    unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            RUN_KEY,
            VALUE,
            RRF_RT_REG_SZ,
            None,
            Some(buffer.as_mut_ptr().cast()),
            Some(&mut bytes),
        )
    }
    .ok()
    .ok()?;
    let end = buffer.iter().position(|&c| c == 0).unwrap_or(buffer.len());
    Some(PathBuf::from(command_path(&String::from_utf16_lossy(&buffer[..end]))))
}

/// `"C:\a b\hwmon.exe" --flag` → `C:\a b\hwmon.exe`; unquoted values up to the
/// first space after ".exe".
pub fn command_path(command: &str) -> &str {
    let command = command.trim();
    if let Some(rest) = command.strip_prefix('"') {
        return rest.split('"').next().unwrap_or(rest);
    }
    match command.to_ascii_lowercase().find(".exe") {
        Some(i) => &command[..i + 4],
        None => command,
    }
}

/// Paths compared the way Windows does: case-insensitive, after resolving `..`,
/// short names and links where the files exist.
fn same_file(a: &Path, b: &Path) -> bool {
    let normal =
        |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf()).to_string_lossy().to_lowercase();
    normal(a) == normal(b)
}

#[cfg(test)]
mod tests {
    use super::command_path;

    #[test]
    fn exe_path_from_run_command() {
        assert_eq!(command_path(r#""C:\Program Files\hwmon\hwmon.exe""#), r"C:\Program Files\hwmon\hwmon.exe");
        assert_eq!(command_path(r#""C:\x\hwmon.exe" --minimized"#), r"C:\x\hwmon.exe");
        assert_eq!(command_path(r"C:\x\hwmon.exe --minimized"), r"C:\x\hwmon.exe");
    }
}
