//! Laptop battery: charge from `GetSystemPowerStatus`, charge/discharge rate and wear
//! from the battery driver's WMI classes (`root\wmi`). Desktops get no group.

use serde::Deserialize;
use windows::Win32::System::Power::{GetSystemPowerStatus, SYSTEM_POWER_STATUS};

use super::{Group, Kind, Reading, Unit, info, order};

pub struct Battery {
    wmi: Option<wmi::WMIConnection>,
    /// Full charge vs. design capacity in mWh, read once.
    capacity: Option<(u32, u32)>,
}

#[derive(Deserialize)]
#[serde(rename = "BatteryStatus", rename_all = "PascalCase")]
struct BatteryStatus {
    charge_rate: Option<i32>,
    discharge_rate: Option<i32>,
}

#[derive(Deserialize)]
#[serde(rename = "BatteryFullChargedCapacity", rename_all = "PascalCase")]
struct FullCharged {
    full_charged_capacity: Option<u32>,
}

#[derive(Deserialize)]
#[serde(rename = "BatteryStaticData", rename_all = "PascalCase")]
struct StaticData {
    designed_capacity: Option<u32>,
}

impl Battery {
    /// `None` on machines without a battery.
    pub fn detect() -> Option<Self> {
        status().filter(|s| s.BatteryFlag & 128 == 0)?;
        let wmi = wmi::WMIConnection::with_namespace_path(r"ROOT\WMI").ok();
        let capacity = wmi.as_ref().and_then(|w| {
            let full = w.query::<FullCharged>().ok()?.into_iter().next()?.full_charged_capacity?;
            let design = w.query::<StaticData>().ok()?.into_iter().next()?.designed_capacity?;
            (design > 0).then_some((full, design))
        });
        Some(Self { wmi, capacity })
    }

    pub fn group(&self) -> Option<Group> {
        let s = status()?;
        let plugged = s.ACLineStatus == 1;
        let mut readings = Vec::new();
        if s.BatteryLifePercent <= 100 {
            readings.push(
                Reading::new("charge", "Charge", s.BatteryLifePercent as f64, Unit::Percent)
                    .range(0.0, 100.0)
                    .detail(if plugged { "Plugged in" } else { "On battery" })
                    .on_dashboard(),
            );
        }
        let rate = self.wmi.as_ref().and_then(|w| w.query::<BatteryStatus>().ok()?.into_iter().next());
        if let Some(rate) = rate {
            // mW; the driver reports whichever direction energy is flowing.
            let (label, mw) =
                if plugged { ("Charging", rate.charge_rate) } else { ("Discharging", rate.discharge_rate) };
            if let Some(mw) = mw.filter(|mw| *mw > 0) {
                readings
                    .push(Reading::new("rate", "Power", mw as f64 / 1000.0, Unit::Watts).detail(label).on_dashboard());
            }
        }

        let mut facts = vec![info("Power source", if plugged { "Plugged in" } else { "Battery" })];
        if !plugged && s.BatteryLifeTime != u32::MAX {
            let minutes = s.BatteryLifeTime / 60;
            facts.push(info("Time left", format!("{} h {:02} min", minutes / 60, minutes % 60)));
        }
        if let Some((full, design)) = self.capacity {
            facts.push(info(
                "Full charge",
                format!("{:.1} Wh of {:.1} Wh design", full as f64 / 1000.0, design as f64 / 1000.0),
            ));
            facts.push(info("Wear", format!("{:.0}%", (1.0 - full as f64 / design as f64).max(0.0) * 100.0)));
        }

        Some(Group {
            id: "battery".into(),
            kind: Kind::Battery,
            order: order::BATTERY,
            title: "Battery".into(),
            subtitle: if plugged { "Plugged in".into() } else { "On battery".into() },
            readings,
            breakdown: None,
            info: facts,
            processes: None,
        })
    }
}

fn status() -> Option<SYSTEM_POWER_STATUS> {
    let mut s = SYSTEM_POWER_STATUS::default();
    unsafe { GetSystemPowerStatus(&mut s) }.ok()?;
    Some(s)
}
