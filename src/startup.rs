//! Startup apps, as on Task Manager's Startup apps tab: the `Run` keys of the user and
//! the machine (64- and 32-bit) and both Startup folders. Whether an entry is on lives
//! in Explorer's `StartupApproved` keys, which is also what Task Manager and Settings
//! change, so all three always agree. Machine-wide entries need admin to switch.

use std::path::Path;

use windows::Win32::Foundation::{ERROR_ACCESS_DENIED, ERROR_FILE_NOT_FOUND, ERROR_NO_MORE_ITEMS};
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx, IPersistFile, STGM_READ,
};
use windows::Win32::System::Environment::ExpandEnvironmentStringsW;
use windows::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ, REG_BINARY, RRF_RT_REG_BINARY, RegCloseKey, RegEnumValueW,
    RegGetValueW, RegOpenKeyExW, RegSetKeyValueW,
};
use windows::Win32::System::SystemInformation::GetSystemTimeAsFileTime;
use windows::Win32::UI::Shell::{IShellLinkW, ShellLink};
use windows::core::{HSTRING, Interface, PWSTR};

use crate::tasks::Error;

#[derive(Clone, Debug)]
pub struct Entry {
    /// What Task Manager shows: the value name, or the shortcut's name without `.lnk`.
    pub name: String,
    pub command: String,
    /// The program it starts, when that can be told from the command.
    pub path: Option<String>,
    pub publisher: Option<String>,
    pub enabled: bool,
    /// When it was switched off, as a `FILETIME` value.
    pub disabled_at: Option<i64>,
    pub source: Source,
    /// The value name or file name it is known by in `StartupApproved`.
    key: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Source {
    UserRun,
    MachineRun,
    MachineRun32,
    UserFolder,
    CommonFolder,
}

impl Source {
    pub fn label(self) -> &'static str {
        match self {
            Source::UserRun => "Registry (user)",
            Source::MachineRun => "Registry (all users)",
            Source::MachineRun32 => "Registry (all users, 32-bit)",
            Source::UserFolder => "Startup folder (user)",
            Source::CommonFolder => "Startup folder (all users)",
        }
    }

    /// The `StartupApproved` key holding this source's on/off flags.
    fn approved(self) -> (HKEY, String) {
        let (root, leaf) = match self {
            Source::UserRun => (HKEY_CURRENT_USER, "Run"),
            Source::MachineRun => (HKEY_LOCAL_MACHINE, "Run"),
            Source::MachineRun32 => (HKEY_LOCAL_MACHINE, "Run32"),
            Source::UserFolder => (HKEY_CURRENT_USER, "StartupFolder"),
            Source::CommonFolder => (HKEY_LOCAL_MACHINE, "StartupFolder"),
        };
        (root, format!(r"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\{leaf}"))
    }
}

/// Every startup entry. Resolving shortcuts uses COM, so call it off the UI thread.
pub fn list() -> Vec<Entry> {
    let _ = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
    let run = r"Software\Microsoft\Windows\CurrentVersion\Run";
    let mut entries = Vec::new();
    for (root, key, source) in [
        (HKEY_CURRENT_USER, run.to_owned(), Source::UserRun),
        (HKEY_LOCAL_MACHINE, run.to_owned(), Source::MachineRun),
        (
            HKEY_LOCAL_MACHINE,
            r"Software\WOW6432Node\Microsoft\Windows\CurrentVersion\Run".to_owned(),
            Source::MachineRun32,
        ),
    ] {
        for (name, command) in string_values(root, &key) {
            entries.push(entry(name.clone(), name, command, source));
        }
    }
    let folders = [
        (std::env::var("APPDATA").ok(), Source::UserFolder),
        (std::env::var("ProgramData").ok(), Source::CommonFolder),
    ];
    for (base, source) in folders {
        let Some(base) = base else { continue };
        let dir = Path::new(&base).join(r"Microsoft\Windows\Start Menu\Programs\Startup");
        let Ok(files) = std::fs::read_dir(dir) else { continue };
        for file in files.flatten() {
            let file_name = file.file_name().to_string_lossy().into_owned();
            if file_name.eq_ignore_ascii_case("desktop.ini") {
                continue;
            }
            let path = file.path().display().to_string();
            let command =
                if file_name.to_lowercase().ends_with(".lnk") { shortcut(&path).unwrap_or(path) } else { path };
            let name = file_name.strip_suffix(".lnk").unwrap_or(&file_name).to_owned();
            entries.push(entry(name, file_name, command, source));
        }
    }
    entries
}

fn entry(name: String, key: String, command: String, source: Source) -> Entry {
    let command = expand(&command);
    let path = Some(crate::autostart::command_path(&command).to_owned()).filter(|p| Path::new(p).is_file());
    let publisher = path.as_deref().and_then(|p| crate::sensors::version_string(p, "CompanyName"));
    let (enabled, disabled_at) = approval(source, &key);
    Entry { name, command, path, publisher, enabled, disabled_at, source, key }
}

/// Whether Explorer's flag for this entry says on; off entries carry the time they
/// were switched off. No flag means on.
fn approval(source: Source, key: &str) -> (bool, Option<i64>) {
    let (root, path) = source.approved();
    let mut data = [0u8; 12];
    let mut size = data.len() as u32;
    let result = unsafe {
        RegGetValueW(
            root,
            &HSTRING::from(path),
            &HSTRING::from(key),
            RRF_RT_REG_BINARY,
            None,
            Some(data.as_mut_ptr().cast()),
            Some(&mut size),
        )
    };
    if result.is_err() || size < 4 {
        return (true, None);
    }
    // Even first bytes (02, 06) mean on, odd ones (03, 07) off.
    let enabled = data[0] & 1 == 0;
    let time = i64::from_le_bytes(data[4..12].try_into().expect("8 bytes"));
    (enabled, (!enabled && time > 0).then_some(time))
}

/// Switches an entry on or off the way Task Manager does.
pub fn set_enabled(entry: &Entry, on: bool) -> Result<(), Error> {
    let (root, path) = entry.source.approved();
    let mut data = [0u8; 12];
    if on {
        data[0] = 2;
    } else {
        data[0] = 3;
        let now = unsafe { GetSystemTimeAsFileTime() };
        let time = ((now.dwHighDateTime as i64) << 32) | now.dwLowDateTime as i64;
        data[4..].copy_from_slice(&time.to_le_bytes());
    }
    let result = unsafe {
        RegSetKeyValueW(
            root,
            &HSTRING::from(path),
            &HSTRING::from(&*entry.key),
            REG_BINARY.0,
            Some(data.as_ptr().cast()),
            12,
        )
    };
    match result {
        r if r.is_ok() => Ok(()),
        ERROR_ACCESS_DENIED => Err(Error::AccessDenied),
        r => Err(Error::Other(windows::core::Error::from(r.to_hresult()).message())),
    }
}

/// Days since a `FILETIME`, for "Disabled 3 days ago".
pub fn days_since(filetime: i64) -> i64 {
    let now = unsafe { GetSystemTimeAsFileTime() };
    let now = ((now.dwHighDateTime as i64) << 32) | now.dwLowDateTime as i64;
    (now - filetime) / (10_000_000 * 86_400)
}

fn string_values(root: HKEY, path: &str) -> Vec<(String, String)> {
    let mut key = HKEY::default();
    if unsafe { RegOpenKeyExW(root, &HSTRING::from(path), None, KEY_READ, &mut key) }.is_err() {
        return Vec::new();
    }
    let mut values = Vec::new();
    for index in 0.. {
        let mut name = [0u16; 512];
        let mut name_len = name.len() as u32;
        let mut data = vec![0u16; 2048];
        let mut data_len = (data.len() * 2) as u32;
        let mut kind = 0u32;
        let result = unsafe {
            RegEnumValueW(
                key,
                index,
                Some(PWSTR(name.as_mut_ptr())),
                &mut name_len,
                None,
                Some(&mut kind),
                Some(data.as_mut_ptr().cast()),
                Some(&mut data_len),
            )
        };
        if result == ERROR_NO_MORE_ITEMS || result == ERROR_FILE_NOT_FOUND {
            break;
        }
        // REG_SZ (1) and REG_EXPAND_SZ (2); skip anything else and oversized values.
        if result.is_err() || !matches!(kind, 1 | 2) {
            continue;
        }
        let chars = (data_len as usize / 2).min(data.len());
        let end = data[..chars].iter().position(|&c| c == 0).unwrap_or(chars);
        values.push((String::from_utf16_lossy(&name[..name_len as usize]), String::from_utf16_lossy(&data[..end])));
    }
    let _ = unsafe { RegCloseKey(key) };
    values
}

fn expand(text: &str) -> String {
    let source = HSTRING::from(text);
    let mut buffer = vec![0u16; 2048];
    let len = unsafe { ExpandEnvironmentStringsW(&source, Some(&mut buffer)) } as usize;
    if len == 0 || len > buffer.len() {
        return text.to_owned();
    }
    String::from_utf16_lossy(&buffer[..len - 1])
}

/// A shortcut's target with its arguments, as a command line.
fn shortcut(path: &str) -> Option<String> {
    let link: IShellLinkW = unsafe { CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER) }.ok()?;
    let file: IPersistFile = link.cast().ok()?;
    unsafe { file.Load(&HSTRING::from(path), STGM_READ) }.ok()?;
    let mut target = [0u16; 520];
    unsafe { link.GetPath(&mut target, std::ptr::null_mut(), 0) }.ok()?;
    let mut args = [0u16; 1024];
    let _ = unsafe { link.GetArguments(&mut args) };
    let text = |b: &[u16]| String::from_utf16_lossy(&b[..b.iter().position(|&c| c == 0).unwrap_or(b.len())]);
    let (target, args) = (text(&target), text(&args));
    if target.is_empty() {
        return None;
    }
    Some(if args.is_empty() { format!("\"{target}\"") } else { format!("\"{target}\" {args}") })
}

#[cfg(test)]
mod tests {
    #[test]
    fn lists_entries_with_programs() {
        let entries = super::list();
        for e in &entries {
            eprintln!("{:<28} {:<5} {:<30} {:?} {}", e.name, e.enabled, e.source.label(), e.publisher, e.command);
        }
        assert!(entries.iter().all(|e| !e.name.is_empty()));
    }
}
