//! Sensor abstraction. Every data source (NVML today, LibreHardwareMonitor etc. later)
//! implements [`SensorProvider`] and reports plain [`Group`]s of [`Reading`]s.
//! The UI only knows about these types, so new providers need no UI changes.

pub mod nvml;

/// What a value means — drives formatting and colour in the UI.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Unit {
    Celsius,
    Percent,
    Watts,
    Megahertz,
    Mebibytes,
}

impl Unit {
    pub fn suffix(self) -> &'static str {
        match self {
            Unit::Celsius => "°C",
            Unit::Percent => "%",
            Unit::Watts => "W",
            Unit::Megahertz => "MHz",
            Unit::Mebibytes => "MB",
        }
    }
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

/// One physical device (a GPU, a CPU, a motherboard …).
#[derive(Clone, Debug)]
pub struct Group {
    /// Stable key, e.g. `"nvml/0"`.
    pub id: String,
    pub title: String,
    pub subtitle: String,
    pub readings: Vec<Reading>,
}

pub trait SensorProvider {
    /// Called once per tick on the poller thread. Must be cheap and non-blocking.
    fn poll(&mut self) -> Vec<Group>;
}

/// Builds every available provider. Failures are returned as messages for the UI
/// instead of aborting, so one missing source does not take down the app.
pub fn create_providers() -> (Vec<Box<dyn SensorProvider>>, Vec<String>) {
    let mut providers: Vec<Box<dyn SensorProvider>> = Vec::new();
    let mut errors = Vec::new();

    match nvml::NvmlProvider::new() {
        Ok(p) => providers.push(Box::new(p)),
        Err(e) => errors.push(format!("NVIDIA: {e}")),
    }

    (providers, errors)
}
