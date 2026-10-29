//! Any GPU (AMD, Intel, Qualcomm, integrated or not) the way Task Manager sees it:
//! engine load and memory from PDH's `GPU Engine` / `GPU Adapter Memory` counters,
//! temperature, fan and memory clock from the graphics kernel's adapter perf data,
//! names and memory sizes from DXGI. NVIDIA cards are left to NVML when it works.

use std::collections::HashMap;

use windows::Wdk::Graphics::Direct3D::{
    D3DKMT_ADAPTER_PERFDATA, D3DKMT_ADAPTER_PERFDATACAPS, D3DKMT_CLOSEADAPTER, D3DKMT_OPENADAPTERFROMLUID,
    D3DKMT_QUERYADAPTERINFO, D3DKMTCloseAdapter, D3DKMTOpenAdapterFromLuid, D3DKMTQueryAdapterInfo,
    KMTQAITYPE_ADAPTERPERFDATA, KMTQAITYPE_ADAPTERPERFDATA_CAPS,
};
use windows::Win32::Foundation::LUID;
use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory1, DXGI_ADAPTER_DESC1, DXGI_ADAPTER_FLAG_SOFTWARE, IDXGIFactory1,
};

use super::pdh::{Counter, Query};
use super::{Group, Kind, Processes, Reading, SensorProvider, Unit, info, order, top_processes};

const NVIDIA: u32 = 0x10DE;
const MIB: f64 = 1024.0 * 1024.0;

pub struct GpuProvider {
    query: Query,
    engines: Option<Counter>,
    dedicated: Option<Counter>,
    shared: Option<Counter>,
    adapters: Vec<Adapter>,
    /// Connected monitors, refreshed every few polls.
    displays: Vec<super::displays::Display>,
    polls: u32,
}

struct Adapter {
    id: String,
    name: String,
    /// PDH instance prefix, e.g. `"luid_0x00000000_0x0000D1B5"`.
    luid_key: String,
    kmt: Option<u32>,
    dedicated_total: u64,
    shared_total: u64,
    integrated: bool,
    max_temp: Option<f64>,
    max_fan_rpm: Option<u32>,
    max_memory_hz: Option<u64>,
    vendor: &'static str,
}

impl GpuProvider {
    /// `skip_nvidia` when NVML already covers NVIDIA cards (with more detail).
    /// `None` when there is nothing left to show.
    pub fn new(skip_nvidia: bool) -> Option<Self> {
        let factory: IDXGIFactory1 = unsafe { CreateDXGIFactory1() }.ok()?;
        let mut found: Vec<DXGI_ADAPTER_DESC1> = Vec::new();
        for i in 0.. {
            let Ok(adapter) = (unsafe { factory.EnumAdapters1(i) }) else { break };
            let Ok(desc) = (unsafe { adapter.GetDesc1() }) else { continue };
            let software = desc.Flags & DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32 != 0;
            let same = |d: &DXGI_ADAPTER_DESC1| {
                d.AdapterLuid.LowPart == desc.AdapterLuid.LowPart && d.AdapterLuid.HighPart == desc.AdapterLuid.HighPart
            };
            if software || (skip_nvidia && desc.VendorId == NVIDIA) || found.iter().any(same) {
                continue;
            }
            found.push(desc);
        }
        if found.is_empty() {
            return None;
        }

        // Integrated GPUs share system RAM and have little or no dedicated memory.
        // Next to a dedicated card they are secondary; alone (laptops, Surface) they
        // are the GPU.
        let has_dedicated = skip_nvidia || found.iter().any(|d| d.DedicatedVideoMemory >= 1 << 30);
        let mut ids: HashMap<String, u32> = HashMap::new();
        let adapters = found
            .into_iter()
            .map(|d| {
                let name = wide(&d.Description);
                // LUIDs change on every boot; the PCI ids do not.
                let base = format!("gpu/{:04X}:{:04X}:{:08X}", d.VendorId, d.DeviceId, d.SubSysId);
                let n = ids.entry(base.clone()).or_default();
                *n += 1;
                let id = if *n == 1 { base } else { format!("{base}/{n}") };
                let integrated = d.DedicatedVideoMemory < 1 << 30;
                let kmt = open_kmt(d.AdapterLuid);
                let caps = kmt.and_then(perf_caps);
                Adapter {
                    id,
                    name,
                    luid_key: format!("luid_0x{:08X}_0x{:08X}", d.AdapterLuid.HighPart as u32, d.AdapterLuid.LowPart),
                    kmt,
                    dedicated_total: d.DedicatedVideoMemory as u64,
                    shared_total: d.SharedSystemMemory as u64,
                    integrated: integrated && has_dedicated,
                    max_temp: caps.and_then(|c| (c.TemperatureMax > 0).then_some(c.TemperatureMax as f64 / 10.0)),
                    max_fan_rpm: caps.and_then(|c| (c.MaxFanRPM > 0).then_some(c.MaxFanRPM)),
                    max_memory_hz: kmt
                        .and_then(perf_data)
                        .and_then(|p| (p.MaxMemoryFrequency > 0).then_some(p.MaxMemoryFrequency)),
                    vendor: match d.VendorId {
                        0x1002 | 0x1022 => "AMD",
                        0x8086 => "Intel",
                        NVIDIA => "NVIDIA",
                        0x5143 | 0x4D4F4351 => "Qualcomm",
                        0x1414 => "Microsoft",
                        _ => "",
                    },
                }
            })
            .collect();

        let query = Query::new()?;
        let engines = query.add(r"\GPU Engine(*)\Utilization Percentage");
        let dedicated = query.add(r"\GPU Adapter Memory(*)\Dedicated Usage");
        let shared = query.add(r"\GPU Adapter Memory(*)\Shared Usage");
        query.collect();
        Some(Self { query, engines, dedicated, shared, adapters, displays: Vec::new(), polls: 0 })
    }
}

impl SensorProvider for GpuProvider {
    fn poll(&mut self) -> Vec<Group> {
        self.query.collect();
        if self.polls.is_multiple_of(5) {
            self.displays = super::displays::displays();
        }
        self.polls = self.polls.wrapping_add(1);
        let engines = self.engines.map(|c| self.query.array(c)).unwrap_or_default();
        let dedicated = self.dedicated.map(|c| self.query.array(c)).unwrap_or_default();
        let shared = self.shared.map(|c| self.query.array(c)).unwrap_or_default();
        let counters = Counters { engines: &engines, dedicated: &dedicated, shared: &shared };
        self.adapters.iter().enumerate().map(|(i, a)| a.group(i, &counters, &self.displays)).collect()
    }
}

/// One poll's PDH arrays, shared by all adapters.
struct Counters<'a> {
    engines: &'a [(String, f64)],
    dedicated: &'a [(String, f64)],
    shared: &'a [(String, f64)],
}

impl Adapter {
    fn group(&self, index: usize, counters: &Counters, displays: &[super::displays::Display]) -> Group {
        let Counters { engines, dedicated, shared } = *counters;
        // Instances look like "pid_1234_luid_0x…_0x…_phys_0_eng_3_engtype_VideoDecode".
        // Each engine's load is the sum over processes; an engine type shows its
        // busiest engine, and the GPU as a whole its busiest engine type (like
        // Task Manager).
        let mut per_engine: HashMap<(&str, String), f64> = HashMap::new();
        let mut per_pid: HashMap<u32, HashMap<&str, f64>> = HashMap::new();
        for (name, value) in engines.iter().filter(|(n, _)| self.owns(n)) {
            let Some(engine) = name.split("_phys_").nth(1) else { continue };
            let kind = engine_kind(engine.rsplit("_engtype_").next().unwrap_or(""));
            *per_pid
                .entry(name.strip_prefix("pid_").and_then(|s| s.split('_').next()?.parse().ok()).unwrap_or(0))
                .or_default()
                .entry(engine)
                .or_default() += value;
            *per_engine.entry((engine, kind)).or_default() += value;
        }
        let mut per_kind: HashMap<String, f64> = HashMap::new();
        for ((_, kind), value) in per_engine {
            let e = per_kind.entry(kind).or_default();
            *e = e.max(value);
        }
        let pct = |v: f64| v.clamp(0.0, 100.0);
        let memory = |list: &[(String, f64)]| list.iter().filter(|(n, _)| self.owns(n)).map(|(_, v)| *v).sum::<f64>();

        let perf = self.kmt.and_then(perf_data);
        let mut readings = Vec::new();
        if let Some(p) = perf.filter(|p| p.Temperature > 0) {
            readings.push(Reading {
                featured: !self.integrated,
                ..Reading::new("temp", "Temperature", p.Temperature as f64 / 10.0, Unit::Celsius)
                    .range(20.0, self.max_temp.unwrap_or(95.0))
                    .on_dashboard()
            });
        }
        if let Some(p) = perf.filter(|p| p.FanRPM > 0 || self.max_fan_rpm.is_some()) {
            let mut r = Reading::new("fan", "Fan speed", p.FanRPM as f64, Unit::Count).detail("RPM").on_dashboard();
            if let Some(max) = self.max_fan_rpm {
                r = r.range(0.0, max as f64);
            }
            readings.push(r);
        }
        readings.push(
            Reading::new("load", "GPU load", pct(per_kind.values().copied().fold(0.0, f64::max)), Unit::Percent)
                .range(0.0, 100.0)
                .detail("busiest engine")
                .on_dashboard(),
        );
        // AMD has one "Video Codec" engine for both directions, others separate ones.
        for (id, label, kind, dashboard) in [
            ("3d", "3D", "3d", true),
            ("encode", "Video encode", "videoencode", true),
            ("decode", "Video decode", "videodecode", true),
            ("codec", "Video codec", "videocodec", true),
            ("copy", "Copy", "copy", false),
            ("compute", "Compute", "compute", false),
        ] {
            if let Some(v) = per_kind.get(kind) {
                let r = Reading::new(id, label, pct(*v), Unit::Percent).range(0.0, 100.0);
                readings.push(if dashboard { r.on_dashboard() } else { r });
            }
        }
        if self.dedicated_total >= 64 << 20 {
            let total = self.dedicated_total as f64 / MIB;
            readings.push(
                Reading::new("vram", "VRAM used", memory(dedicated) / MIB, Unit::Mebibytes)
                    .range(0.0, total)
                    .detail(format!("of {:.1} GB", total / 1024.0))
                    .on_dashboard(),
            );
        }
        let shared_total = self.shared_total as f64 / MIB;
        let mut shared_reading = Reading::new("shared", "Shared memory", memory(shared) / MIB, Unit::Mebibytes)
            .range(0.0, shared_total)
            .detail(format!("of {:.1} GB system RAM", shared_total / 1024.0));
        // For integrated GPUs, shared RAM is their main memory.
        if self.dedicated_total < 64 << 20 {
            shared_reading = shared_reading.on_dashboard();
        }
        readings.push(shared_reading);
        if let Some(p) = perf.filter(|p| p.MemoryFrequency > 0) {
            readings.push(Reading::new("mem_clock", "Memory clock", p.MemoryFrequency as f64 / 1e6, Unit::Megahertz));
        }

        let mut facts = Vec::new();
        if !self.vendor.is_empty() {
            facts.push(info("Vendor", self.vendor));
        }
        facts.push(info("Type", if self.dedicated_total >= 1 << 30 { "Dedicated" } else { "Integrated" }));
        facts.push(info("Dedicated memory", format!("{:.1} GB", self.dedicated_total as f64 / (1u64 << 30) as f64)));
        facts.push(info("Shared memory", format!("{:.1} GB", self.shared_total as f64 / (1u64 << 30) as f64)));
        if let Some(hz) = self.max_memory_hz {
            facts.push(info("Max memory clock", format!("{:.0} MHz", hz as f64 / 1e6)));
        }
        let mut engine_types: Vec<&str> = per_kind.keys().map(|k| engine_label(k)).filter(|k| !k.is_empty()).collect();
        engine_types.sort_unstable();
        engine_types.dedup();
        if !engine_types.is_empty() {
            facts.push(info("Engines", engine_types.join(", ")));
        }
        for display in displays.iter().filter(|d| d.adapter == self.name) {
            facts.push(info(display.monitor.clone(), display.summary()));
        }

        // A process's load is its busiest engine, as in Task Manager's GPU column.
        let by_gpu = per_pid.iter().filter(|(pid, _)| **pid != 0).filter_map(|(pid, engines)| {
            let busiest = engines.values().copied().fold(0.0, f64::max);
            (busiest >= 0.5)
                .then(|| (super::processes::image_name(*pid).unwrap_or_else(|| format!("PID {pid}")), pct(busiest)))
        });

        Group {
            id: self.id.clone(),
            kind: if self.integrated { Kind::IntegratedGpu } else { Kind::Gpu },
            order: order::GPU + 50 + index as u16,
            title: self.name.clone(),
            subtitle: [self.vendor, if self.dedicated_total >= 1 << 30 { "Dedicated" } else { "Integrated" }]
                .into_iter()
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
                .join(" · "),
            readings,
            breakdown: None,
            info: facts,
            processes: Some(Processes { metric: "GPU", unit: Unit::Percent, items: top_processes(by_gpu, 8) }),
        }
    }
}

impl Drop for GpuProvider {
    fn drop(&mut self) {
        for a in &self.adapters {
            if let Some(handle) = a.kmt {
                let close = D3DKMT_CLOSEADAPTER { hAdapter: handle };
                let _ = unsafe { D3DKMTCloseAdapter(&close) };
            }
        }
    }
}

impl Adapter {
    /// Whether a PDH instance name belongs to this adapter (hex case varies).
    fn owns(&self, instance: &str) -> bool {
        instance.len() >= self.luid_key.len()
            && instance
                .as_bytes()
                .windows(self.luid_key.len())
                .any(|w| w.eq_ignore_ascii_case(self.luid_key.as_bytes()))
    }
}

/// Engine type without vendor spelling and numbering: "Video Decode 1",
/// "VideoDecode" and "videodecode_0" all become "videodecode".
fn engine_kind(raw: &str) -> String {
    let trimmed = raw.trim_end_matches(|c: char| c.is_ascii_digit() || c == '_' || c == ' ');
    trimmed.chars().filter(|c| !matches!(c, ' ' | '_')).collect::<String>().to_ascii_lowercase()
}

fn engine_label(kind: &str) -> &str {
    match kind {
        "3d" => "3D",
        "highpriority3d" => "3D (high priority)",
        "videoencode" => "Video encode",
        "videodecode" => "Video decode",
        "videocodec" => "Video codec",
        "videojpeg" => "JPEG",
        "videoprocessing" => "Video processing",
        "copy" => "Copy",
        "compute" => "Compute",
        "highprioritycompute" => "Compute (high priority)",
        "security" => "Security",
        "legacyoverlay" => "Overlay",
        "timer" => "Timer",
        "vr" => "VR",
        "ofa" => "Optical flow",
        "cuda" => "CUDA",
        other => other,
    }
}

fn open_kmt(luid: LUID) -> Option<u32> {
    let mut open = D3DKMT_OPENADAPTERFROMLUID { AdapterLuid: luid, hAdapter: 0 };
    unsafe { D3DKMTOpenAdapterFromLuid(&mut open) }.is_ok().then_some(open.hAdapter)
}

fn query<T: Default>(handle: u32, kind: windows::Wdk::Graphics::Direct3D::KMTQUERYADAPTERINFOTYPE) -> Option<T> {
    let mut data = T::default();
    let mut q = D3DKMT_QUERYADAPTERINFO {
        hAdapter: handle,
        Type: kind,
        pPrivateDriverData: (&mut data as *mut T).cast(),
        PrivateDriverDataSize: size_of::<T>() as u32,
    };
    unsafe { D3DKMTQueryAdapterInfo(&mut q) }.is_ok().then_some(data)
}

/// Temperature (deci-°C), fan RPM and memory clock right now.
fn perf_data(handle: u32) -> Option<D3DKMT_ADAPTER_PERFDATA> {
    query(handle, KMTQAITYPE_ADAPTERPERFDATA)
}

fn perf_caps(handle: u32) -> Option<D3DKMT_ADAPTER_PERFDATACAPS> {
    query(handle, KMTQAITYPE_ADAPTERPERFDATA_CAPS)
}

fn wide(chars: &[u16]) -> String {
    let end = chars.iter().position(|&c| c == 0).unwrap_or(chars.len());
    String::from_utf16_lossy(&chars[..end]).trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::engine_kind;

    #[test]
    fn engine_names_from_different_vendors_match() {
        assert_eq!(engine_kind("VideoDecode"), "videodecode");
        assert_eq!(engine_kind("Video Decode 1"), "videodecode");
        assert_eq!(engine_kind("Video Codec 0"), "videocodec");
        assert_eq!(engine_kind("Compute_1"), "compute");
        assert_eq!(engine_kind("3D"), "3d");
        assert_eq!(engine_kind("High Priority 3D"), "highpriority3d");
        assert_eq!(engine_kind("Security_1"), "security");
    }
}
