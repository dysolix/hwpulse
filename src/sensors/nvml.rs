//! NVIDIA GPUs via NVML (`nvml.dll`, shipped with the driver). Talks to the driver
//! directly — no `nvidia-smi` process per poll.

use nvml_wrapper::Device;
use nvml_wrapper::Nvml;
use nvml_wrapper::bitmasks::device::ThrottleReasons;
use nvml_wrapper::enum_wrappers::device::{Clock, PcieUtilCounter, PerformanceState, TemperatureSensor};
use nvml_wrapper::error::NvmlError;

use super::{Group, Info, Kind, ProcessItem, Processes, Reading, SensorProvider, Unit, info, top_processes};

pub struct NvmlProvider {
    gpus: Vec<Gpu>,
    /// Monitors per adapter name, refreshed every few seconds (cheap, but rarely changes).
    displays: Vec<super::displays::Display>,
    polls: u32,
}

struct Gpu {
    index: u32,
    nvml: &'static Nvml,
    device: Device<'static>,
    name: String,
    driver: String,
    fan_count: u32,
    power_limit_w: Option<f64>,
    vram_total_mb: f64,
    /// Facts that never change while running (bus width, VBIOS, max clocks …).
    fixed: Vec<Info>,
    /// Newest per-process sample seen, so each poll only gets new ones.
    process_timestamp: u64,
}

impl NvmlProvider {
    /// `Ok(None)` when the machine simply has no NVIDIA GPU or driver; that is not
    /// an error worth showing. Other failures are reported.
    pub fn new() -> Result<Option<Self>, String> {
        let nvml = match Nvml::init() {
            Ok(nvml) => nvml,
            Err(
                NvmlError::LibloadingError(_)
                | NvmlError::LibraryNotFound
                | NvmlError::DriverNotLoaded
                | NvmlError::NotFound,
            ) => return Ok(None),
            Err(e) => return Err(e.to_string()),
        };
        // NVML stays loaded for the whole process lifetime; leaking it gives
        // `Device<'static>` handles without a self-referential struct.
        let nvml: &'static Nvml = Box::leak(Box::new(nvml));
        let driver = nvml.sys_driver_version().unwrap_or_default();

        let count = nvml.device_count().map_err(|e| e.to_string())?;
        let mut gpus = Vec::new();
        for index in 0..count {
            let Ok(device) = nvml.device_by_index(index) else { continue };
            let name = device.name().unwrap_or_else(|_| format!("GPU {index}"));
            let power_limit_w = device.enforced_power_limit().ok().map(|mw| mw as f64 / 1000.0);
            gpus.push(Gpu {
                index,
                nvml,
                fan_count: device.num_fans().unwrap_or(0),
                vram_total_mb: device.memory_info().map(|m| m.total as f64 / MIB).unwrap_or(0.0),
                fixed: fixed_info(&device, power_limit_w),
                power_limit_w,
                name,
                driver: driver.clone(),
                device,
                process_timestamp: 0,
            });
        }
        Ok((!gpus.is_empty()).then_some(Self { gpus, displays: Vec::new(), polls: 0 }))
    }
}

const MIB: f64 = 1024.0 * 1024.0;

impl SensorProvider for NvmlProvider {
    fn poll(&mut self) -> Vec<Group> {
        if self.polls.is_multiple_of(5) {
            self.displays = super::displays::displays();
        }
        self.polls = self.polls.wrapping_add(1);
        let displays = &self.displays;
        self.gpus.iter_mut().map(|gpu| gpu.poll(displays)).collect()
    }
}

/// Facts read once: architecture, cores, bus, VBIOS, clock and power limits.
fn fixed_info(d: &Device, power_limit_w: Option<f64>) -> Vec<Info> {
    let mut facts = Vec::new();
    if let Ok(arch) = d.architecture() {
        facts.push(info("Architecture", format!("{arch:?}")));
    }
    if let Ok(cores) = d.num_cores() {
        facts.push(info("CUDA cores", cores.to_string()));
    }
    if let Ok(bits) = d.memory_bus_width() {
        facts.push(info("Memory bus", format!("{bits}-bit")));
    }
    let max = |clock| d.max_clock_info(clock).ok();
    if let (Some(core), Some(mem)) = (max(Clock::Graphics), max(Clock::Memory)) {
        facts.push(info("Max clocks", format!("{core} MHz core · {mem} MHz memory")));
    }
    if let Some(limit) = power_limit_w {
        let default = d.power_management_limit_default().ok().map(|mw| mw as f64 / 1000.0);
        facts.push(info(
            "Power limit",
            match default {
                Some(def) if (def - limit).abs() >= 1.0 => format!("{limit:.0} W (default {def:.0} W)"),
                _ => format!("{limit:.0} W"),
            },
        ));
    }
    if let Ok(vbios) = d.vbios_version() {
        facts.push(info("VBIOS", vbios));
    }
    facts
}

/// Why clocks are held back right now, in plain words; empty when they are not.
fn throttle_text(reasons: ThrottleReasons) -> String {
    let names = [
        (ThrottleReasons::GPU_IDLE, "Idle"),
        (ThrottleReasons::SW_POWER_CAP, "Power limit"),
        (ThrottleReasons::HW_SLOWDOWN, "Hardware slowdown"),
        (ThrottleReasons::SW_THERMAL_SLOWDOWN, "Temperature limit"),
        (ThrottleReasons::HW_THERMAL_SLOWDOWN, "Hardware temperature limit"),
        (ThrottleReasons::HW_POWER_BRAKE_SLOWDOWN, "Power brake"),
        (ThrottleReasons::APPLICATIONS_CLOCKS_SETTING, "Application clocks"),
        (ThrottleReasons::SYNC_BOOST, "Sync boost"),
        (ThrottleReasons::DISPLAY_CLOCK_SETTING, "Display clock"),
    ];
    names.iter().filter(|(flag, _)| reasons.contains(*flag)).map(|(_, name)| *name).collect::<Vec<_>>().join(", ")
}

fn pstate_text(state: PerformanceState) -> String {
    let n = match state {
        PerformanceState::Zero => 0,
        PerformanceState::One => 1,
        PerformanceState::Two => 2,
        PerformanceState::Three => 3,
        PerformanceState::Four => 4,
        PerformanceState::Five => 5,
        PerformanceState::Six => 6,
        PerformanceState::Seven => 7,
        PerformanceState::Eight => 8,
        PerformanceState::Nine => 9,
        PerformanceState::Ten => 10,
        PerformanceState::Eleven => 11,
        PerformanceState::Twelve => 12,
        PerformanceState::Thirteen => 13,
        PerformanceState::Fourteen => 14,
        PerformanceState::Fifteen => 15,
        PerformanceState::Unknown => return "Unknown".into(),
    };
    match n {
        0 => "P0 (maximum performance)".into(),
        8 | 12 | 15 => format!("P{n} (idle)"),
        n => format!("P{n}"),
    }
}

impl Gpu {
    fn poll(&mut self, displays: &[super::displays::Display]) -> Group {
        let d = &self.device;
        let mut readings = Vec::with_capacity(16);

        if let Ok(t) = d.temperature(TemperatureSensor::Gpu) {
            readings.push(Reading {
                id: "temp",
                label: "Temperature",
                value: t as f64,
                unit: Unit::Celsius,
                range: Some((20.0, 95.0)),
                detail: None,
                featured: true,
                dashboard: true,
            });
        }

        if self.fan_count > 0 {
            let speeds: Vec<u32> = (0..self.fan_count).filter_map(|i| d.fan_speed(i).ok()).collect();
            if !speeds.is_empty() {
                let avg = speeds.iter().sum::<u32>() as f64 / speeds.len() as f64;
                let rpm = d.fan_speed_rpm(0).ok();
                let detail = match rpm {
                    _ if avg == 0.0 => "Zero RPM mode".to_string(),
                    Some(rpm) => format!("{rpm} RPM"),
                    None => format!("{} fans", self.fan_count),
                };
                readings.push(Reading {
                    id: "fan",
                    label: "Fan speed",
                    value: avg,
                    unit: Unit::Percent,
                    range: Some((0.0, 100.0)),
                    detail: Some(detail),
                    featured: true,
                    dashboard: true,
                });
            }
        }

        if let Ok(u) = d.utilization_rates() {
            readings.push(Reading {
                id: "load",
                label: "GPU load",
                value: u.gpu as f64,
                unit: Unit::Percent,
                range: Some((0.0, 100.0)),
                detail: None,
                featured: false,
                dashboard: true,
            });
        }

        if let Ok(u) = d.encoder_utilization() {
            readings.push(Reading {
                id: "nvenc",
                label: "Video encode",
                value: u.utilization as f64,
                unit: Unit::Percent,
                range: Some((0.0, 100.0)),
                detail: Some("NVENC".into()),
                featured: false,
                dashboard: true,
            });
        }

        if let Ok(u) = d.decoder_utilization() {
            readings.push(Reading {
                id: "nvdec",
                label: "Video decode",
                value: u.utilization as f64,
                unit: Unit::Percent,
                range: Some((0.0, 100.0)),
                detail: Some("NVDEC".into()),
                featured: false,
                dashboard: true,
            });
        }

        if let Ok(mw) = d.power_usage() {
            readings.push(Reading {
                id: "power",
                label: "Power draw",
                value: mw as f64 / 1000.0,
                unit: Unit::Watts,
                range: self.power_limit_w.map(|l| (0.0, l)),
                detail: self.power_limit_w.map(|l| format!("of {l:.0} W")),
                featured: false,
                dashboard: true,
            });
        }

        if let Ok(m) = d.memory_info() {
            readings.push(Reading {
                id: "vram",
                label: "VRAM used",
                value: m.used as f64 / MIB,
                unit: Unit::Mebibytes,
                range: Some((0.0, self.vram_total_mb)),
                detail: Some(format!("of {:.1} GB", self.vram_total_mb / 1024.0)),
                featured: false,
                dashboard: true,
            });
        }

        if let Ok(mhz) = d.clock_info(Clock::Graphics) {
            readings.push(Reading {
                id: "core_clock",
                label: "Core clock",
                value: mhz as f64,
                unit: Unit::Megahertz,
                range: None,
                detail: None,
                featured: false,
                dashboard: true,
            });
        }

        if let Ok(mhz) = d.clock_info(Clock::Memory) {
            readings.push(Reading {
                id: "mem_clock",
                label: "Memory clock",
                value: mhz as f64,
                unit: Unit::Megahertz,
                range: None,
                detail: None,
                featured: false,
                dashboard: true,
            });
        }

        // Component page only: bus traffic, individual fans, encoder sessions.
        let pcie = |counter| d.pcie_throughput(counter).ok().map(|kb| kb as f64 * 1024.0);
        if let Some(rx) = pcie(PcieUtilCounter::Receive) {
            readings.push(Reading::new("pcie_rx", "PCIe receive", rx, Unit::BytesPerSec).detail("to GPU"));
        }
        if let Some(tx) = pcie(PcieUtilCounter::Send) {
            readings.push(Reading::new("pcie_tx", "PCIe send", tx, Unit::BytesPerSec).detail("from GPU"));
        }
        if self.fan_count > 1 {
            const IDS: [&str; 4] = ["fan1", "fan2", "fan3", "fan4"];
            const LABELS: [&str; 4] = ["Fan 1", "Fan 2", "Fan 3", "Fan 4"];
            for i in 0..self.fan_count.min(4) {
                if let Ok(speed) = d.fan_speed(i) {
                    let mut r = Reading::new(IDS[i as usize], LABELS[i as usize], speed as f64, Unit::Percent)
                        .range(0.0, 100.0);
                    if let Ok(rpm) = d.fan_speed_rpm(i) {
                        r = r.detail(format!("{rpm} RPM"));
                    }
                    readings.push(r);
                }
            }
        }
        if let Ok(stats) = d.encoder_stats() {
            let mut r = Reading::new("enc_sessions", "Encoder sessions", stats.session_count as f64, Unit::Count);
            if stats.session_count > 0 {
                r = r.detail(format!("{} fps · {} µs latency", stats.average_fps, stats.average_latency));
            }
            readings.push(r);
        }

        // Live facts first, then the fixed ones, then the monitors on this GPU.
        let mut facts = Vec::new();
        if let Ok(state) = d.performance_state() {
            facts.push(info("Performance state", pstate_text(state)));
        }
        if let Ok(reasons) = d.current_throttle_reasons() {
            let text = throttle_text(reasons);
            facts.push(info("Clocks limited by", if text.is_empty() { "Nothing".into() } else { text }));
        }
        let link = |generation: Result<u32, NvmlError>, width: Result<u32, NvmlError>| {
            generation.ok().zip(width.ok()).map(|(g, w)| format!("Gen {g} × {w}"))
        };
        if let Some(now) = link(d.current_pcie_link_gen(), d.current_pcie_link_width()) {
            let max = link(d.max_pcie_link_gen(), d.max_pcie_link_width());
            facts.push(info(
                "PCIe link",
                match max {
                    // Links drop to Gen 1 when idle to save power.
                    Some(max) if max != now => format!("{now} (max {max})"),
                    _ => now,
                },
            ));
        }
        facts.extend(self.fixed.iter().cloned());
        for display in displays.iter().filter(|m| m.adapter == self.name) {
            facts.push(info(display.monitor.clone(), display.summary()));
        }

        Group {
            id: format!("nvml/{}", self.index),
            kind: Kind::Gpu,
            order: super::order::GPU + self.index as u16,
            breakdown: None,
            info: facts,
            processes: self.processes(),
            title: self.name.clone(),
            subtitle: if self.driver.is_empty() {
                "NVIDIA".into()
            } else {
                format!("NVIDIA · Driver {}", self.driver)
            },
            readings,
        }
    }

    /// Processes by share of the GPU's shader time since the last poll.
    fn processes(&mut self) -> Option<Processes> {
        let samples = self.device.process_utilization_stats(self.process_timestamp).ok()?;
        self.process_timestamp = samples.iter().map(|s| s.timestamp).max().unwrap_or(self.process_timestamp);
        // Several samples per process can arrive between polls; average them.
        let mut per_pid: std::collections::HashMap<u32, (u32, u32)> = std::collections::HashMap::new();
        for s in &samples {
            let e = per_pid.entry(s.pid).or_default();
            e.0 += s.sm_util;
            e.1 += 1;
        }
        let entries = per_pid.into_iter().filter(|(_, (sum, _))| *sum > 0).map(|(pid, (sum, n))| {
            let util = sum as f64 / n as f64;
            // NVML fails for some system processes (dwm.exe); Windows still knows them.
            let name = super::processes::image_name(pid)
                .or_else(|| {
                    let path = self.nvml.sys_process_name(pid, 256).ok()?;
                    path.rsplit(['\\', '/']).next().map(str::to_string)
                })
                .unwrap_or_else(|| format!("PID {pid}"));
            (name, util)
        });
        let items: Vec<ProcessItem> = top_processes(entries, 8);
        Some(Processes { metric: "GPU", unit: Unit::Percent, items })
    }
}
