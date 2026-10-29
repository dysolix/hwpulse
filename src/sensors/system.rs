//! CPU, memory, disks and network from Windows itself — no admin, no driver.
//! Live values come from PDH counters (what Task Manager shows); component names are
//! read once at startup via WMI.

use std::collections::HashMap;

use serde::Deserialize;
use windows::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};

use super::pdh::{Counter, Query};
use super::{Group, Reading, SensorProvider, Unit, order};

pub struct SystemProvider {
    query: Query,
    cpu: Cpu,
    disk: Disk,
    net: Net,
    inventory: Inventory,
}

struct Cpu {
    utility: Option<Counter>,
    utility_per_thread: Option<Counter>,
    frequency: Option<Counter>,
    performance: Option<Counter>,
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
            utility_per_thread: query.add(r"\Processor Information(*)\% Processor Utility"),
            frequency: query.add(r"\Processor Information(_Total)\Processor Frequency"),
            performance: query.add(r"\Processor Information(_Total)\% Processor Performance"),
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

        Ok(Self { query, cpu, disk, net, inventory: Inventory::load() })
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

        // A single maxed-out thread hides in the total; show the busiest one.
        let busiest = self
            .cpu
            .utility_per_thread
            .map(|c| q.array(c))
            .unwrap_or_default()
            .into_iter()
            .filter(|(name, _)| !name.contains("_Total"))
            .max_by(|a, b| a.1.total_cmp(&b.1));
        if let Some((name, value)) = busiest {
            let thread = name.rsplit(',').next().unwrap_or(&name);
            readings.push(Reading {
                id: "busiest",
                label: "Busiest thread",
                value: value.clamp(0.0, 100.0),
                unit: Unit::Percent,
                range: Some((0.0, 100.0)),
                detail: Some(format!("thread {thread}")),
                featured: false,
            });
        }

        let inv = &self.inventory;
        Group {
            id: "cpu".into(),
            order: order::CPU,
            title: inv.cpu_name.clone().unwrap_or_else(|| "Processor".into()),
            subtitle: inv.cpu_topology.clone().unwrap_or_default(),
            readings,
        }
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
            order: order::MEMORY,
            title: inv.memory_name.clone().unwrap_or_else(|| format!("{:.0} GB Memory", total / 1024.0)),
            subtitle: inv.memory_modules.clone().unwrap_or_default(),
            readings,
        })
    }

    fn disk_groups(&self) -> Vec<Group> {
        let q = &self.query;
        let read = by_instance(q, self.disk.read);
        let write = by_instance(q, self.disk.write);
        let idle = by_instance(q, self.disk.idle);

        let mut instances: Vec<&String> = read.keys().filter(|n| *n != "_Total").collect();
        instances.sort();

        instances
            .into_iter()
            .map(|instance| {
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

                let mut subtitle = vec![format!("Disk {index}")];
                if !letters.is_empty() {
                    subtitle.push(letters.to_string());
                }
                if let Some(size) = info.and_then(|d| d.size) {
                    subtitle.push(format_size(size));
                }

                Group {
                    id: format!("disk/{index}"),
                    order: order::DISK + index.min(99) as u16,
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
                    order: order::NETWORK + i as u16,
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

        if let Ok(disks) = wmi.query::<Win32DiskDrive>() {
            for d in disks {
                let mut model = d.model.unwrap_or_else(|| format!("Disk {}", d.index)).trim().to_string();
                // Seagate reports bare part numbers like "ST4000VN006-3CW104".
                if model.starts_with("ST") && model[2..].starts_with(|c: char| c.is_ascii_digit()) {
                    model = format!("Seagate {model}");
                }
                inv.disks.insert(d.index, DiskInfo { model, size: d.size });
            }
        }

        inv
    }
}
