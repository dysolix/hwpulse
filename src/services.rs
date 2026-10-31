//! Windows services, as on Task Manager's Services tab: one call to the Service
//! Control Manager lists them with state and process id; start type and svchost group
//! come from each service's registry key. Starting and stopping needs admin for most.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{ERROR_ACCESS_DENIED, ERROR_SERVICE_ALREADY_RUNNING, ERROR_SERVICE_NOT_ACTIVE};
use windows::Win32::System::Registry::{
    HKEY_LOCAL_MACHINE, RRF_RT_REG_DWORD, RRF_RT_REG_EXPAND_SZ, RRF_RT_REG_SZ, RegGetValueW,
};
use windows::Win32::System::Services::{
    CloseServiceHandle, ControlService, ENUM_SERVICE_STATUS_PROCESSW, EnumServicesStatusExW, OpenSCManagerW,
    OpenServiceW, QueryServiceStatus, SC_ENUM_PROCESS_INFO, SC_HANDLE, SC_MANAGER_CONNECT,
    SC_MANAGER_ENUMERATE_SERVICE, SERVICE_CONTROL_STOP, SERVICE_QUERY_STATUS, SERVICE_START, SERVICE_STATE_ALL,
    SERVICE_STATUS, SERVICE_STATUS_CURRENT_STATE, SERVICE_STOP, SERVICE_STOPPED, SERVICE_WIN32, StartServiceW,
};
use windows::core::{HSTRING, PCWSTR};

use crate::tasks::Error;

#[derive(Clone, Debug)]
pub struct Service {
    pub name: String,
    pub display: String,
    /// 0 while stopped.
    pub pid: u32,
    pub status: Status,
    /// The svchost group (`-k netsvcs`), if any.
    pub group: Option<String>,
    pub start: Option<StartType>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Status {
    Running,
    Starting,
    Stopping,
    Paused,
    Stopped,
}

impl Status {
    pub fn label(self) -> &'static str {
        match self {
            Status::Running => "Running",
            Status::Starting => "Starting",
            Status::Stopping => "Stopping",
            Status::Paused => "Paused",
            Status::Stopped => "Stopped",
        }
    }

    fn from(state: SERVICE_STATUS_CURRENT_STATE) -> Self {
        match state.0 {
            1 => Status::Stopped,
            2 => Status::Starting,
            3 => Status::Stopping,
            4 => Status::Running,
            _ => Status::Paused, // continue/pause pending, paused
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum StartType {
    Automatic,
    Delayed,
    Manual,
    Disabled,
}

impl StartType {
    pub fn label(self) -> &'static str {
        match self {
            StartType::Automatic => "Automatic",
            StartType::Delayed => "Automatic (delayed)",
            StartType::Manual => "Manual",
            StartType::Disabled => "Disabled",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Control {
    Start,
    Stop,
    Restart,
}

struct Handle(SC_HANDLE);

impl Drop for Handle {
    fn drop(&mut self) {
        let _ = unsafe { CloseServiceHandle(self.0) };
    }
}

/// Win32 services plus per-user service instances (`CDPUserSvc_1a2b3`), like Task Manager.
const TYPES: u32 = SERVICE_WIN32.0 | 0x40 | 0x80;

pub fn list() -> Result<Vec<Service>, String> {
    let scm = unsafe { OpenSCManagerW(PCWSTR::null(), PCWSTR::null(), SC_MANAGER_ENUMERATE_SERVICE) }
        .map(Handle)
        .map_err(|e| e.message())?;
    // 256 KB is the most the call accepts; more services follow via the resume handle.
    let mut buffer = vec![0u64; 32 * 1024];
    let mut resume = 0u32;
    let mut services = Vec::new();
    loop {
        let (mut needed, mut count) = (0u32, 0u32);
        let bytes = unsafe { std::slice::from_raw_parts_mut(buffer.as_mut_ptr().cast::<u8>(), buffer.len() * 8) };
        let result = unsafe {
            EnumServicesStatusExW(
                scm.0,
                SC_ENUM_PROCESS_INFO,
                windows::Win32::System::Services::ENUM_SERVICE_TYPE(TYPES),
                SERVICE_STATE_ALL,
                Some(bytes),
                &mut needed,
                &mut count,
                Some(&mut resume),
                PCWSTR::null(),
            )
        };
        // SAFETY: the call wrote `count` records (with their strings) into the buffer.
        let records = unsafe {
            std::slice::from_raw_parts(buffer.as_ptr() as *const ENUM_SERVICE_STATUS_PROCESSW, count as usize)
        };
        for r in records {
            let name = unsafe { r.lpServiceName.to_string() }.unwrap_or_default();
            let (group, start) = cached_config(&name);
            services.push(Service {
                display: unsafe { r.lpDisplayName.to_string() }.unwrap_or_default(),
                pid: r.ServiceStatusProcess.dwProcessId,
                status: Status::from(r.ServiceStatusProcess.dwCurrentState),
                group,
                start,
                name,
            });
        }
        match result {
            Ok(()) => return Ok(services),
            // ERROR_MORE_DATA: the rest follows with the same resume handle.
            Err(e) if e.code() == windows::Win32::Foundation::ERROR_MORE_DATA.to_hresult() => {
                if count == 0 {
                    return Err("a service entry does not fit".into());
                }
            }
            Err(e) => return Err(e.message()),
        }
    }
}

/// Names of the services running in process `pid`.
pub fn for_pid(pid: u32) -> Vec<String> {
    list().unwrap_or_default().into_iter().filter(|s| s.pid == pid && pid != 0).map(|s| s.name).collect()
}

/// [`config`], re-read at most every 30 s per service: the registry reads are most of
/// what listing costs, and start types rarely change.
fn cached_config(name: &str) -> (Option<String>, Option<StartType>) {
    type Cache = HashMap<String, (Instant, Option<String>, Option<StartType>)>;
    static CACHE: Mutex<Option<Cache>> = Mutex::new(None);
    let mut cache = CACHE.lock().unwrap();
    let cache = cache.get_or_insert_with(HashMap::new);
    if let Some((read, group, start)) = cache.get(name)
        && read.elapsed() < Duration::from_secs(30)
    {
        return (group.clone(), *start);
    }
    let (group, start) = config(name);
    cache.insert(name.to_owned(), (Instant::now(), group.clone(), start));
    (group, start)
}

fn config(name: &str) -> (Option<String>, Option<StartType>) {
    let key = HSTRING::from(format!(r"SYSTEM\CurrentControlSet\Services\{name}"));
    let dword = |value: &str| {
        let mut data = 0u32;
        let mut size = 4u32;
        unsafe {
            RegGetValueW(
                HKEY_LOCAL_MACHINE,
                &key,
                &HSTRING::from(value),
                RRF_RT_REG_DWORD,
                None,
                Some(&mut data as *mut _ as *mut _),
                Some(&mut size),
            )
        }
        .ok()
        .ok()
        .map(|_| data)
    };
    let start = match dword("Start") {
        Some(2) if dword("DelayedAutostart") == Some(1) => Some(StartType::Delayed),
        Some(0..=2) => Some(StartType::Automatic),
        Some(3) => Some(StartType::Manual),
        Some(4) => Some(StartType::Disabled),
        _ => None,
    };

    let mut buffer = [0u16; 512];
    let mut size = (buffer.len() * 2) as u32;
    let path = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            &key,
            &HSTRING::from("ImagePath"),
            RRF_RT_REG_SZ | RRF_RT_REG_EXPAND_SZ | windows::Win32::System::Registry::RRF_NOEXPAND,
            None,
            Some(buffer.as_mut_ptr().cast()),
            Some(&mut size),
        )
    }
    .ok()
    .ok()
    .map(|_| {
        let end = buffer.iter().position(|&c| c == 0).unwrap_or(buffer.len());
        String::from_utf16_lossy(&buffer[..end])
    });
    (path.as_deref().and_then(group), start)
}

/// `C:\Windows\system32\svchost.exe -k netsvcs -p` → `netsvcs`.
fn group(image_path: &str) -> Option<String> {
    let mut words = image_path.split_whitespace();
    words.find(|w| w.eq_ignore_ascii_case("-k"))?;
    words.next().map(str::to_string)
}

/// Starts, stops or restarts a service and waits (up to 30 s) for a stop to finish.
/// Blocks, so call it off the UI thread.
pub fn control(name: &str, control: Control) -> Result<(), Error> {
    let denied = |e: windows::core::Error| {
        if e.code() == ERROR_ACCESS_DENIED.to_hresult() { Error::AccessDenied } else { Error::Other(e.message()) }
    };
    let scm =
        unsafe { OpenSCManagerW(PCWSTR::null(), PCWSTR::null(), SC_MANAGER_CONNECT) }.map(Handle).map_err(denied)?;
    let access = SERVICE_START | SERVICE_STOP | SERVICE_QUERY_STATUS;
    let service = unsafe { OpenServiceW(scm.0, &HSTRING::from(name), access) }.map(Handle).map_err(denied)?;

    if matches!(control, Control::Stop | Control::Restart) {
        let mut status = SERVICE_STATUS::default();
        match unsafe { ControlService(service.0, SERVICE_CONTROL_STOP, &mut status) } {
            Ok(()) => {}
            Err(e) if e.code() == ERROR_SERVICE_NOT_ACTIVE.to_hresult() => {}
            Err(e) => return Err(denied(e)),
        }
        for _ in 0..120 {
            let mut status = SERVICE_STATUS::default();
            unsafe { QueryServiceStatus(service.0, &mut status) }.map_err(denied)?;
            if status.dwCurrentState == SERVICE_STOPPED {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(250));
        }
    }
    if matches!(control, Control::Start | Control::Restart) {
        match unsafe { StartServiceW(service.0, None) } {
            Ok(()) => {}
            Err(e) if e.code() == ERROR_SERVICE_ALREADY_RUNNING.to_hresult() => {}
            Err(e) => return Err(denied(e)),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn svchost_group_from_image_path() {
        assert_eq!(group(r"C:\Windows\system32\svchost.exe -k netsvcs -p").as_deref(), Some("netsvcs"));
        assert_eq!(group(r#""C:\Program Files\x\x.exe""#), None);
    }

    #[test]
    fn lists_running_services_with_process_ids() {
        let services = list().unwrap();
        assert!(services.len() > 50);
        let rpc = services.iter().find(|s| s.name.eq_ignore_ascii_case("RpcSs")).expect("RPC service");
        assert_eq!(rpc.status, Status::Running);
        assert!(rpc.pid != 0 && rpc.start == Some(StartType::Automatic) && rpc.group.as_deref() == Some("rpcss"));
    }
}
