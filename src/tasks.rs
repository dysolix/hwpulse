//! Acting on processes (end, suspend, resume) and running hwmon as administrator.
//! Without admin only processes of the same user that are not elevated can be ended;
//! as admin everything except protected processes (Defender, csrss …), like Task Manager.

use windows::Win32::Foundation::{CloseHandle, ERROR_ACCESS_DENIED, FILETIME, HANDLE, LUID};
use windows::Win32::Security::{
    AdjustTokenPrivileges, GetTokenInformation, LUID_AND_ATTRIBUTES, LookupPrivilegeValueW, SE_DEBUG_NAME,
    SE_PRIVILEGE_ENABLED, TOKEN_ADJUST_PRIVILEGES, TOKEN_ELEVATION, TOKEN_PRIVILEGES, TOKEN_QUERY, TokenElevation,
};
use windows::Win32::System::Threading::{
    GetCurrentProcess, GetProcessTimes, IsProcessCritical, OpenProcess, OpenProcessToken, PROCESS_ACCESS_RIGHTS,
    PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SUSPEND_RESUME, PROCESS_TERMINATE, TerminateProcess,
};
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
use windows::core::{HSTRING, w};

#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "system" {
    // Undocumented but stable since XP; what Resource Monitor and Process Explorer use.
    fn NtSuspendProcess(process: HANDLE) -> i32;
    fn NtResumeProcess(process: HANDLE) -> i32;
}

/// A running process, identified by id and start time so a reused id is never hit.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Target {
    pub pid: u32,
    pub create_time: i64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Action {
    End,
    Suspend,
    Resume,
}

#[derive(Debug, PartialEq)]
pub enum Error {
    /// Elevated, another user's or SYSTEM; may work as administrator.
    AccessDenied,
    /// Already ended (or its id now belongs to another process).
    Gone,
    Other(String),
}

pub fn run(action: Action, target: Target) -> Result<(), Error> {
    let access = match action {
        Action::End => PROCESS_TERMINATE,
        Action::Suspend | Action::Resume => PROCESS_SUSPEND_RESUME,
    };
    let handle = open(target, access)?;

    match action {
        Action::End => unsafe { TerminateProcess(handle.0, 1) }.map_err(|e| Error::Other(e.message())),
        Action::Suspend => status(unsafe { NtSuspendProcess(handle.0) }),
        Action::Resume => status(unsafe { NtResumeProcess(handle.0) }),
    }
}

/// Whether ending the process stops Windows (csrss, wininit …). `None` without access.
pub fn is_critical(target: Target) -> Option<bool> {
    let handle = open(target, PROCESS_QUERY_LIMITED_INFORMATION).ok()?;
    let mut critical = Default::default();
    unsafe { IsProcessCritical(handle.0, &mut critical) }.ok()?;
    Some(critical.as_bool())
}

fn status(code: i32) -> Result<(), Error> {
    /// `STATUS_ACCESS_DENIED`
    const ACCESS_DENIED: i32 = 0xC000_0022_u32 as i32;
    match code {
        c if c >= 0 => Ok(()),
        ACCESS_DENIED => Err(Error::AccessDenied),
        c => Err(Error::Other(format!("NTSTATUS {c:#x}"))),
    }
}

struct Handle(HANDLE);

impl Drop for Handle {
    fn drop(&mut self) {
        unsafe { CloseHandle(self.0) }.ok();
    }
}

/// Opens the process with `access` (plus the right to check its start time).
fn open(target: Target, access: PROCESS_ACCESS_RIGHTS) -> Result<Handle, Error> {
    let handle = unsafe { OpenProcess(access | PROCESS_QUERY_LIMITED_INFORMATION, false, target.pid) }
        .map(Handle)
        .map_err(|e| if e.code() == ERROR_ACCESS_DENIED.to_hresult() { Error::AccessDenied } else { Error::Gone })?;
    let mut times = [FILETIME::default(); 4];
    let [created, exited, kernel, user] = &mut times;
    unsafe { GetProcessTimes(handle.0, created, exited, kernel, user) }.map_err(|e| Error::Other(e.message()))?;
    let created = ((created.dwHighDateTime as i64) << 32) | created.dwLowDateTime as i64;
    if created != target.create_time {
        return Err(Error::Gone);
    }
    Ok(handle)
}

/// Whether hwmon itself runs as administrator.
pub fn is_elevated() -> bool {
    let mut token = HANDLE::default();
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) }.is_err() {
        return false;
    }
    let mut elevation = TOKEN_ELEVATION::default();
    let mut len = 0;
    let ok = unsafe {
        GetTokenInformation(
            token,
            TokenElevation,
            Some(&mut elevation as *mut _ as *mut _),
            size_of::<TOKEN_ELEVATION>() as u32,
            &mut len,
        )
    };
    unsafe { CloseHandle(token) }.ok();
    ok.is_ok() && elevation.TokenIsElevated != 0
}

/// As administrator, switches on the debug privilege so services and other users'
/// processes can be opened too, as Task Manager does. Harmless when not elevated.
pub fn enable_debug_privilege() {
    let mut token = HANDLE::default();
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_ADJUST_PRIVILEGES, &mut token) }.is_err() {
        return;
    }
    let mut luid = LUID::default();
    if unsafe { LookupPrivilegeValueW(None, SE_DEBUG_NAME, &mut luid) }.is_ok() {
        let privileges = TOKEN_PRIVILEGES {
            PrivilegeCount: 1,
            Privileges: [LUID_AND_ATTRIBUTES { Luid: luid, Attributes: SE_PRIVILEGE_ENABLED }],
        };
        let _ = unsafe { AdjustTokenPrivileges(token, false, Some(&privileges), 0, None, None) };
    }
    unsafe { CloseHandle(token) }.ok();
}

/// Starts this exe again as administrator (UAC prompt), opening `page`. False when
/// the prompt was declined, in which case the current instance should keep running.
pub fn restart_as_admin(page: &str) -> bool {
    let Ok(exe) = std::env::current_exe() else { return false };
    let args = HSTRING::from(format!("--page {page}"));
    let result =
        unsafe { ShellExecuteW(None, w!("runas"), &HSTRING::from(exe.as_os_str()), &args, None, SW_SHOWNORMAL) };
    // Values above 32 mean success.
    result.0 as usize > 32
}
