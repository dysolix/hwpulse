//! CPU, memory, disks and network from Windows itself — no admin, no driver.
//! Live values come from PDH counters (what Task Manager shows); component names are
//! read once at startup via WMI.

use std::collections::HashMap;

use serde::Deserialize;
use windows::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};

use super::pdh::{Counter, Query};
use super::{Breakdown, BreakdownItem, Group, Kind, Reading, SensorProvider, Unit, order};

pub struct SystemProvider {
    query: Query,
    cpu: Cpu,
    disk: Disk,
    net: Net,
    inventory: Inventory,
    /// Disk instances seen on the last poll, to notice plug/unplug.
    known_disks: Vec<String>,
}

struct Cpu {
    utility: Option<Counter>,
    frequency: Option<Counter>,
    performance: Option<Counter>,
    thread_utility: Option<Counter>,
    thread_frequency: Option<Counter>,
    thread_performance: Option<Counter>,
}

struct Disk {
    read: Option<Counter>,
    write: Option<Counter>,
    idle: Option<Counter>,
}

struct Net {
    received: Option<Counter>,
    sent: Option<Counter>,
    bandwidth: Option<Counter>,
}

impl SystemProvider {
    pub fn new() -> Result<Self, String> {
        let query = Query::new().ok_or("could not open PDH query")?;
        let cpu = Cpu {
            utility: query.add(r"\Processor Information(_Total)\% Processor Utility"),
            frequency: query.add(r"\Processor Information(_Total)\Processor Frequency"),
            performance: query.add(r"\Processor Information(_Total)\% Processor Performance"),
            thread_utility: query.add(r"\Processor Information(*)\% Processor Utility"),
            thread_frequency: query.add(r"\Processor Information(*)\Processor Frequency"),
            thread_performance: query.add(r"\Processor Information(*)\% Processor Performance"),
        };
        let disk = Disk {
            read: query.add(r"\PhysicalDisk(*)\Disk Read Bytes/sec"),
            write: query.add(r"\PhysicalDisk(*)\Disk Write Bytes/sec"),
            idle: query.add(r"\PhysicalDisk(*)\% Idle Time"),
        };
        let net = Net {
            received: query.add(r"\Network Interface(*)\Bytes Received/sec"),
            sent: query.add(r"\Network Interface(*)\Bytes Sent/sec"),
            bandwidth: query.add(r"\Network Interface(*)\Current Bandwidth"),
        };
        // Prime rate counters so the first real poll has a baseline.
        query.collect();

        Ok(Self { query, cpu, disk, net, inventory: Inventory::load(), known_disks: Vec::new() })
    }

    fn cpu_group(&self) -> Group {
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
            });
        }

        let inv = &self.inventory;
        Group {
            id: "cpu".into(),
            kind: Kind::Cpu,
            order: order::CPU,
            title: inv.cpu_name.clone().unwrap_or_else(|| "Processor".into()),
            subtitle: inv.cpu_topology.clone().unwrap_or_default(),
            readings,
            breakdown: (!threads.is_empty()).then(|| Breakdown {
                title: "Threads",
                subtitle: "Load and effective clock per logical processor",
                unit: Unit::Percent,
                range: Some((0.0, 100.0)),
                items: threads,
            }),
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

    fn memory_group(&self) -> Option<Group> {
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
            },
            Reading {
                id: "available",
                label: "Available",
                value: mb(m.ullAvailPhys),
                unit: Unit::Mebibytes,
                range: None,
                detail: None,
                featured: false,
            },
            Reading {
                id: "committed",
                label: "Committed",
                value: commit_limit - mb(m.ullAvailPageFile),
                unit: Unit::Mebibytes,
                range: Some((0.0, commit_limit)),
                detail: Some(format!("of {:.1} GB", commit_limit / 1024.0)),
                featured: false,
            },
        ];

        let inv = &self.inventory;
        Some(Group {
            id: "memory".into(),
            kind: Kind::Memory,
            order: order::MEMORY,
            breakdown: None,
            title: inv.memory_name.clone().unwrap_or_else(|| format!("{:.0} GB Memory", total / 1024.0)),
            subtitle: inv.memory_modules.clone().unwrap_or_default(),
            readings,
        })
    }

    fn disk_groups(&mut self) -> Vec<Group> {
        let q = &self.query;
        let read = by_instance(q, self.disk.read);
        let write = by_instance(q, self.disk.write);
        let idle = by_instance(q, self.disk.idle);

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
                let info = self.inventory.disks.get(&index);

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
                    });
                }

                let mut subtitle: Vec<String> = info.and_then(DiskInfo::type_label).into_iter().collect();
                subtitle.push(format!("Disk {index}"));
                if !letters.is_empty() {
                    subtitle.push(letters.to_string());
                }
                if let Some(size) = info.and_then(|d| d.size) {
                    subtitle.push(format_size(size));
                }

                // Identify the physical drive by serial number, so settings, history and
                // an open detail page follow it across disk numbers and USB ports.
                // Without a serial, number + model at least keeps different drives apart.
                let id = match info {
                    Some(DiskInfo { serial: Some(serial), .. }) => format!("disk/{serial}"),
                    Some(d) => format!("disk/{index}/{}", d.model),
                    None => format!("disk/{index}"),
                };
                let external = info.is_some_and(|d| d.external);
                Group {
                    id,
                    kind: if external { Kind::ExternalDisk } else { Kind::Disk },
                    order: order::DISK + rank.min(99) as u16,
                    breakdown: None,
                    title: info.map(|d| d.model.clone()).unwrap_or_else(|| format!("Disk {index}")),
                    subtitle: subtitle.join(" · "),
                    readings,
                }
            })
            .collect()
    }

    fn network_groups(&self) -> Vec<Group> {
        let q = &self.query;
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
                Group {
                    id: format!("net/{name}"),
                    kind: Kind::Network,
                    order: order::NETWORK + i as u16,
                    breakdown: None,
                    // PDH escapes "(" and ")" in instance names as "[" and "]".
                    title: name.replace("[R]", "").replace('[', "(").replace(']', ")").replace("  ", " "),
                    subtitle: format!("Network · {} link", format_bits(bits)),
                    readings: vec![
                        throughput("down", "Download", down, Unit::BitsPerSec, range),
                        throughput("up", "Upload", up, Unit::BitsPerSec, range),
                    ],
                }
            })
            .collect()
    }
}

impl SensorProvider for SystemProvider {
    fn poll(&mut self) -> Vec<Group> {
        self.query.collect();
        let mut groups = vec![self.cpu_group()];
        groups.extend(self.memory_group());
        groups.extend(self.disk_groups());
        groups.extend(self.network_groups());
        groups
    }

    fn machine(&self) -> Option<String> {
        self.inventory.board.clone()
    }
}

const MIB: f64 = 1024.0 * 1024.0;

fn by_instance(q: &Query, counter: Option<Counter>) -> HashMap<String, f64> {
    counter.map(|c| q.array(c)).unwrap_or_default().into_iter().collect()
}

fn throughput(id: &'static str, label: &'static str, value: f64, unit: Unit, range: Option<(f64, f64)>) -> Reading {
    Reading { id, label, value: value.max(0.0), unit, range, detail: None, featured: false }
}

fn is_virtual_adapter(name: &str) -> bool {
    const SKIP: [&str; 8] = ["vEthernet", "Hyper-V", "Virtual", "Loopback", "isatap", "Teredo", "WAN Miniport", "Bluetooth"];
    SKIP.iter().any(|s| name.to_lowercase().contains(&s.to_lowercase()))
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
    memory_name: Option<String>,
    memory_modules: Option<String>,
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
}

impl DiskInfo {
    /// E.g. "NVMe SSD", "SATA HDD", "External · USB".
    fn type_label(&self) -> Option<String> {
        let parts: Vec<&str> = [self.external.then_some("External"), self.bus, self.media].into_iter().flatten().collect();
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
}

impl Inventory {
    /// Best effort: any query that fails just leaves that name empty.
    fn load() -> Self {
        let mut inv = Inventory::default();
        let Ok(wmi) = wmi::WMIConnection::new() else { return inv };

        if let Some(cpu) = wmi.query::<Win32Processor>().ok().and_then(|v| v.into_iter().next()) {
            inv.cpu_name = Some(cpu.name.trim().to_string());
            inv.cpu_topology =
                Some(format!("{} cores · {} threads", cpu.number_of_cores, cpu.number_of_logical_processors));
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
        }

        inv.disks = load_disks(&wmi);
        inv
    }
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
            let serial = d
                .serial_number
                .map(|s| s.trim().trim_end_matches('.').to_string())
                .filter(|s| !s.is_empty());
            let (bus, media) = types.remove(&d.index).unwrap_or_default();
            (d.index, DiskInfo { model, size: d.size, serial, external, bus, media })
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
