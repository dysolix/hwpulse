//! User choices, persisted by eframe (`%APPDATA%\hwpulse\data\app.ron`).
//!
//! Only deviations from the defaults are stored, so a newly detected component
//! follows the default for its kind: GPUs, CPU and memory are shown, drives and
//! network adapters start hidden.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::sensors::{Group, Kind, Reading};

pub const STORAGE_KEY: &str = "settings";

#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// [`Group::id`]s hidden from the dashboard although shown by default.
    pub hidden_groups: BTreeSet<String>,
    /// [`Group::id`]s shown on the dashboard although hidden by default.
    pub shown_groups: BTreeSet<String>,
    /// `"{group id}/{reading id}"` of cards hidden from the dashboard although shown
    /// by default.
    pub hidden_readings: BTreeSet<String>,
    /// `"{group id}/{reading id}"` of component-page-only readings added to the dashboard.
    pub shown_readings: BTreeSet<String>,
    /// Custom dashboard order as [`Group::id`]s; empty means the default order.
    pub order: Vec<String>,
    pub animation: Animation,
    /// The former "Animate in the background" switch; only read, to carry it over
    /// into [`Settings::animation`].
    #[serde(skip_serializing)]
    pub slide_in_background: bool,
    /// Keys of the Processes page's shown columns; empty means the default set.
    pub process_columns: Vec<String>,
    pub always_on_top: bool,
    /// Minimize hwpulse after "Switch to" brings another app to the front.
    pub minimize_on_use: bool,
    pub speed: Speed,
    /// The small window with just the dashboard's values.
    pub compact: bool,
    /// "Copy hardware" lists every component's details, not just its name.
    pub copy_details: bool,
    /// The user switched off the daily check for a new release.
    pub no_update_check: bool,
}

/// When charts slide new samples in; otherwise they jump once per sample, which
/// costs almost nothing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Animation {
    Always,
    /// Only while hwpulse is the active window.
    #[default]
    Foreground,
    Never,
}

impl Animation {
    pub const ALL: [Animation; 3] = [Animation::Always, Animation::Foreground, Animation::Never];

    pub fn label(self) -> &'static str {
        match self {
            Animation::Always => "Always",
            Animation::Foreground => "Only while active",
            Animation::Never => "Never",
        }
    }

    pub fn slides(self, focused: bool) -> bool {
        match self {
            Animation::Always => true,
            Animation::Foreground => focused,
            Animation::Never => false,
        }
    }
}

impl Settings {
    /// Carries over choices stored by older versions.
    pub fn migrate(mut self) -> Self {
        if std::mem::take(&mut self.slide_in_background) {
            self.animation = Animation::Always;
        }
        self
    }
}

/// How often sensors are read, as Task Manager's "Update speed".
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Speed {
    High,
    #[default]
    Normal,
    Low,
    Paused,
}

impl Speed {
    pub const ALL: [Speed; 4] = [Speed::High, Speed::Normal, Speed::Low, Speed::Paused];

    /// `None` while paused.
    pub fn interval(self) -> Option<std::time::Duration> {
        let ms = match self {
            Speed::High => 500,
            Speed::Normal => 1000,
            Speed::Low => 4000,
            Speed::Paused => return None,
        };
        Some(std::time::Duration::from_millis(ms))
    }

    pub fn label(self) -> &'static str {
        match self {
            Speed::High => "High · 0.5 s",
            Speed::Normal => "Normal · 1 s",
            Speed::Low => "Low · 4 s",
            Speed::Paused => "Paused",
        }
    }
}

impl Settings {
    pub fn group_visible(&self, group: &Group) -> bool {
        if shown_by_default(group.kind) {
            !self.hidden_groups.contains(&group.id)
        } else {
            self.shown_groups.contains(&group.id)
        }
    }

    pub fn reading_visible(&self, group: &str, reading: &Reading) -> bool {
        let key = reading_key(group, reading.id);
        if reading.dashboard { !self.hidden_readings.contains(&key) } else { self.shown_readings.contains(&key) }
    }

    pub fn toggle_group(&mut self, group: &Group) {
        let overrides = if shown_by_default(group.kind) { &mut self.hidden_groups } else { &mut self.shown_groups };
        if !overrides.remove(&group.id) {
            overrides.insert(group.id.clone());
        }
    }

    pub fn toggle_reading(&mut self, group: &str, reading: &Reading) {
        let key = reading_key(group, reading.id);
        let overrides = if reading.dashboard { &mut self.hidden_readings } else { &mut self.shown_readings };
        if !overrides.remove(&key) {
            overrides.insert(key);
        }
    }

    pub fn visibility_is_default(&self) -> bool {
        self.hidden_groups.is_empty()
            && self.shown_groups.is_empty()
            && self.hidden_readings.is_empty()
            && self.shown_readings.is_empty()
    }

    /// Restores the default components and cards; order is kept.
    pub fn reset_visibility(&mut self) {
        self.hidden_groups.clear();
        self.shown_groups.clear();
        self.hidden_readings.clear();
        self.shown_readings.clear();
    }

    /// `groups` (in default order) rearranged by the custom order. Components missing
    /// from it (e.g. a drive plugged in for the first time) keep their default spot
    /// relative to their neighbours: right after the closest preceding known one.
    pub fn ordered<'a>(&self, groups: &'a [Group]) -> Vec<&'a Group> {
        let mut rank = -1.0f32;
        let mut ranked: Vec<(f32, &Group)> = groups
            .iter()
            .map(|g| {
                rank = match self.order.iter().position(|id| *id == g.id) {
                    Some(i) => i as f32,
                    None => rank + 0.001,
                };
                (rank, g)
            })
            .collect();
        ranked.sort_by(|a, b| a.0.total_cmp(&b.0));
        ranked.into_iter().map(|(_, g)| g).collect()
    }

    /// Moves the component at `from` to `to` (both indices into `ordered`, `to` being
    /// the slot it is dropped before) and stores the resulting order.
    pub fn reorder(&mut self, ordered: &[&Group], from: usize, to: usize) {
        let mut ids: Vec<String> = ordered.iter().map(|g| g.id.clone()).collect();
        let moved = ids.remove(from);
        ids.insert(if to > from { to - 1 } else { to }, moved);
        // Keep components that are not connected right now (an unplugged drive) at
        // their place: re-insert each right after the id that preceded it before.
        let previous = std::mem::take(&mut self.order);
        for (pos, id) in previous.iter().enumerate() {
            if ids.contains(id) {
                continue;
            }
            let after = previous[..pos].iter().rev().find_map(|p| ids.iter().position(|x| x == p));
            ids.insert(after.map_or(0, |i| i + 1), id.clone());
        }
        self.order = ids;
    }

    /// The groups as the dashboard should show them: custom order applied, hidden
    /// components and cards removed, components without any visible card dropped.
    pub fn dashboard_groups(&self, groups: &[Group]) -> Vec<Group> {
        self.ordered(groups)
            .into_iter()
            .filter(|g| self.group_visible(g))
            .map(|g| Group {
                id: g.id.clone(),
                kind: g.kind,
                order: g.order,
                title: g.title.clone(),
                subtitle: g.subtitle.clone(),
                readings: g.readings.iter().filter(|r| self.reading_visible(&g.id, r)).cloned().collect(),
                // Detail-page data is not needed on the dashboard.
                breakdown: None,
                info: Vec::new(),
                processes: None,
            })
            .filter(|g| !g.readings.is_empty())
            .collect()
    }
}

fn shown_by_default(kind: Kind) -> bool {
    matches!(kind, Kind::Gpu | Kind::Cpu | Kind::Memory | Kind::Battery)
}

fn reading_key(group: &str, reading: &str) -> String {
    format!("{group}/{reading}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn groups(ids: &[&str]) -> Vec<Group> {
        ids.iter()
            .map(|id| Group {
                id: id.to_string(),
                kind: match *id {
                    "gpu" => Kind::Gpu,
                    "mem" => Kind::Memory,
                    "net" => Kind::Network,
                    id if id.starts_with("disk") => Kind::Disk,
                    _ => Kind::Cpu,
                },
                order: 0,
                title: String::new(),
                subtitle: String::new(),
                readings: Vec::new(),
                breakdown: None,
                info: Vec::new(),
                processes: None,
            })
            .collect()
    }

    #[test]
    fn detail_readings_stay_off_the_dashboard_until_switched_on() {
        use crate::sensors::Unit;
        let mut g = groups(&["gpu"]);
        let load = Reading::new("load", "Load", 1.0, Unit::Percent).on_dashboard();
        let pcie = Reading::new("pcie_rx", "PCIe", 1.0, Unit::BytesPerSec);
        g[0].readings = vec![load.clone(), pcie.clone()];
        let shown = |s: &Settings| s.dashboard_groups(&g)[0].readings.iter().map(|r| r.id).collect::<Vec<_>>();

        let mut s = Settings::default();
        assert_eq!(shown(&s), ["load"]);
        s.toggle_reading("gpu", &pcie);
        assert_eq!(shown(&s), ["load", "pcie_rx"]);
        s.toggle_reading("gpu", &load);
        assert_eq!(shown(&s), ["pcie_rx"]);
        s.toggle_reading("gpu", &load);
        s.toggle_reading("gpu", &pcie);
        assert!(s.visibility_is_default());
    }

    fn ids(groups: &[&Group]) -> Vec<String> {
        groups.iter().map(|g| g.id.clone()).collect()
    }

    #[test]
    fn default_order_without_custom_order() {
        let g = groups(&["cpu", "gpu", "mem"]);
        assert_eq!(ids(&Settings::default().ordered(&g)), ["cpu", "gpu", "mem"]);
    }

    #[test]
    fn default_visibility_by_kind_and_toggling() {
        let g = groups(&["gpu", "cpu", "mem", "disk1", "net"]);
        let mut s = Settings::default();
        let visible = |s: &Settings| g.iter().filter(|g| s.group_visible(g)).map(|g| g.id.clone()).collect::<Vec<_>>();
        assert_eq!(visible(&s), ["gpu", "cpu", "mem"]);
        s.toggle_group(&g[3]); // show disk1
        s.toggle_group(&g[1]); // hide cpu
        assert_eq!(visible(&s), ["gpu", "mem", "disk1"]);
        assert!(!s.visibility_is_default());
        s.toggle_group(&g[3]);
        s.toggle_group(&g[1]);
        assert!(s.visibility_is_default());
    }

    #[test]
    fn reorder_moves_down_and_up() {
        let g = groups(&["cpu", "gpu", "mem", "net"]);
        let mut s = Settings::default();
        s.reorder(&s.ordered(&g), 0, 3); // cpu dropped before net
        assert_eq!(ids(&s.ordered(&g)), ["gpu", "mem", "cpu", "net"]);
        s.reorder(&s.ordered(&g), 3, 0); // net to the top
        assert_eq!(ids(&s.ordered(&g)), ["net", "gpu", "mem", "cpu"]);
    }

    #[test]
    fn new_component_appears_after_its_default_predecessor() {
        let mut s = Settings { order: vec!["mem".into(), "cpu".into(), "disk1".into()], ..Default::default() };
        // usb sits after disk1 in default order, so it follows disk1.
        let g = groups(&["cpu", "mem", "disk1", "usb"]);
        assert_eq!(ids(&s.ordered(&g)), ["mem", "cpu", "disk1", "usb"]);
        // A new component first in default order goes to the very top.
        s.order = vec!["mem".into(), "cpu".into()];
        let g = groups(&["new", "cpu", "mem"]);
        assert_eq!(ids(&s.ordered(&g)), ["new", "mem", "cpu"]);
    }

    #[test]
    fn unplugged_component_keeps_its_place_when_reordering() {
        let mut s =
            Settings { order: vec!["cpu".into(), "usb".into(), "disk1".into(), "net".into()], ..Default::default() };
        // usb is unplugged while net is moved to the top.
        let connected = groups(&["cpu", "disk1", "net"]);
        s.reorder(&s.ordered(&connected), 2, 0);
        assert_eq!(s.order, ["net", "cpu", "usb", "disk1"]);
    }
}
