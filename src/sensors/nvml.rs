//! NVIDIA GPUs via NVML (`nvml.dll`, shipped with the driver). Talks to the driver
//! directly — no `nvidia-smi` process per poll.

use nvml_wrapper::Device;
use nvml_wrapper::Nvml;
use nvml_wrapper::error::NvmlError;
use nvml_wrapper::enum_wrappers::device::{Clock, TemperatureSensor};

use super::{Group, Kind, Reading, SensorProvider, Unit};

pub struct NvmlProvider {
    gpus: Vec<Gpu>,
}

struct Gpu {
    index: u32,
    device: Device<'static>,
    name: String,
    driver: String,
    fan_count: u32,
    power_limit_w: Option<f64>,
    vram_total_mb: f64,
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
            gpus.push(Gpu {
                index,
                fan_count: device.num_fans().unwrap_or(0),
                power_limit_w: device.enforced_power_limit().ok().map(|mw| mw as f64 / 1000.0),
                vram_total_mb: device.memory_info().map(|m| m.total as f64 / MIB).unwrap_or(0.0),
                name,
                driver: driver.clone(),
                device,
            });
        }
        Ok((!gpus.is_empty()).then_some(Self { gpus }))
    }
}

const MIB: f64 = 1024.0 * 1024.0;

impl SensorProvider for NvmlProvider {
    fn poll(&mut self) -> Vec<Group> {
        self.gpus.iter().map(Gpu::poll).collect()
    }
}

impl Gpu {
    fn poll(&self) -> Group {
        let d = &self.device;
        let mut readings = Vec::with_capacity(9);

        if let Ok(t) = d.temperature(TemperatureSensor::Gpu) {
            readings.push(Reading {
                id: "temp",
                label: "Temperature",
                value: t as f64,
                unit: Unit::Celsius,
                range: Some((20.0, 95.0)),
                detail: None,
                featured: true,
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
            });
        }

        Group {
            id: format!("nvml/{}", self.index),
            kind: Kind::Gpu,
            order: super::order::GPU + self.index as u16,
            breakdown: None,
            title: self.name.clone(),
            subtitle: if self.driver.is_empty() {
                "NVIDIA".into()
            } else {
                format!("NVIDIA · Driver {}", self.driver)
            },
            readings,
        }
    }
}
