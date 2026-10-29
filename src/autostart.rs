//! "Start with Windows" via the per-user `Run` key. The registry is the only source of
//! truth, so the installer's checkbox and the switch in Settings always agree.

use windows::Win32::Foundation::ERROR_FILE_NOT_FOUND;
use windows::Win32::System::Registry::{
    HKEY_CURRENT_USER, REG_SZ, RRF_RT_REG_SZ, RegDeleteKeyValueW, RegGetValueW, RegSetKeyValueW,
};
use windows::core::{HSTRING, w};

const RUN_KEY: windows::core::PCWSTR = w!(r"Software\Microsoft\Windows\CurrentVersion\Run");
/// Same value name as in `installer/hwmon.iss`.
const VALUE: windows::core::PCWSTR = w!("hwmon");

pub fn enabled() -> bool {
    unsafe { RegGetValueW(HKEY_CURRENT_USER, RUN_KEY, VALUE, RRF_RT_REG_SZ, None, None, None) }.is_ok()
}

/// Registers the running exe (wherever it lives now) or removes the entry.
pub fn set(on: bool) -> windows::core::Result<()> {
    if on {
        let exe = std::env::current_exe().map_err(|e| windows::core::Error::new(windows::core::HRESULT(-1), e.to_string()))?;
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
