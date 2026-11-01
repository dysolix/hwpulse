//! Acting on processes (end, suspend, resume, priority, affinity, efficiency mode,
//! dump files) and running hwpulse as administrator. Without admin only processes of the
//! same user that are not elevated can be changed; as admin everything except protected
//! processes (Defender, csrss …), like Task Manager.

use std::path::PathBuf;

use windows::Win32::Foundation::{CloseHandle, ERROR_ACCESS_DENIED, FILETIME, HANDLE, LUID};
use windows::Win32::Security::{
    AdjustTokenPrivileges, GetTokenInformation, LUID_AND_ATTRIBUTES, LookupPrivilegeValueW, SE_DEBUG_NAME,
    SE_PRIVILEGE_ENABLED, TOKEN_ADJUST_PRIVILEGES, TOKEN_ELEVATION, TOKEN_PRIVILEGES, TOKEN_QUERY, TokenElevation,
};
use windows::Win32::System::Diagnostics::Debug::{
    MINIDUMP_TYPE, MiniDumpWithFullMemory, MiniDumpWithHandleData, MiniDumpWithThreadInfo, MiniDumpWithUnloadedModules,
    MiniDumpWriteDump,
};
use windows::Win32::System::Threading::{
    ABOVE_NORMAL_PRIORITY_CLASS, BELOW_NORMAL_PRIORITY_CLASS, GetCurrentProcess, GetProcessAffinityMask,
    GetProcessTimes, HIGH_PRIORITY_CLASS, IDLE_PRIORITY_CLASS, IsProcessCritical, NORMAL_PRIORITY_CLASS, OpenProcess,
    OpenProcessToken, PROCESS_ACCESS_RIGHTS, PROCESS_CREATION_FLAGS, PROCESS_POWER_THROTTLING_CURRENT_VERSION,
    PROCESS_POWER_THROTTLING_EXECUTION_SPEED, PROCESS_POWER_THROTTLING_STATE, PROCESS_QUERY_INFORMATION,
    PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SET_INFORMATION, PROCESS_SUSPEND_RESUME, PROCESS_TERMINATE,
    PROCESS_VM_READ, ProcessPowerThrottling, REALTIME_PRIORITY_CLASS, SetPriorityClass, SetProcessAffinityMask,
    SetProcessInformation, TerminateProcess,
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
    Priority(Priority),
    /// Efficiency mode on (EcoQoS and low priority) or off (normal priority).
    Efficiency(bool),
    /// Bit mask of the logical processors the process may run on.
    Affinity(u64),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Priority {
    Realtime,
    High,
    AboveNormal,
    Normal,
    BelowNormal,
    Low,
}

impl Priority {
    pub const ALL: [Priority; 6] = [
        Priority::Realtime,
        Priority::High,
        Priority::AboveNormal,
        Priority::Normal,
        Priority::BelowNormal,
        Priority::Low,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Priority::Realtime => "Realtime",
            Priority::High => "High",
            Priority::AboveNormal => "Above normal",
            Priority::Normal => "Normal",
            Priority::BelowNormal => "Below normal",
            Priority::Low => "Low",
        }
    }

    /// From a process's base priority (4, 6, 8, 10, 13, 24).
    pub fn from_base(base: i32) -> Option<Self> {
        Some(match base {
            24.. => Priority::Realtime,
            13..=23 => Priority::High,
            10..=12 => Priority::AboveNormal,
            8 | 9 => Priority::Normal,
            6 | 7 => Priority::BelowNormal,
            1..=5 => Priority::Low,
            _ => return None,
        })
    }

    fn class(self) -> PROCESS_CREATION_FLAGS {
        match self {
            Priority::Realtime => REALTIME_PRIORITY_CLASS,
            Priority::High => HIGH_PRIORITY_CLASS,
            Priority::AboveNormal => ABOVE_NORMAL_PRIORITY_CLASS,
            Priority::Normal => NORMAL_PRIORITY_CLASS,
            Priority::BelowNormal => BELOW_NORMAL_PRIORITY_CLASS,
            Priority::Low => IDLE_PRIORITY_CLASS,
        }
    }
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
        Action::Priority(_) | Action::Efficiency(_) | Action::Affinity(_) => PROCESS_SET_INFORMATION,
    };
    let handle = open(target, access)?;
    let win32 = |r: windows::core::Result<()>| r.map_err(win32_error);

    match action {
        Action::End => win32(unsafe { TerminateProcess(handle.0, 1) }),
        Action::Suspend => status(unsafe { NtSuspendProcess(handle.0) }),
        Action::Resume => status(unsafe { NtResumeProcess(handle.0) }),
        Action::Priority(p) => win32(unsafe { SetPriorityClass(handle.0, p.class()) }),
        Action::Efficiency(on) => {
            // Off hands the decision back to Windows, as Task Manager does.
            let speed = if on { PROCESS_POWER_THROTTLING_EXECUTION_SPEED } else { 0 };
            let state = PROCESS_POWER_THROTTLING_STATE {
                Version: PROCESS_POWER_THROTTLING_CURRENT_VERSION,
                ControlMask: speed,
                StateMask: speed,
            };
            win32(unsafe {
                SetProcessInformation(
                    handle.0,
                    ProcessPowerThrottling,
                    &state as *const _ as *const _,
                    size_of::<PROCESS_POWER_THROTTLING_STATE>() as u32,
                )
            })?;
            let class = if on { IDLE_PRIORITY_CLASS } else { NORMAL_PRIORITY_CLASS };
            win32(unsafe { SetPriorityClass(handle.0, class) })
        }
        Action::Affinity(mask) => win32(unsafe { SetProcessAffinityMask(handle.0, mask as usize) }),
    }
}

fn win32_error(e: windows::core::Error) -> Error {
    if e.code() == ERROR_ACCESS_DENIED.to_hresult() { Error::AccessDenied } else { Error::Other(e.message()) }
}

/// The process's affinity mask and the mask of all processors it could use.
pub fn affinity(target: Target) -> Option<(u64, u64)> {
    let handle = open(target, PROCESS_QUERY_LIMITED_INFORMATION).ok()?;
    let (mut process, mut system) = (0usize, 0usize);
    unsafe { GetProcessAffinityMask(handle.0, &mut process, &mut system) }.ok()?;
    Some((process as u64, system as u64))
}

/// Writes a full memory dump to `%TEMP%\{name}.DMP` (where Task Manager puts them)
/// and returns its path. Takes seconds for big processes, so call it off the UI thread.
pub fn dump(target: Target, name: &str) -> Result<PathBuf, Error> {
    use std::os::windows::io::AsRawHandle;
    let handle = open(target, PROCESS_QUERY_INFORMATION | PROCESS_VM_READ)?;
    let stem = name.strip_suffix(".exe").unwrap_or(name);
    // Only ever a new file (`name (2).DMP` when taken): %TEMP% is writable by every
    // program of the user, which could plant a link there to make an elevated hwpulse
    // overwrite a protected file.
    let (path, file) = (1..100)
        .find_map(|n| {
            let name = if n == 1 { format!("{stem}.DMP") } else { format!("{stem} ({n}).DMP") };
            let path = std::env::temp_dir().join(name);
            std::fs::OpenOptions::new().write(true).create_new(true).open(&path).ok().map(|f| (path, f))
        })
        .ok_or_else(|| Error::Other("no free file name in the temporary folder".into()))?;
    let kind = MINIDUMP_TYPE(
        MiniDumpWithFullMemory.0 | MiniDumpWithHandleData.0 | MiniDumpWithThreadInfo.0 | MiniDumpWithUnloadedModules.0,
    );
    let result =
        unsafe { MiniDumpWriteDump(handle.0, target.pid, HANDLE(file.as_raw_handle()), kind, None, None, None) };
    drop(file);
    if let Err(e) = result {
        let _ = std::fs::remove_file(&path);
        return Err(win32_error(e));
    }
    Ok(path)
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

/// Whether hwpulse itself runs as administrator.
pub fn is_elevated() -> bool {
    // Fixed for the process lifetime; asked on every frame by the table pages.
    static ELEVATED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ELEVATED.get_or_init(read_elevation)
}

fn read_elevation() -> bool {
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
