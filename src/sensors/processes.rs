//! All processes in one system call (`NtQuerySystemInformation`), the way Task
//! Manager reads them: much cheaper than PDH's per-process counters. Gives CPU time,
//! memory, I/O and thread states per process, plus process/thread/handle totals.
//! Facts that need a process handle (user, architecture, description) are looked up
//! by [`Details`] only for the Processes page, once per process.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use windows::Wdk::System::SystemInformation::{NtQuerySystemInformation, SystemProcessInformation};
use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::Security::{
    GetTokenInformation, LookupAccountSidW, SID_NAME_USE, TOKEN_ELEVATION, TOKEN_QUERY, TOKEN_USER, TokenElevation,
    TokenUser,
};
use windows::Win32::Storage::FileSystem::{GetFileVersionInfoSizeW, GetFileVersionInfoW, VerQueryValueW};
use windows::Win32::System::SystemInformation::{
    IMAGE_FILE_MACHINE, IMAGE_FILE_MACHINE_AMD64, IMAGE_FILE_MACHINE_ARM64, IMAGE_FILE_MACHINE_ARMNT,
    IMAGE_FILE_MACHINE_I386, IMAGE_FILE_MACHINE_UNKNOWN,
};
use windows::Win32::System::Threading::{
    IsWow64Process2, OpenProcess, OpenProcessToken, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
    QueryFullProcessImageNameW,
};
use windows::core::{HSTRING, PWSTR, w};

/// Executable file name of a running process, e.g. `"dwm.exe"`. Needs only the
/// limited query right, which works for nearly every process without admin.
pub fn image_name(pid: u32) -> Option<String> {
    let handle = Handle::open(pid)?;
    image_path(handle.0)?.rsplit('\\').next().map(str::to_string)
}

/// A process handle with the limited query right, closed on drop.
struct Handle(HANDLE);

impl Handle {
    fn open(pid: u32) -> Option<Self> {
        unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }.ok().map(Self)
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        unsafe { CloseHandle(self.0) }.ok();
    }
}

fn image_path(handle: HANDLE) -> Option<String> {
    let mut buffer = [0u16; 512];
    let mut len = buffer.len() as u32;
    unsafe { QueryFullProcessImageNameW(handle, PROCESS_NAME_WIN32, PWSTR(buffer.as_mut_ptr()), &mut len) }.ok()?;
    Some(String::from_utf16_lossy(&buffer[..len as usize]))
}

/// `STATUS_INFO_LENGTH_MISMATCH`: the buffer was too small.
const LENGTH_MISMATCH: i32 = 0xC000_0004_u32 as i32;

/// The documented prefix of `SYSTEM_PROCESS_INFORMATION` (64-bit layout); thread
/// entries follow each record and are skipped via `next_entry_offset`.
#[repr(C)]
struct ProcessRecord {
    next_entry_offset: u32,
    number_of_threads: u32,
    working_set_private_size: i64,
    hard_fault_count: u32,
    number_of_threads_high_watermark: u32,
    cycle_time: u64,
    create_time: i64,
    user_time: i64,
    kernel_time: i64,
    image_name_length: u16,
    image_name_maximum_length: u16,
    image_name_buffer: *const u16,
    base_priority: i32,
    unique_process_id: usize,
    inherited_from_unique_process_id: usize,
    handle_count: u32,
    session_id: u32,
    unique_process_key: usize,
    peak_virtual_size: usize,
    virtual_size: usize,
    page_fault_count: u32,
    peak_working_set_size: usize,
    working_set_size: usize,
    quota_peak_paged_pool_usage: usize,
    quota_paged_pool_usage: usize,
    quota_peak_non_paged_pool_usage: usize,
    quota_non_paged_pool_usage: usize,
    pagefile_usage: usize,
    peak_pagefile_usage: usize,
    private_page_count: usize,
    read_operation_count: i64,
    write_operation_count: i64,
    other_operation_count: i64,
    read_transfer_count: i64,
    write_transfer_count: i64,
    other_transfer_count: i64,
}

/// `SYSTEM_THREAD_INFORMATION` (64-bit layout); `number_of_threads` of them follow
/// each [`ProcessRecord`].
#[repr(C)]
struct ThreadRecord {
    kernel_time: i64,
    user_time: i64,
    create_time: i64,
    wait_time: u32,
    start_address: usize,
    client_id: [usize; 2],
    priority: i32,
    base_priority: i32,
    context_switches: u32,
    thread_state: u32,
    wait_reason: u32,
}

const _: () = assert!(size_of::<ProcessRecord>() == 0x100 && size_of::<ThreadRecord>() == 0x50);

/// `KTHREAD_STATE::Waiting` and `KWAIT_REASON::Suspended`.
const WAITING: u32 = 5;
const SUSPENDED: u32 = 5;

#[derive(Clone, Debug)]
pub struct Process {
    pub pid: u32,
    /// With the id, identifies a process across samples (ids are reused).
    pub create_time: i64,
    pub name: String,
    /// Share of all logical processors, 0–100.
    pub cpu: f64,
    /// Private working set: memory only this process uses (Task Manager's "Memory").
    pub private_bytes: u64,
    /// Read + write throughput to files, disks and devices.
    pub io_bytes_per_sec: f64,
    pub threads: u32,
    pub handles: u32,
    /// Every thread is suspended, e.g. a UWP app in the background or a process in a debugger.
    pub suspended: bool,
}

#[derive(Default)]
pub struct Snapshot {
    pub processes: Vec<Process>,
    pub threads: u64,
    pub handles: u64,
    /// Working set of "Memory Compression", i.e. compressed RAM.
    pub compressed_bytes: Option<u64>,
}

pub struct Sampler {
    logical_processors: f64,
    buffer: Vec<u64>,
    /// Per process id (and creation time, as ids are reused): CPU time and I/O bytes.
    previous: HashMap<(usize, i64), (i64, i64)>,
    previous_at: Option<Instant>,
}

impl Sampler {
    pub fn new(logical_processors: u32) -> Self {
        Self {
            logical_processors: logical_processors.max(1) as f64,
            buffer: vec![0; 64 * 1024],
            previous: HashMap::new(),
            previous_at: None,
        }
    }

    pub fn sample(&mut self) -> Snapshot {
        if !self.query() {
            return Snapshot::default();
        }
        let now = Instant::now();
        let elapsed = self.previous_at.map(|t| now.duration_since(t).as_secs_f64()).filter(|s| *s > 0.0);
        self.previous_at = Some(now);

        let mut snapshot = Snapshot::default();
        let mut current = HashMap::with_capacity(self.previous.len());
        let base = self.buffer.as_ptr() as *const u8;
        let mut offset = 0usize;
        loop {
            // SAFETY: the kernel wrote a chain of records into the buffer; offsets
            // stay inside it and every record starts 8-byte aligned.
            let record = unsafe { &*(base.add(offset) as *const ProcessRecord) };
            snapshot.threads += record.number_of_threads as u64;
            snapshot.handles += record.handle_count as u64;

            let pid = record.unique_process_id;
            if pid != 0 {
                let name = if record.image_name_buffer.is_null() {
                    String::from("System")
                } else {
                    let chars = unsafe {
                        std::slice::from_raw_parts(record.image_name_buffer, record.image_name_length as usize / 2)
                    };
                    String::from_utf16_lossy(chars)
                };
                let cpu_time = record.user_time + record.kernel_time; // 100 ns units
                let io = record.read_transfer_count + record.write_transfer_count;
                let key = (pid, record.create_time);
                let (cpu, io_rate) = match (self.previous.get(&key), elapsed) {
                    (Some(&(cpu0, io0)), Some(secs)) => (
                        ((cpu_time - cpu0) as f64 / (secs * 1e7 * self.logical_processors) * 100.0).clamp(0.0, 100.0),
                        ((io - io0) as f64 / secs).max(0.0),
                    ),
                    _ => (0.0, 0.0),
                };
                current.insert(key, (cpu_time, io));
                if name == "Memory Compression" {
                    snapshot.compressed_bytes = Some(record.working_set_size as u64);
                }
                // SAFETY: the thread entries follow the record inside the same buffer.
                let threads = unsafe {
                    std::slice::from_raw_parts(
                        base.add(offset + size_of::<ProcessRecord>()) as *const ThreadRecord,
                        record.number_of_threads as usize,
                    )
                };
                let suspended = !threads.is_empty()
                    && threads.iter().all(|t| t.thread_state == WAITING && t.wait_reason == SUSPENDED);
                snapshot.processes.push(Process {
                    pid: pid as u32,
                    create_time: record.create_time,
                    name,
                    cpu,
                    private_bytes: record.working_set_private_size.max(0) as u64,
                    io_bytes_per_sec: io_rate,
                    threads: record.number_of_threads,
                    handles: record.handle_count,
                    suspended,
                });
            }

            if record.next_entry_offset == 0 {
                break;
            }
            offset += record.next_entry_offset as usize;
        }
        self.previous = current;
        snapshot
    }

    /// Fills the buffer, growing it while the process list does not fit.
    fn query(&mut self) -> bool {
        for _ in 0..4 {
            let bytes = (self.buffer.len() * 8) as u32;
            let mut needed = 0u32;
            let status = unsafe {
                NtQuerySystemInformation(SystemProcessInformation, self.buffer.as_mut_ptr().cast(), bytes, &mut needed)
            };
            if status.0 == LENGTH_MISMATCH {
                // Leave room for processes started in the meantime.
                self.buffer.resize((needed as usize + 64 * 1024).div_ceil(8), 0);
                continue;
            }
            return status.is_ok();
        }
        false
    }
}

/// Facts about a process that need a handle or a file read. They never change while
/// the process runs, so [`Details`] looks them up once per process.
#[derive(Clone, Debug, Default)]
pub struct Facts {
    /// `None` when the process does not let us read its token (elevated, SYSTEM).
    pub user: Option<String>,
    pub elevated: Option<bool>,
    /// E.g. `"x64"`; `None` without access.
    pub architecture: Option<&'static str>,
    pub path: Option<String>,
    /// The executable's file description, e.g. `"Desktop Window Manager"`.
    pub description: Option<String>,
}

/// A process plus its [`Facts`], one row of the Processes page.
#[derive(Clone, Debug)]
pub struct Row {
    pub process: Process,
    pub facts: Arc<Facts>,
}

/// Cache of [`Facts`] per process and of descriptions per executable.
#[derive(Default)]
pub struct Details {
    facts: HashMap<(u32, i64), Arc<Facts>>,
    descriptions: HashMap<String, Option<String>>,
}

impl Details {
    /// Rows for all `processes`; only processes not seen before cost a lookup.
    pub fn rows(&mut self, processes: &[Process]) -> Vec<Row> {
        let mut facts = HashMap::with_capacity(processes.len());
        let rows = processes
            .iter()
            .map(|p| {
                let key = (p.pid, p.create_time);
                let f = self.facts.remove(&key).unwrap_or_else(|| Arc::new(self.lookup(p.pid)));
                facts.insert(key, f.clone());
                Row { process: p.clone(), facts: f }
            })
            .collect();
        // Forget processes that ended.
        self.facts = facts;
        rows
    }

    fn lookup(&mut self, pid: u32) -> Facts {
        let Some(handle) = Handle::open(pid) else { return Facts::default() };
        let (user, elevated) = token_facts(handle.0);
        let path = image_path(handle.0);
        let description = path.as_ref().and_then(|path| {
            self.descriptions.entry(path.to_lowercase()).or_insert_with(|| file_description(path)).clone()
        });
        Facts { user, elevated, architecture: architecture(handle.0), path, description }
    }
}

fn token_facts(process: HANDLE) -> (Option<String>, Option<bool>) {
    let mut token = HANDLE::default();
    if unsafe { OpenProcessToken(process, TOKEN_QUERY, &mut token) }.is_err() {
        return (None, None);
    }
    let mut elevation = TOKEN_ELEVATION::default();
    let mut len = 0;
    let elevated = unsafe {
        GetTokenInformation(
            token,
            TokenElevation,
            Some(&mut elevation as *mut _ as *mut _),
            size_of::<TOKEN_ELEVATION>() as u32,
            &mut len,
        )
    }
    .ok()
    .map(|_| elevation.TokenIsElevated != 0);

    // TOKEN_USER is followed by the SID it points to; 256 bytes fit any SID.
    let mut buffer = [0u64; 32];
    let user = unsafe {
        GetTokenInformation(token, TokenUser, Some(buffer.as_mut_ptr().cast()), size_of_val(&buffer) as u32, &mut len)
    }
    .ok()
    .and_then(|_| {
        let sid = unsafe { &*(buffer.as_ptr() as *const TOKEN_USER) }.User.Sid;
        let (mut name, mut domain) = ([0u16; 128], [0u16; 128]);
        let (mut name_len, mut domain_len) = (name.len() as u32, domain.len() as u32);
        let mut kind = SID_NAME_USE::default();
        unsafe {
            LookupAccountSidW(
                None,
                sid,
                Some(PWSTR(name.as_mut_ptr())),
                &mut name_len,
                Some(PWSTR(domain.as_mut_ptr())),
                &mut domain_len,
                &mut kind,
            )
        }
        .ok()?;
        Some(String::from_utf16_lossy(&name[..name_len as usize]))
    });
    unsafe { CloseHandle(token) }.ok();
    (user, elevated)
}

fn architecture(process: HANDLE) -> Option<&'static str> {
    let (mut machine, mut native) = (IMAGE_FILE_MACHINE::default(), IMAGE_FILE_MACHINE::default());
    unsafe { IsWow64Process2(process, &mut machine, Some(&mut native)) }.ok()?;
    // UNKNOWN means not running under WOW64, i.e. the native architecture.
    let machine = if machine == IMAGE_FILE_MACHINE_UNKNOWN { native } else { machine };
    Some(match machine {
        IMAGE_FILE_MACHINE_AMD64 => "x64",
        IMAGE_FILE_MACHINE_I386 => "x86",
        IMAGE_FILE_MACHINE_ARM64 => "ARM64",
        IMAGE_FILE_MACHINE_ARMNT => "ARM",
        _ => "other",
    })
}

/// `FileDescription` from the executable's version resource, in its first language.
fn file_description(path: &str) -> Option<String> {
    let path = HSTRING::from(path);
    let size = unsafe { GetFileVersionInfoSizeW(&path, None) };
    if size == 0 {
        return None;
    }
    let mut data = vec![0u8; size as usize];
    unsafe { GetFileVersionInfoW(&path, None, size, data.as_mut_ptr().cast()) }.ok()?;

    let mut ptr = std::ptr::null_mut();
    let mut len = 0u32;
    unsafe { VerQueryValueW(data.as_ptr().cast(), w!("\\VarFileInfo\\Translation"), &mut ptr, &mut len) }.ok().ok()?;
    if len < 4 {
        return None;
    }
    let [lang, codepage] = unsafe { *(ptr as *const [u16; 2]) };
    let key = HSTRING::from(format!("\\StringFileInfo\\{lang:04x}{codepage:04x}\\FileDescription"));
    unsafe { VerQueryValueW(data.as_ptr().cast(), &key, &mut ptr, &mut len) }.ok().ok()?;
    let chars = unsafe { std::slice::from_raw_parts(ptr as *const u16, len as usize) };
    let text = String::from_utf16_lossy(chars).trim_end_matches('\0').trim().to_string();
    (!text.is_empty()).then_some(text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tasks::{self, Action, Target};

    fn find(sampler: &mut Sampler, pid: u32) -> Option<Process> {
        sampler.sample().processes.into_iter().find(|p| p.pid == pid)
    }

    #[test]
    fn suspend_resume_and_end_a_child() {
        let mut child = std::process::Command::new("ping").args(["-n", "30", "127.0.0.1"]).spawn().unwrap();
        let pid = child.id();
        let mut sampler = Sampler::new(1);
        let p = find(&mut sampler, pid).expect("child listed");
        let target = Target { pid, create_time: p.create_time };

        let facts = Details::default().rows(std::slice::from_ref(&p)).remove(0).facts;
        assert!(facts.user.is_some() && facts.architecture.is_some() && facts.path.is_some());
        assert_eq!(tasks::is_critical(target), Some(false));

        tasks::run(Action::Suspend, target).unwrap();
        assert!(find(&mut sampler, pid).unwrap().suspended);
        tasks::run(Action::Resume, target).unwrap();
        assert!(!find(&mut sampler, pid).unwrap().suspended);

        // A wrong start time stands for a reused id and must not be hit.
        let stale = Target { create_time: target.create_time - 1, ..target };
        assert_eq!(tasks::run(Action::End, stale), Err(tasks::Error::Gone));
        tasks::run(Action::End, target).unwrap();
        assert_eq!(child.wait().unwrap().code(), Some(1));
    }
}
