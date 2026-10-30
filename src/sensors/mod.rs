//! Sensor abstraction. Every data source (NVML, Windows counters, LibreHardwareMonitor
//! later) implements [`SensorProvider`] and reports plain [`Group`]s of [`Reading`]s.
//! The UI only knows about these types, so new providers need no UI changes.

mod battery;
mod displays;
mod gpu;
mod netinfo;
pub mod nvml;
mod pdh;
mod processes;
pub use processes::Row as ProcessRow;
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
    Milliseconds,
    /// Events per second (context switches, page faults).
    PerSecond,
    /// A plain number (queue length, sessions).
    Count,
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
    /// Shown on the dashboard unless switched off in Settings. Readings for a closer
    /// look (latencies, pools, PCIe traffic) only appear on the component pages
    /// unless switched on.
    pub dashboard: bool,
}

impl Reading {
    /// A component-page-only reading; chain the setters for the rest.
    pub fn new(id: &'static str, label: &'static str, value: f64, unit: Unit) -> Self {
        Self { id, label, value, unit, range: None, detail: None, featured: false, dashboard: false }
    }

    pub fn range(mut self, lo: f64, hi: f64) -> Self {
        self.range = Some((lo, hi));
        self
    }

    pub fn detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    pub fn on_dashboard(mut self) -> Self {
        self.dashboard = true;
        self
    }
}

/// A fact about a component that is text rather than a measurement, e.g.
/// `"PCIe link" → "Gen 4 × 16"`. Shown on the component and Performance pages.
#[derive(Clone, Debug)]
pub struct Info {
    pub label: String,
    pub value: String,
}

pub fn info(label: impl Into<String>, value: impl Into<String>) -> Info {
    Info { label: label.into(), value: value.into() }
}

/// The processes using a component the most, e.g. by CPU time or memory.
#[derive(Clone, Debug)]
pub struct Processes {
    /// What the list is sorted by, e.g. `"CPU"`.
    pub metric: &'static str,
    pub unit: Unit,
    pub items: Vec<ProcessItem>,
}

/// One program; several processes of the same executable (browser tabs) are summed.
#[derive(Clone, Debug)]
pub struct ProcessItem {
    pub name: String,
    pub count: u32,
    pub value: f64,
}

/// The `n` largest of `(name, value)` pairs, summed per name.
pub fn top_processes(entries: impl IntoIterator<Item = (String, f64)>, n: usize) -> Vec<ProcessItem> {
    let mut by_name: std::collections::HashMap<String, (u32, f64)> = std::collections::HashMap::new();
    for (name, value) in entries {
        let e = by_name.entry(name).or_default();
        e.0 += 1;
        e.1 += value;
    }
    let mut items: Vec<ProcessItem> =
        by_name.into_iter().map(|(name, (count, value))| ProcessItem { name, count, value }).collect();
    items.sort_by(|a, b| b.value.total_cmp(&a.value).then_with(|| a.name.cmp(&b.name)));
    items.truncate(n);
    items
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
    /// Text facts for the detail pages, in display order.
    pub info: Vec<Info>,
    /// Top processes, shown on the detail page.
    pub processes: Option<Processes>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Kind {
    Cpu,
    Gpu,
    /// A GPU built into the CPU, next to a dedicated card (alone it is just [`Kind::Gpu`]).
    IntegratedGpu,
    Memory,
    Disk,
    /// USB sticks, USB enclosures and other hot-pluggable drives.
    ExternalDisk,
    Network,
    Battery,
}

impl Kind {
    pub fn label(self) -> &'static str {
        match self {
            Kind::Cpu => "Processor",
            Kind::Gpu => "Graphics",
            Kind::IntegratedGpu => "Integrated graphics",
            Kind::Memory => "Memory",
            Kind::Disk => "Storage",
            Kind::ExternalDisk => "External storage",
            Kind::Network => "Network",
            Kind::Battery => "Battery",
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
    pub const GPU: u16 = 0;
    pub const CPU: u16 = 100;
    pub const MEMORY: u16 = 200;
    pub const BATTERY: u16 = 250;
    pub const NETWORK: u16 = 300;
    pub const DISK: u16 = 400;
}

pub trait SensorProvider {
    /// Called once per tick on the poller thread. Must be cheap and non-blocking.
    fn poll(&mut self) -> Vec<Group>;

    /// Name of the whole machine (e.g. the mainboard), if this provider knows it.
    fn machine(&self) -> Option<String> {
        None
    }

    /// Every running process, if this provider lists them. Only called while the
    /// Processes page is open, as it looks up facts for new processes.
    fn process_table(&mut self) -> Option<Vec<ProcessRow>> {
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
    let has_nvml = match nvml::NvmlProvider::new() {
        Ok(Some(p)) => {
            providers.push(Box::new(p));
            true
        }
        Ok(None) => false, // no NVIDIA GPU in this machine
        Err(e) => {
            errors.push(format!("NVIDIA: {e}"));
            false
        }
    };
    // Every other GPU, and NVIDIA ones too when NVML is not available.
    if let Some(p) = gpu::GpuProvider::new(has_nvml) {
        providers.push(Box::new(p));
    }

    (providers, errors)
}
