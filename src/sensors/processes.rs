//! All processes in one system call (`NtQuerySystemInformation`), the way Task
//! Manager reads them: much cheaper than PDH's per-process counters. Gives CPU time,
//! private memory and I/O per process, plus process/thread/handle totals.

use std::collections::HashMap;
use std::time::Instant;

use windows::Wdk::System::SystemInformation::{NtQuerySystemInformation, SystemProcessInformation};
use windows::Win32::Foundation::CloseHandle;
use windows::Win32::System::Threading::{
    OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
};
use windows::core::PWSTR;

/// Executable file name of a running process, e.g. `"dwm.exe"`. Needs only the
/// limited query right, which works for nearly every process without admin.
pub fn image_name(pid: u32) -> Option<String> {
    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }.ok()?;
    let mut buffer = [0u16; 512];
    let mut len = buffer.len() as u32;
    let ok = unsafe { QueryFullProcessImageNameW(handle, PROCESS_NAME_WIN32, PWSTR(buffer.as_mut_ptr()), &mut len) };
    unsafe { CloseHandle(handle) }.ok();
    ok.ok()?;
    let path = String::from_utf16_lossy(&buffer[..len as usize]);
    path.rsplit('\\').next().map(str::to_string)
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

pub struct Process {
    pub name: String,
    /// Share of all logical processors, 0–100.
    pub cpu: f64,
    /// Private working set: memory only this process uses (Task Manager's "Memory").
    pub private_bytes: u64,
    /// Read + write throughput to files, disks and devices.
    pub io_bytes_per_sec: f64,
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
                snapshot.processes.push(Process {
                    name,
                    cpu,
                    private_bytes: record.working_set_private_size.max(0) as u64,
                    io_bytes_per_sec: io_rate,
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
