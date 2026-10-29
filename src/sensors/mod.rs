//! Sensor abstraction. Every data source (NVML, Windows counters, LibreHardwareMonitor
//! later) implements [`SensorProvider`] and reports plain [`Group`]s of [`Reading`]s.
//! The UI only knows about these types, so new providers need no UI changes.

pub mod nvml;
mod pdh;
pub mod system;

/// What a value means — drives formatting and colour in the UI.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Unit {
    Celsius,
    Percent,
    Watts,
    Megahertz,
    Mebibytes,
    /// Disk throughput.
    BytesPerSec,
    /// Network throughput.
    BitsPerSec,
}

#[derive(Clone, Debug)]
pub struct Reading {
    /// Stable key within its group, used to track history (e.g. `"temp"`).
    pub id: &'static str,
    pub label: &'static str,
    pub value: f64,
    pub unit: Unit,
    /// Scale for gauges and bars. `None` means unbounded (sparkline auto-scales).
    pub range: Option<(f64, f64)>,
    /// Secondary line under the value, e.g. `"1450 RPM"` or `"of 285 W"`.
    pub detail: Option<String>,
    /// Featured readings are drawn as large gauges.
    pub featured: bool,
}

/// One physical component (a GPU, a CPU, a disk …).
#[derive(Clone, Debug)]
pub struct Group {
    /// Stable key, e.g. `"nvml/0"`.
    pub id: String,
    pub kind: Kind,
    /// Display position across all providers, see [`order`].
    pub order: u16,
    pub title: String,
    pub subtitle: String,
    pub readings: Vec<Reading>,
    /// Optional fine-grained values (e.g. per CPU thread), shown on the detail page.
    pub breakdown: Option<Breakdown>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Kind {
    Cpu,
    Gpu,
    Memory,
    Disk,
    Network,
}

impl Kind {
    pub fn label(self) -> &'static str {
        match self {
            Kind::Cpu => "Processor",
            Kind::Gpu => "Graphics",
            Kind::Memory => "Memory",
            Kind::Disk => "Storage",
            Kind::Network => "Network",
        }
    }
}

/// Many values of the same kind, e.g. load per logical processor.
#[derive(Clone, Debug)]
pub struct Breakdown {
    pub title: &'static str,
    pub subtitle: &'static str,
    pub unit: Unit,
    pub range: Option<(f64, f64)>,
    pub items: Vec<BreakdownItem>,
}

#[derive(Clone, Debug)]
pub struct BreakdownItem {
    /// Also the history key within the group, so it must be stable.
    pub label: String,
    pub value: f64,
    pub detail: Option<String>,
}

/// Sort keys so groups from different providers interleave sensibly.
pub mod order {
    pub const CPU: u16 = 0;
    pub const GPU: u16 = 100;
    pub const MEMORY: u16 = 200;
    pub const DISK: u16 = 300;
    pub const NETWORK: u16 = 400;
}

pub trait SensorProvider {
    /// Called once per tick on the poller thread. Must be cheap and non-blocking.
    fn poll(&mut self) -> Vec<Group>;

    /// Name of the whole machine (e.g. the mainboard), if this provider knows it.
    fn machine(&self) -> Option<String> {
        None
    }
}

/// Builds every available provider. Failures are returned as messages for the UI
/// instead of aborting, so one missing source does not take down the app.
pub fn create_providers() -> (Vec<Box<dyn SensorProvider>>, Vec<String>) {
    let mut providers: Vec<Box<dyn SensorProvider>> = Vec::new();
    let mut errors = Vec::new();

    match system::SystemProvider::new() {
        Ok(p) => providers.push(Box::new(p)),
        Err(e) => errors.push(format!("System: {e}")),
    }
    match nvml::NvmlProvider::new() {
        Ok(p) => providers.push(Box::new(p)),
        Err(e) => errors.push(format!("NVIDIA: {e}")),
    }

    (providers, errors)
}
