//! Handing things to Windows: Explorer, file properties, the browser, other windows,
//! and starting programs ("Run new task").

use std::os::windows::process::CommandExt;

use windows::Win32::Foundation::HWND;
use windows::Win32::UI::Shell::{SHOP_FILEPATH, SHObjectProperties, ShellExecuteW};
use windows::Win32::UI::WindowsAndMessaging::{IsIconic, SW_RESTORE, SW_SHOWNORMAL, SetForegroundWindow, ShowWindow};
use windows::core::{HSTRING, PCWSTR, w};

/// Opens Explorer with the file selected.
pub fn open_location(path: &str) {
    let _ = std::process::Command::new("explorer.exe").raw_arg(format!("/select,\"{path}\"")).spawn();
}

/// The file's Properties dialog, as from Explorer.
pub fn properties(path: &str) {
    let _ = unsafe { SHObjectProperties(None, SHOP_FILEPATH, &HSTRING::from(path), PCWSTR::null()) };
}

/// Searches the web in the default browser, as Task Manager's "Search online".
pub fn search_online(query: &str) {
    let encoded: String = query
        .bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' => (b as char).to_string(),
            b' ' => "+".into(),
            b => format!("%{b:02X}"),
        })
        .collect();
    open(&format!("https://www.bing.com/search?q={encoded}"));
}

/// Opens a file, folder, URL or console (`services.msc`) with its default handler.
pub fn open(target: &str) -> bool {
    let result = unsafe { ShellExecuteW(None, w!("open"), &HSTRING::from(target), None, None, SW_SHOWNORMAL) };
    result.0 as usize > 32
}

/// Brings a window to the front, restoring it when minimized.
pub fn switch_to(hwnd: isize) -> bool {
    let hwnd = HWND(hwnd as *mut _);
    unsafe {
        if IsIconic(hwnd).as_bool() {
            let _ = ShowWindow(hwnd, SW_RESTORE);
        }
        SetForegroundWindow(hwnd).as_bool()
    }
}

/// Starts `command` (a program, file, folder or URL, optionally with arguments) the way
/// the Run dialog does; `admin` asks for elevation first.
pub fn run(command: &str, admin: bool) -> Result<(), String> {
    let (file, args) = split_command(command.trim());
    if file.is_empty() {
        return Err("Type the name of a program, folder, document or website.".into());
    }
    let verb = if admin { w!("runas") } else { w!("open") };
    let args = (!args.is_empty()).then(|| HSTRING::from(args));
    let args = args.as_ref().map_or(PCWSTR::null(), |a| PCWSTR(a.as_ptr()));
    let result = unsafe { ShellExecuteW(None, verb, &HSTRING::from(file), args, None, SW_SHOWNORMAL) };
    match result.0 as usize {
        n if n > 32 => Ok(()),
        2 | 3 => Err(format!("Windows cannot find \u{201c}{file}\u{201d}.")),
        5 => Err("Access denied or the administrator prompt was declined.".into()),
        n => Err(format!("Could not start \u{201c}{file}\u{201d} (error {n}).")),
    }
}

/// `"C:\a b\x.exe" -y` → (`C:\a b\x.exe`, `-y`); unquoted: up to the first space.
fn split_command(command: &str) -> (&str, &str) {
    if let Some(rest) = command.strip_prefix('"') {
        return match rest.find('"') {
            Some(end) => (&rest[..end], rest[end + 1..].trim()),
            None => (rest, ""),
        };
    }
    match command.find(' ') {
        Some(i) => (&command[..i], command[i + 1..].trim()),
        None => (command, ""),
    }
}

#[cfg(test)]
mod tests {
    use super::split_command;

    #[test]
    fn splits_program_and_arguments() {
        assert_eq!(split_command(r#""C:\a b\x.exe" -y"#), (r"C:\a b\x.exe", "-y"));
        assert_eq!(split_command("notepad file.txt"), ("notepad", "file.txt"));
        assert_eq!(split_command("cmd"), ("cmd", ""));
    }
}
