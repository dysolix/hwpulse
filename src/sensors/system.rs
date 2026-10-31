//! CPU, memory, disks and network from Windows itself — no admin, no driver.
//! Live values come from PDH counters (what Task Manager shows); component names are
//! read once at startup via WMI.

use std::collections::HashMap;

use serde::Deserialize;
use windows::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
use windows::Win32::System::SystemInformation::{
    CACHE_RELATIONSHIP, GetLogicalProcessorInformationEx, GetPhysicallyInstalledSystemMemory, GetTickCount64,
    GlobalMemoryStatusEx, MEMORYSTATUSEX, RelationCache, SYSTEM_LOGICAL_PROCESSOR_INFORMATION_EX,
};
use windows::core::HSTRING;

use super::netinfo::{Adapter, Wifi};
use super::pdh::{Counter, Query};
use super::processes::{Details, GpuUse, Process, Row, Sampler, Snapshot};
use super::{
    Breakdown, BreakdownItem, Group, Info, Kind, Processes, Reading, SensorProvider, Unit, info, order, top_processes,
};

pub struct SystemProvider {
    query: Query,
    cpu: Cpu,
    memory: Memory,
    disk: Disk,
    net: Net,
    inventory: Inventory,
    /// Disk instances seen on the last poll, to notice plug/unplug.
    known_disks: Vec<String>,
    sampler: Sampler,
    /// The latest sample's processes, for [`SensorProvider::process_table`].
    processes: Vec<Process>,
    details: Details,
    /// Per-process GPU counters, set up the first time the process table is built.
    gpu: Option<GpuQuery>,
    battery: Option<super::battery::Battery>,
    wifi: Option<Wifi>,
    /// Adapter addresses, refreshed every few polls (they rarely change).
    adapters: Vec<Adapter>,
    polls: u32,
}

struct Cpu {
    utility: Option<Counter>,
    frequency: Option<Counter>,
    performance: Option<Counter>,
    thread_utility: Option<Counter>,
    thread_frequency: Option<Counter>,
    thread_performance: Option<Counter>,
    interrupt: Option<Counter>,
    dpc: Option<Counter>,
    c1: Option<Counter>,
    c2: Option<Counter>,
    c3: Option<Counter>,
    context_switches: Option<Counter>,
    queue: Option<Counter>,
}

struct Memory {
    standby_core: Option<Counter>,
    standby_normal: Option<Counter>,
    standby_reserve: Option<Counter>,
    modified: Option<Counter>,
    paged_pool: Option<Counter>,
    nonpaged_pool: Option<Counter>,
    hard_faults: Option<Counter>,
    page_file: Option<Counter>,
}

struct Disk {
    read: Option<Counter>,
    write: Option<Counter>,
    idle: Option<Counter>,
    read_latency: Option<Counter>,
    write_latency: Option<Counter>,
    queue: Option<Counter>,
}

struct Net {
    received: Option<Counter>,
    sent: Option<Counter>,
    bandwidth: Option<Counter>,
}

impl SystemProvider {
    pub fn new() -> Result<Self, String> {
        // WMI takes ~50 ms; ask it while PDH sets up its counters.
        let inventory = std::thread::spawn(Inventory::load);
        let query = Query::new().ok_or("could not open PDH query")?;
        let cpu = Cpu {
            utility: query.add(r"\Processor Information(_Total)\% Processor Utility"),
            frequency: query.add(r"\Processor Information(_Total)\Processor Frequency"),
            performance: query.add(r"\Processor Information(_Total)\% Processor Performance"),
            thread_utility: query.add(r"\Processor Information(*)\% Processor Utility"),
            thread_frequency: query.add(r"\Processor Information(*)\Processor Frequency"),
            thread_performance: query.add(r"\Processor Information(*)\% Processor Performance"),
            interrupt: query.add(r"\Processor Information(_Total)\% Interrupt Time"),
            dpc: query.add(r"\Processor Information(_Total)\% DPC Time"),
            c1: query.add(r"\Processor Information(_Total)\% C1 Time"),
            c2: query.add(r"\Processor Information(_Total)\% C2 Time"),
            c3: query.add(r"\Processor Information(_Total)\% C3 Time"),
            context_switches: query.add(r"\System\Context Switches/sec"),
            queue: query.add(r"\System\Processor Queue Length"),
        };
        let memory = Memory {
            standby_core: query.add(r"\Memory\Standby Cache Core Bytes"),
            standby_normal: query.add(r"\Memory\Standby Cache Normal Priority Bytes"),
            standby_reserve: query.add(r"\Memory\Standby Cache Reserve Bytes"),
            modified: query.add(r"\Memory\Modified Page List Bytes"),
            paged_pool: query.add(r"\Memory\Pool Paged Bytes"),
            nonpaged_pool: query.add(r"\Memory\Pool Nonpaged Bytes"),
            hard_faults: query.add(r"\Memory\Page Reads/sec"),
            page_file: query.add(r"\Paging File(_Total)\% Usage"),
        };
        let disk = Disk {
            read: query.add(r"\PhysicalDisk(*)\Disk Read Bytes/sec"),
            write: query.add(r"\PhysicalDisk(*)\Disk Write Bytes/sec"),
            idle: query.add(r"\PhysicalDisk(*)\% Idle Time"),
            read_latency: query.add(r"\PhysicalDisk(*)\Avg. Disk sec/Read"),
            write_latency: query.add(r"\PhysicalDisk(*)\Avg. Disk sec/Write"),
            queue: query.add(r"\PhysicalDisk(*)\Current Disk Queue Length"),
        };
        let net = Net {
            received: query.add(r"\Network Interface(*)\Bytes Received/sec"),
            sent: query.add(r"\Network Interface(*)\Bytes Sent/sec"),
            bandwidth: query.add(r"\Network Interface(*)\Current Bandwidth"),
        };
        // Prime rate counters so the first real poll has a baseline.
        query.collect();
        let inventory = inventory.join().unwrap_or_default();
        let mut sampler = Sampler::new(inventory.logical_processors);
        sampler.sample();

        Ok(Self {
            query,
            cpu,
            memory,
            disk,
            net,
            inventory,
            known_disks: Vec::new(),
            sampler,
            processes: Vec::new(),
            details: Details::default(),
            gpu: None,
            battery: super::battery::Battery::detect(),
            wifi: Wifi::open(),
            adapters: Vec::new(),
            polls: 0,
        })
    }

    fn cpu_group(&self, snapshot: &Snapshot) -> Group {
        let q = &self.query;
        let mut readings = Vec::new();

        if let Some(load) = self.cpu.utility.and_then(|c| q.value(c)) {
            readings.push(Reading {
                id: "load",
                label: "CPU load",
                value: load.clamp(0.0, 100.0),
                unit: Unit::Percent,
                range: Some((0.0, 100.0)),
                detail: None,
                featured: false,
                dashboard: true,
            });
        }

        // Effective clock the way Task Manager computes "Speed".
        let freq = self.cpu.frequency.and_then(|c| q.value(c));
        let perf = self.cpu.performance.and_then(|c| q.value(c));
        if let (Some(freq), Some(perf)) = (freq, perf) {
            readings.push(Reading {
                id: "clock",
                label: "Clock",
                value: freq * perf / 100.0,
                unit: Unit::Megahertz,
                range: None,
                detail: Some(format!("base {:.1} GHz", freq / 1000.0)),
                featured: false,
                dashboard: true,
            });
        }

        let threads = self.threads();

        // A single maxed-out thread hides in the total; show the busiest one.
        if let Some(busiest) = threads.iter().max_by(|a, b| a.value.total_cmp(&b.value)) {
            readings.push(Reading {
                id: "busiest",
                label: "Busiest thread",
                value: busiest.value,
                unit: Unit::Percent,
                range: Some((0.0, 100.0)),
                detail: Some(busiest.label.to_lowercase()),
                featured: false,
                dashboard: true,
            });
        }

        // Component page only: where CPU time goes besides programs, and how deep
        // the cores sleep while idle.
        let value = |c: Option<Counter>| c.and_then(|c| q.value(c));
        let percent = |id, label, c, detail: &str| {
            value(c)
                .map(|v| Reading::new(id, label, v.clamp(0.0, 100.0), Unit::Percent).range(0.0, 100.0).detail(detail))
        };
        readings.extend(percent("interrupt", "Interrupts", self.cpu.interrupt, "hardware interrupt time"));
        readings.extend(percent("dpc", "DPCs", self.cpu.dpc, "deferred driver work"));
        readings.extend(percent("c1", "Light sleep (C1)", self.cpu.c1, "share of time"));
        let deep = value(self.cpu.c2).map(|c2| c2 + value(self.cpu.c3).unwrap_or(0.0));
        readings.extend(deep.map(|v| {
            Reading::new("c2", "Deep sleep (C2+)", v.clamp(0.0, 100.0), Unit::Percent)
                .range(0.0, 100.0)
                .detail("share of time")
        }));
        if let Some(v) = value(self.cpu.context_switches) {
            readings.push(Reading::new("ctx", "Context switches", v, Unit::PerSecond));
        }
        if let Some(v) = value(self.cpu.queue) {
            readings.push(Reading::new("queue", "Ready queue", v, Unit::Count).detail("threads waiting for a core"));
        }

        let inv = &self.inventory;
        let mut facts: Vec<Info> = inv.cpu_info.clone();
        facts.push(info("Processes", snapshot.processes.len().to_string()));
        facts.push(info("Threads", snapshot.threads.to_string()));
        facts.push(info("Handles", snapshot.handles.to_string()));
        facts.push(info("Up time", uptime()));

        let by_cpu = snapshot.processes.iter().filter(|p| p.cpu > 0.0).map(|p| (p.name.clone(), p.cpu));
        Group {
            id: "cpu".into(),
            kind: Kind::Cpu,
            order: order::CPU,
            title: inv.cpu_name.clone().unwrap_or_else(|| "Processor".into()),
            subtitle: inv.cpu_topology.clone().unwrap_or_default(),
            readings,
            breakdown: (!threads.is_empty()).then_some(Breakdown {
                title: "Threads",
                subtitle: "Load and effective clock per logical processor",
                unit: Unit::Percent,
                range: Some((0.0, 100.0)),
                items: threads,
            }),
            info: facts,
            processes: Some(Processes { metric: "CPU", unit: Unit::Percent, items: top_processes(by_cpu, 8) }),
        }
    }

    /// Per logical processor, ordered by index. Instance names look like "0,7"
    /// (group, index); totals are named "_Total" / "0,_Total".
    fn threads(&self) -> Vec<BreakdownItem> {
        let q = &self.query;
        let index = |name: &str| name.rsplit(',').next().and_then(|i| i.parse::<u32>().ok());
        let freq = by_instance(q, self.cpu.thread_frequency);
        let perf = by_instance(q, self.cpu.thread_performance);

        let mut threads: Vec<(u32, BreakdownItem)> = self
            .cpu
            .thread_utility
            .map(|c| q.array(c))
            .unwrap_or_default()
            .into_iter()
            .filter_map(|(name, load)| {
                let i = index(&name)?;
                let clock = freq.get(&name).zip(perf.get(&name)).map(|(f, p)| f * p / 100.0);
                Some((
                    i,
                    BreakdownItem {
                        label: format!("Thread {i}"),
                        value: load.clamp(0.0, 100.0),
                        detail: clock.map(|mhz| format!("{:.2} GHz", mhz / 1000.0)),
                    },
                ))
            })
            .collect();
        threads.sort_by_key(|(i, _)| *i);
        threads.into_iter().map(|(_, t)| t).collect()
    }

    fn memory_group(&self, snapshot: &Snapshot) -> Option<Group> {
        let mut m = MEMORYSTATUSEX { dwLength: size_of::<MEMORYSTATUSEX>() as u32, ..Default::default() };
        unsafe { GlobalMemoryStatusEx(&mut m) }.ok()?;

        let mb = |bytes: u64| bytes as f64 / MIB;
        let total = mb(m.ullTotalPhys);
        let commit_limit = mb(m.ullTotalPageFile);

        let readings = vec![
            Reading {
                id: "used",
                label: "In use",
                value: total - mb(m.ullAvailPhys),
                unit: Unit::Mebibytes,
                range: Some((0.0, total)),
                detail: Some(format!("of {:.1} GB", total / 1024.0)),
                featured: false,
                dashboard: true,
            },
            Reading {
                id: "available",
                label: "Available",
                value: mb(m.ullAvailPhys),
                unit: Unit::Mebibytes,
                range: None,
                detail: None,
                featured: false,
                dashboard: true,
            },
            Reading {
                id: "committed",
                label: "Committed",
                value: commit_limit - mb(m.ullAvailPageFile),
                unit: Unit::Mebibytes,
                range: Some((0.0, commit_limit)),
                detail: Some(format!("of {:.1} GB", commit_limit / 1024.0)),
                featured: false,
                dashboard: true,
            },
        ];

        // Component page only: how the rest of RAM is used, like Task Manager's
        // memory composition.
        let q = &self.query;
        let bytes = |c: Option<Counter>| c.and_then(|c| q.value(c)).map(|v| v / MIB);
        let mut readings = readings;
        let standby = [self.memory.standby_core, self.memory.standby_normal, self.memory.standby_reserve]
            .into_iter()
            .filter_map(bytes)
            .sum::<f64>();
        let modified = bytes(self.memory.modified).unwrap_or(0.0);
        readings.push(
            Reading::new("cached", "Cached", standby + modified, Unit::Mebibytes)
                .range(0.0, total)
                .detail("file cache, freed on demand"),
        );
        if let Some(compressed) = snapshot.compressed_bytes {
            readings.push(
                Reading::new("compressed", "Compressed", compressed as f64 / MIB, Unit::Mebibytes)
                    .detail("in-memory compression store"),
            );
        }
        if let Some(v) = bytes(self.memory.paged_pool) {
            readings.push(Reading::new("paged", "Paged pool", v, Unit::Mebibytes).detail("kernel, may go to disk"));
        }
        if let Some(v) = bytes(self.memory.nonpaged_pool) {
            readings
                .push(Reading::new("nonpaged", "Non-paged pool", v, Unit::Mebibytes).detail("kernel, always in RAM"));
        }
        if let Some(v) = self.memory.page_file.and_then(|c| q.value(c)) {
            readings.push(
                Reading::new("pagefile", "Page file", v.clamp(0.0, 100.0), Unit::Percent)
                    .range(0.0, 100.0)
                    .detail("in use"),
            );
        }
        if let Some(v) = self.memory.hard_faults.and_then(|c| q.value(c)) {
            readings.push(Reading::new("hardfaults", "Hard faults", v, Unit::PerSecond).detail("reads from disk"));
        }

        let inv = &self.inventory;
        let mut facts = inv.memory_info.clone();
        if let Some(installed_kb) = inv.installed_kb {
            let reserved = installed_kb as f64 / 1024.0 - total;
            if reserved > 0.0 {
                facts.push(info("Hardware reserved", format!("{reserved:.0} MB")));
            }
        }
        // The compression store is already the "Compressed" reading, not a program.
        let by_memory = snapshot
            .processes
            .iter()
            .filter(|p| p.name != "Memory Compression")
            .map(|p| (p.name.clone(), p.private_bytes as f64 / MIB));
        Some(Group {
            id: "memory".into(),
            kind: Kind::Memory,
            order: order::MEMORY,
            breakdown: None,
            info: facts,
            processes: Some(Processes { metric: "Memory", unit: Unit::Mebibytes, items: top_processes(by_memory, 8) }),
            title: inv.memory_name.clone().unwrap_or_else(|| format!("{:.0} GB Memory", total / 1024.0)),
            subtitle: inv.memory_modules.clone().unwrap_or_default(),
            readings,
        })
    }

    fn disk_groups(&mut self, snapshot: &Snapshot) -> Vec<Group> {
        let q = &self.query;
        let read = by_instance(q, self.disk.read);
        let write = by_instance(q, self.disk.write);
        let idle = by_instance(q, self.disk.idle);
        let read_latency = by_instance(q, self.disk.read_latency);
        let write_latency = by_instance(q, self.disk.write_latency);
        let queue = by_instance(q, self.disk.queue);
        let system_drive = std::env::var("SystemDrive").unwrap_or_else(|_| "C:".into());
        // Per-process I/O is not split by drive; every disk page shows the same list.
        let by_io: Vec<(String, f64)> = snapshot
            .processes
            .iter()
            .filter(|p| p.io_bytes_per_sec > 0.0)
            .map(|p| (p.name.clone(), p.io_bytes_per_sec))
            .collect();

        let mut instances: Vec<&String> = read.keys().filter(|n| *n != "_Total").collect();
        instances.sort();

        // A drive was plugged in or removed (PDH wildcards pick that up on their own):
        // re-read names, since disk numbers can now point to different drives.
        if instances.iter().map(|s| s.as_str()).ne(self.known_disks.iter().map(|s| s.as_str())) {
            self.known_disks = instances.iter().map(|s| s.to_string()).collect();
            if let Ok(wmi) = wmi::WMIConnection::new() {
                self.inventory.disks = load_disks(&wmi);
            }
        }

        // Internal drives first, then external ones; each by first drive letter
        // (C:, D:, …), drives without a letter last.
        let mut sorted = instances.clone();
        sorted.sort_by_key(|name| {
            let (index, letters) = name.split_once(' ').unwrap_or((name, ""));
            let index = index.parse::<u32>().unwrap_or(u32::MAX);
            let external = self.inventory.disks.get(&index).is_some_and(|d| d.external);
            (external, letters.chars().next().unwrap_or('~'), index)
        });

        sorted
            .into_iter()
            .enumerate()
            .map(|(rank, instance)| {
                // Instance names look like "0 C:" or "1 D: E:".
                let (index, letters) = instance.split_once(' ').unwrap_or((instance, ""));
                let index: u32 = index.parse().unwrap_or(u32::MAX);
                let drive = self.inventory.disks.get(&index);

                let mut readings = vec![
                    throughput("read", "Read", read[instance], Unit::BytesPerSec, None),
                    throughput("write", "Write", write.get(instance).copied().unwrap_or(0.0), Unit::BytesPerSec, None),
                ];
                if let Some(idle) = idle.get(instance) {
                    readings.push(Reading {
                        id: "active",
                        label: "Active time",
                        value: (100.0 - idle).clamp(0.0, 100.0),
                        unit: Unit::Percent,
                        range: Some((0.0, 100.0)),
                        detail: None,
                        featured: false,
                        dashboard: true,
                    });
                }

                // Component page only: response times, waiting requests, filled space.
                let ms = |m: &HashMap<String, f64>| m.get(instance).map(|s| s * 1000.0);
                if let Some(v) = ms(&read_latency) {
                    readings.push(
                        Reading::new("read_ms", "Read latency", v, Unit::Milliseconds).detail("average per read"),
                    );
                }
                if let Some(v) = ms(&write_latency) {
                    readings.push(
                        Reading::new("write_ms", "Write latency", v, Unit::Milliseconds).detail("average per write"),
                    );
                }
                if let Some(v) = queue.get(instance) {
                    readings.push(Reading::new("queue", "Queue", *v, Unit::Count).detail("requests waiting"));
                }
                let volumes: Vec<(&str, u64, u64)> = letters
                    .split_whitespace()
                    .filter_map(|l| volume_space(l).map(|(free, total)| (l, free, total)))
                    .collect();
                let (free, total): (u64, u64) = volumes.iter().fold((0, 0), |(f, t), v| (f + v.1, t + v.2));
                if total > 0 {
                    readings.push(
                        Reading::new("space", "Space used", (total - free) as f64 / MIB, Unit::Mebibytes)
                            .range(0.0, total as f64 / MIB)
                            .detail(format!("{} free", format_binary(free))),
                    );
                }

                let mut subtitle: Vec<String> = drive.and_then(DiskInfo::type_label).into_iter().collect();
                subtitle.push(format!("Disk {index}"));
                if !letters.is_empty() {
                    subtitle.push(letters.to_string());
                }
                if let Some(size) = drive.and_then(|d| d.size) {
                    subtitle.push(format_size(size));
                }

                let mut facts = Vec::new();
                if let Some(d) = drive {
                    facts.extend(d.type_label().map(|t| info("Type", t)));
                    facts.extend(d.size.map(|s| info("Capacity", format_size(s))));
                    facts.extend(d.firmware.clone().map(|f| info("Firmware", f)));
                    facts.extend(d.partitions.map(|p| info("Partitions", p.to_string())));
                }
                for (letter, free, total) in &volumes {
                    // Binary units like Explorer, so used + free adds up with "Space used".
                    facts.push(info(
                        format!("Volume {letter}"),
                        format!("{} free of {}", format_binary(*free), format_binary(*total)),
                    ));
                }
                facts.push(info(
                    "System disk",
                    if letters.split_whitespace().any(|l| l.eq_ignore_ascii_case(&system_drive)) {
                        "Yes"
                    } else {
                        "No"
                    },
                ));

                // Identify the physical drive by serial number, so settings, history and
                // an open detail page follow it across disk numbers and USB ports.
                // Without a serial, number + model at least keeps different drives apart.
                let id = match drive {
                    Some(DiskInfo { serial: Some(serial), .. }) => format!("disk/{serial}"),
                    Some(d) => format!("disk/{index}/{}", d.model),
                    None => format!("disk/{index}"),
                };
                let external = drive.is_some_and(|d| d.external);
                Group {
                    id,
                    kind: if external { Kind::ExternalDisk } else { Kind::Disk },
                    order: order::DISK + rank.min(99) as u16,
                    breakdown: None,
                    info: facts,
                    processes: Some(Processes {
                        // Windows counts I/O per process, not per drive or device.
                        metric: "I/O across all drives, network and devices",
                        unit: Unit::BytesPerSec,
                        items: top_processes(by_io.iter().cloned(), 8),
                    }),
                    title: drive.map(|d| d.model.clone()).unwrap_or_else(|| format!("Disk {index}")),
                    subtitle: subtitle.join(" · "),
                    readings,
                }
            })
            .collect()
    }

    fn network_groups(&self) -> Vec<Group> {
        let q = &self.query;
        let wifi = self.wifi.as_ref().map(Wifi::links).unwrap_or_default();
        let received = by_instance(q, self.net.received);
        let sent = by_instance(q, self.net.sent);
        let bandwidth = by_instance(q, self.net.bandwidth);

        let mut adapters: Vec<(&String, f64)> = bandwidth
            .iter()
            .filter(|(name, bw)| **bw > 0.0 && !is_virtual_adapter(name))
            .map(|(name, bw)| (name, *bw))
            .collect();
        adapters.sort_by(|a, b| a.0.cmp(b.0));

        adapters
            .into_iter()
            .enumerate()
            .map(|(i, (name, bits))| {
                let range = Some((0.0, bits));
                let down = received.get(name).copied().unwrap_or(0.0) * 8.0;
                let up = sent.get(name).copied().unwrap_or(0.0) * 8.0;
                let mut readings = vec![
                    throughput("down", "Download", down, Unit::BitsPerSec, range),
                    throughput("up", "Upload", up, Unit::BitsPerSec, range),
                ];

                let adapter = self.adapters.iter().find(|a| a.matches_pdh(name));
                let wifi = adapter.and_then(|a| wifi.iter().find(|w| w.description == a.description));
                let mut facts = Vec::new();
                if let Some(w) = wifi {
                    // Signal strength matters as much as throughput on Wi-Fi.
                    readings.push(
                        Reading::new("signal", "Signal", w.signal as f64, Unit::Percent)
                            .range(0.0, 100.0)
                            .detail(&w.ssid)
                            .on_dashboard(),
                    );
                    facts.push(info("Network", w.ssid.clone()));
                    facts.extend(w.standard.map(|s| info("Standard", s)));
                    facts.push(info("Link rate", format!("{:.0} Mbps down · {:.0} Mbps up", w.rx_mbps, w.tx_mbps)));
                }
                facts.push(info("Link speed", format_bits(bits)));
                if let Some(a) = adapter {
                    facts.push(info("Adapter", a.name.clone()));
                    facts.extend(a.ipv4.first().map(|ip| info("IPv4", ip.clone())));
                    facts.extend(a.ipv6.first().map(|ip| info("IPv6", ip.clone())));
                    facts.extend(a.gateway.clone().map(|g| info("Gateway", g)));
                    if !a.dns.is_empty() {
                        facts.push(info("DNS", a.dns.join(", ")));
                    }
                    facts.extend(a.mac.clone().map(|m| info("MAC address", m)));
                }

                Group {
                    id: format!("net/{name}"),
                    kind: Kind::Network,
                    order: order::NETWORK + i as u16,
                    breakdown: None,
                    info: facts,
                    processes: None,
                    // PDH escapes "(" and ")" in instance names as "[" and "]".
                    title: name.replace("[R]", "").replace('[', "(").replace(']', ")").replace("  ", " "),
                    subtitle: match wifi {
                        Some(w) => format!("Wi-Fi · {}", w.ssid),
                        None => format!("Network · {} link", format_bits(bits)),
                    },
                    readings,
                }
            })
            .collect()
    }
}

/// Free and total bytes of a volume such as `"C:"`.
fn volume_space(letter: &str) -> Option<(u64, u64)> {
    let (mut free, mut total) = (0u64, 0u64);
    unsafe { GetDiskFreeSpaceExW(&HSTRING::from(format!("{letter}\\")), None, Some(&mut total), Some(&mut free)) }
        .ok()?;
    Some((free, total))
}

/// E.g. "3 d 4 h 12 min".
fn uptime() -> String {
    let minutes = unsafe { GetTickCount64() } / 60_000;
    let (days, hours, mins) = (minutes / 1440, minutes / 60 % 24, minutes % 60);
    if days > 0 { format!("{days} d {hours} h {mins} min") } else { format!("{hours} h {mins} min") }
}

impl SensorProvider for SystemProvider {
    fn poll(&mut self) -> Vec<Group> {
        self.query.collect();
        let snapshot = self.sampler.sample();
        if self.polls.is_multiple_of(10) {
            self.adapters = super::netinfo::adapters();
        }
        self.polls = self.polls.wrapping_add(1);

        let mut groups = vec![self.cpu_group(&snapshot)];
        groups.extend(self.memory_group(&snapshot));
        groups.extend(self.battery.as_ref().and_then(|b| b.group()));
        groups.extend(self.disk_groups(&snapshot));
        groups.extend(self.network_groups());
        self.processes = snapshot.processes;
        groups
    }

    fn machine(&self) -> Option<String> {
        self.inventory.board.clone()
    }

    fn process_table(&mut self) -> Option<Vec<Row>> {
        let gpu = self.gpu.get_or_insert_with(GpuQuery::new).sample();
        Some(self.details.rows(&self.processes, &gpu))
    }
}

const MIB: f64 = 1024.0 * 1024.0;

fn by_instance(q: &Query, counter: Option<Counter>) -> HashMap<String, f64> {
    counter.map(|c| q.array(c)).unwrap_or_default().into_iter().collect()
}

fn throughput(id: &'static str, label: &'static str, value: f64, unit: Unit, range: Option<(f64, f64)>) -> Reading {
    Reading { id, label, value: value.max(0.0), unit, range, detail: None, featured: false, dashboard: true }
}

fn is_virtual_adapter(name: &str) -> bool {
    const SKIP: [&str; 8] =
        ["vEthernet", "Hyper-V", "Virtual", "Loopback", "isatap", "Teredo", "WAN Miniport", "Bluetooth"];
    SKIP.iter().any(|s| name.to_lowercase().contains(&s.to_lowercase()))
}

/// Sizes as Windows Explorer shows them (1 GB = 1024³ bytes).
fn format_binary(bytes: u64) -> String {
    let gb = bytes as f64 / (1u64 << 30) as f64;
    if gb >= 1024.0 { format!("{:.2} TB", gb / 1024.0) } else { format!("{gb:.0} GB") }
}

fn format_size(bytes: u64) -> String {
    let gb = bytes as f64 / 1e9;
    if gb >= 1000.0 { format!("{:.1} TB", gb / 1000.0) } else { format!("{gb:.0} GB") }
}

fn format_bits(bits: f64) -> String {
    if bits >= 1e9 { format!("{:.1} Gbps", bits / 1e9).replace(".0 ", " ") } else { format!("{:.0} Mbps", bits / 1e6) }
}

// ---------------------------------------------------------------------------
// Component names (WMI, read once)

#[derive(Default)]
struct Inventory {
    cpu_name: Option<String>,
    cpu_topology: Option<String>,
    logical_processors: u32,
    /// Base speed, sockets, caches, virtualization.
    cpu_info: Vec<Info>,
    memory_name: Option<String>,
    memory_modules: Option<String>,
    /// Speed, slots, form factor, one line per module.
    memory_info: Vec<Info>,
    /// RAM installed, including what firmware and devices reserve.
    installed_kb: Option<u64>,
    board: Option<String>,
    disks: HashMap<u32, DiskInfo>,
}

struct DiskInfo {
    model: String,
    size: Option<u64>,
    serial: Option<String>,
    external: bool,
    bus: Option<&'static str>,
    media: Option<&'static str>,
    firmware: Option<String>,
    partitions: Option<u32>,
}

impl DiskInfo {
    /// E.g. "NVMe SSD", "SATA HDD", "External · USB".
    fn type_label(&self) -> Option<String> {
        let parts: Vec<&str> =
            [self.external.then_some("External"), self.bus, self.media].into_iter().flatten().collect();
        if parts.is_empty() {
            return None;
        }
        // "External" reads as its own item; bus and media read as one ("NVMe SSD").
        Some(match parts.split_first() {
            Some((&"External", rest)) if !rest.is_empty() => format!("External · {}", rest.join(" ")),
            _ => parts.join(" "),
        })
    }
}

#[derive(Deserialize)]
#[serde(rename = "Win32_Processor", rename_all = "PascalCase")]
struct Win32Processor {
    name: String,
    number_of_cores: u32,
    number_of_logical_processors: u32,
    max_clock_speed: Option<u32>,
    virtualization_firmware_enabled: Option<bool>,
    socket_designation: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename = "Win32_PhysicalMemoryArray", rename_all = "PascalCase")]
struct Win32PhysicalMemoryArray {
    memory_devices: Option<u32>,
}

#[derive(Deserialize)]
#[serde(rename = "Win32_BaseBoard", rename_all = "PascalCase")]
struct Win32BaseBoard {
    manufacturer: Option<String>,
    product: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename = "Win32_PhysicalMemory", rename_all = "PascalCase")]
struct Win32PhysicalMemory {
    manufacturer: Option<String>,
    part_number: Option<String>,
    capacity: Option<u64>,
    configured_clock_speed: Option<u32>,
    #[serde(rename = "SMBIOSMemoryType")]
    smbios_memory_type: Option<u32>,
    speed: Option<u32>,
    form_factor: Option<u16>,
    bank_label: Option<String>,
    device_locator: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename = "Win32_DiskDrive", rename_all = "PascalCase")]
struct Win32DiskDrive {
    index: u32,
    model: Option<String>,
    size: Option<u64>,
    serial_number: Option<String>,
    interface_type: Option<String>,
    media_type: Option<String>,
    #[serde(rename = "PNPDeviceID")]
    pnp_device_id: Option<String>,
    firmware_revision: Option<String>,
    partitions: Option<u32>,
}

impl Inventory {
    /// Best effort: any query that fails just leaves that name empty.
    fn load() -> Self {
        let mut inv = Inventory::default();
        let Ok(wmi) = wmi::WMIConnection::new() else { return inv };

        if let Ok(cpus) = wmi.query::<Win32Processor>()
            && let Some(cpu) = cpus.first()
        {
            inv.cpu_name = Some(cpu.name.trim().to_string());
            inv.cpu_topology =
                Some(format!("{} cores · {} threads", cpu.number_of_cores, cpu.number_of_logical_processors));
            inv.logical_processors = cpus.iter().map(|c| c.number_of_logical_processors).sum();

            let facts = &mut inv.cpu_info;
            if let Some(mhz) = cpu.max_clock_speed {
                facts.push(info("Base speed", format!("{:.2} GHz", mhz as f64 / 1000.0)));
            }
            facts.push(info("Sockets", cpus.len().to_string()));
            facts.extend(cpu.socket_designation.as_deref().map(|s| info("Socket", s.trim())));
            facts.push(info("Cores", cpus.iter().map(|c| c.number_of_cores).sum::<u32>().to_string()));
            facts.push(info("Logical processors", inv.logical_processors.to_string()));
            for (level, bytes) in cache_sizes() {
                facts.push(info(format!("L{level} cache"), format_cache(bytes)));
            }
            if let Some(on) = cpu.virtualization_firmware_enabled {
                facts.push(info("Virtualization", if on { "Enabled" } else { "Disabled" }));
            }
        }

        if let Some(board) = wmi.query::<Win32BaseBoard>().ok().and_then(|v| v.into_iter().next()) {
            let name = [board.manufacturer, board.product]
                .into_iter()
                .flatten()
                .map(|s| s.trim().replace("ASUSTeK COMPUTER INC.", "ASUS"))
                .collect::<Vec<_>>()
                .join(" ");
            inv.board = (!name.is_empty()).then_some(name);
        }

        if let Ok(modules) = wmi.query::<Win32PhysicalMemory>()
            && let Some(first) = modules.first()
        {
            let total_gb: u64 = modules.iter().filter_map(|m| m.capacity).sum::<u64>() >> 30;
            let kind = match first.smbios_memory_type {
                Some(34) => "DDR5",
                Some(26) => "DDR4",
                Some(24) => "DDR3",
                _ => "RAM",
            };
            inv.memory_name = Some(match first.configured_clock_speed {
                Some(mts) if mts > 0 => format!("{total_gb} GB {kind}-{mts}"),
                _ => format!("{total_gb} GB {kind}"),
            });

            let vendor = first.manufacturer.as_deref().unwrap_or("").trim();
            let part = first.part_number.as_deref().unwrap_or("").trim();
            let per_module = first.capacity.map(|c| format!("{} × {} GB", modules.len(), c >> 30));
            let parts: Vec<&str> =
                [vendor, part, per_module.as_deref().unwrap_or("")].into_iter().filter(|s| !s.is_empty()).collect();
            inv.memory_modules = Some(parts.join(" · "));

            let facts = &mut inv.memory_info;
            if let Some(mts) = first.configured_clock_speed.filter(|s| *s > 0) {
                facts.push(info(
                    "Speed",
                    match first.speed {
                        // E.g. XMP/EXPO profiles run above the module's JEDEC speed.
                        Some(rated) if rated > 0 && rated != mts => format!("{mts} MT/s (JEDEC {rated} MT/s)"),
                        _ => format!("{mts} MT/s"),
                    },
                ));
            }
            let slots: Option<u32> =
                wmi.query::<Win32PhysicalMemoryArray>().ok().and_then(|a| a.iter().map(|a| a.memory_devices).sum());
            facts.push(info(
                "Slots used",
                match slots {
                    Some(slots) if slots > 0 => format!("{} of {slots}", modules.len()),
                    _ => modules.len().to_string(),
                },
            ));
            let form = match first.form_factor {
                Some(8) => Some("DIMM"),
                Some(12) => Some("SO-DIMM"),
                _ => None,
            };
            facts.extend(form.map(|f| info("Form factor", f)));
            for m in &modules {
                let slot = module_slot(m.bank_label.as_deref(), m.device_locator.as_deref());
                let size = m.capacity.map(|c| format!("{} GB", c >> 30));
                let part = m.part_number.as_deref().map(str::trim).filter(|p| !p.is_empty());
                let value: Vec<&str> = [size.as_deref(), part].into_iter().flatten().collect();
                facts.push(info(slot, value.join(" · ")));
            }
        }

        let mut installed_kb = 0u64;
        inv.installed_kb =
            unsafe { GetPhysicallyInstalledSystemMemory(&mut installed_kb) }.is_ok().then_some(installed_kb);
        inv.disks = load_disks(&wmi);
        inv
    }
}

/// Readable slot name: "Channel A · DIMM 1" from bank "P0 CHANNEL A" and locator
/// "DIMM 1", or whichever of the two is present.
fn module_slot(bank: Option<&str>, locator: Option<&str>) -> String {
    let channel = bank.and_then(|b| {
        let upper = b.to_uppercase();
        let i = upper.find("CHANNEL")?;
        Some(format!("Channel {}", b[i + 7..].trim()))
    });
    [channel.as_deref(), locator.map(str::trim)]
        .into_iter()
        .flatten()
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" · ")
}

/// Total cache size per level (1–3), summed over all cores like Task Manager.
fn cache_sizes() -> Vec<(u8, u64)> {
    let mut size = 0u32;
    unsafe { GetLogicalProcessorInformationEx(RelationCache, None, &mut size) }.ok();
    if size == 0 {
        return Vec::new();
    }
    let mut buffer = vec![0u64; (size as usize).div_ceil(8)];
    let ptr = buffer.as_mut_ptr() as *mut SYSTEM_LOGICAL_PROCESSOR_INFORMATION_EX;
    if unsafe { GetLogicalProcessorInformationEx(RelationCache, Some(ptr), &mut size) }.is_err() {
        return Vec::new();
    }
    let mut totals = [0u64; 4];
    let mut offset = 0usize;
    while offset < size as usize {
        // SAFETY: variable-sized records packed back to back; `Size` gives the stride.
        let record =
            unsafe { &*((buffer.as_ptr() as *const u8).add(offset) as *const SYSTEM_LOGICAL_PROCESSOR_INFORMATION_EX) };
        if record.Relationship == RelationCache {
            let cache: &CACHE_RELATIONSHIP = unsafe { &record.Anonymous.Cache };
            if (1..=3).contains(&cache.Level) {
                totals[cache.Level as usize] += cache.CacheSize as u64;
            }
        }
        if record.Size == 0 {
            break;
        }
        offset += record.Size as usize;
    }
    (1..=3u8).filter(|l| totals[*l as usize] > 0).map(|l| (l, totals[l as usize])).collect()
}

fn format_cache(bytes: u64) -> String {
    if bytes >= 1 << 20 { format!("{} MB", bytes >> 20) } else { format!("{} KB", bytes >> 10) }
}

fn load_disks(wmi: &wmi::WMIConnection) -> HashMap<u32, DiskInfo> {
    let Ok(disks) = wmi.query::<Win32DiskDrive>() else { return HashMap::new() };
    let mut types = load_disk_types();
    disks
        .into_iter()
        .map(|d| {
            let external = is_external(&d);
            let mut model = d.model.unwrap_or_else(|| format!("Disk {}", d.index)).trim().to_string();
            // Seagate reports bare part numbers like "ST4000VN006-3CW104".
            if model.starts_with("ST") && model[2..].starts_with(|c: char| c.is_ascii_digit()) {
                model = format!("Seagate {model}");
            }
            // Serials come padded ("    WW60QK2F") or with a trailing dot (NVMe EUI).
            let serial = d.serial_number.map(|s| s.trim().trim_end_matches('.').to_string()).filter(|s| !s.is_empty());
            let (bus, media) = types.remove(&d.index).unwrap_or_default();
            let firmware = d.firmware_revision.map(|f| f.trim().to_string()).filter(|f| !f.is_empty());
            (
                d.index,
                DiskInfo { model, size: d.size, serial, external, bus, media, firmware, partitions: d.partitions },
            )
        })
        .collect()
}

/// Bus ("NVMe", "SATA", …) and media ("SSD", "HDD") per disk number, from the
/// Storage Management API (what `Get-PhysicalDisk` shows). No admin needed.
fn load_disk_types() -> HashMap<u32, (Option<&'static str>, Option<&'static str>)> {
    let Ok(wmi) = wmi::WMIConnection::with_namespace_path(r"ROOT\Microsoft\Windows\Storage") else {
        return HashMap::new();
    };
    let Ok(disks) = wmi.query::<MsftPhysicalDisk>() else { return HashMap::new() };

    disks
        .into_iter()
        .filter_map(|d| {
            let index = d.device_id.trim().parse().ok()?;
            let bus = match d.bus_type? {
                7 => "USB",
                8 => "RAID",
                10 => "SAS",
                11 => "SATA",
                12 => "SD",
                13 => "MMC",
                17 => "NVMe",
                18 => "SCM",
                _ => return Some((index, (None, media_label(d.media_type)))),
            };
            Some((index, (Some(bus), media_label(d.media_type))))
        })
        .collect()
}

fn media_label(media_type: Option<u16>) -> Option<&'static str> {
    match media_type? {
        3 => Some("HDD"),
        4 => Some("SSD"),
        5 => Some("SCM"),
        _ => None, // 0 = unspecified, typical for USB sticks
    }
}

#[derive(Deserialize)]
#[serde(rename = "MSFT_PhysicalDisk", rename_all = "PascalCase")]
struct MsftPhysicalDisk {
    device_id: String,
    media_type: Option<u16>,
    bus_type: Option<u16>,
}

/// USB sticks, USB/UASP enclosures and other hot-pluggable media.
fn is_external(d: &Win32DiskDrive) -> bool {
    let interface = d.interface_type.as_deref().unwrap_or("");
    let media = d.media_type.as_deref().unwrap_or("").to_lowercase();
    let pnp = d.pnp_device_id.as_deref().unwrap_or("");
    interface.eq_ignore_ascii_case("USB")
        || interface.eq_ignore_ascii_case("1394")
        || media.contains("removable")
        || media.contains("external")
        || pnp.starts_with("USBSTOR\\")
}

/// GPU load and memory per process. Instances are named like
/// `pid_1234_luid_0x…_phys_0_eng_3_engtype_3D`; a process's load is its busiest engine,
/// as in Task Manager.
struct GpuQuery {
    query: Option<Query>,
    engines: Option<Counter>,
    memory: Option<Counter>,
    collected: Option<std::time::Instant>,
    last: HashMap<u32, GpuUse>,
}

impl GpuQuery {
    fn new() -> Self {
        let query = Query::new();
        let engines = query.as_ref().and_then(|q| q.add(r"\GPU Engine(*)\Utilization Percentage"));
        let memory = query.as_ref().and_then(|q| q.add(r"\GPU Process Memory(*)\Dedicated Usage"));
        if let Some(q) = &query {
            q.collect();
        }
        Self { query, engines, memory, collected: Some(std::time::Instant::now()), last: HashMap::new() }
    }

    /// Fresh values when the last sample is old enough for a meaningful rate, else the
    /// previous ones (the table is also rebuilt right after a tick).
    fn sample(&mut self) -> HashMap<u32, GpuUse> {
        let Some(q) = &self.query else { return HashMap::new() };
        if self.collected.is_some_and(|t| t.elapsed().as_millis() < 400) {
            return self.last.clone();
        }
        q.collect();
        self.collected = Some(std::time::Instant::now());
        let pid = |name: &str| name.strip_prefix("pid_")?.split('_').next()?.parse::<u32>().ok();
        let mut usage: HashMap<u32, GpuUse> = HashMap::new();
        for (name, value) in self.engines.map(|c| q.array(c)).unwrap_or_default() {
            if let Some(pid) = pid(&name) {
                let entry = usage.entry(pid).or_default();
                entry.load = entry.load.max(value.clamp(0.0, 100.0));
            }
        }
        for (name, value) in self.memory.map(|c| q.array(c)).unwrap_or_default() {
            if let Some(pid) = pid(&name) {
                usage.entry(pid).or_default().memory += value.max(0.0) as u64;
            }
        }
        self.last = usage.clone();
        usage
    }
}
