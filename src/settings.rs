//! User choices, persisted by eframe (`%APPDATA%\hwmon\data\app.ron`).
//!
//! Hidden items are stored rather than shown ones, so newly detected components
//! (a plugged-in drive, a new GPU) appear on the dashboard by default.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::sensors::Group;

pub const STORAGE_KEY: &str = "settings";

#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// [`Group::id`]s hidden from the dashboard.
    pub hidden_groups: BTreeSet<String>,
    /// `"{group id}/{reading id}"` of individual cards hidden from the dashboard.
    pub hidden_readings: BTreeSet<String>,
}

impl Settings {
    pub fn group_visible(&self, group: &str) -> bool {
        !self.hidden_groups.contains(group)
    }

    pub fn reading_visible(&self, group: &str, reading: &str) -> bool {
        !self.hidden_readings.contains(&reading_key(group, reading))
    }

    pub fn toggle_group(&mut self, group: &str) {
        if !self.hidden_groups.remove(group) {
            self.hidden_groups.insert(group.to_string());
        }
    }

    pub fn toggle_reading(&mut self, group: &str, reading: &str) {
        let key = reading_key(group, reading);
        if !self.hidden_readings.remove(&key) {
            self.hidden_readings.insert(key);
        }
    }

    pub fn dashboard_is_default(&self) -> bool {
        self.hidden_groups.is_empty() && self.hidden_readings.is_empty()
    }

    /// Shows every component and card again; other settings are kept.
    pub fn reset_dashboard(&mut self) {
        self.hidden_groups.clear();
        self.hidden_readings.clear();
    }

    /// The groups as the dashboard should show them: hidden components and cards
    /// removed, components without any visible card dropped.
    pub fn dashboard_groups(&self, groups: &[Group]) -> Vec<Group> {
        groups
            .iter()
            .filter(|g| self.group_visible(&g.id))
            .map(|g| {
                let mut g = g.clone();
                let id = g.id.clone();
                g.readings.retain(|r| self.reading_visible(&id, r.id));
                g
            })
            .filter(|g| !g.readings.is_empty())
            .collect()
    }
}

fn reading_key(group: &str, reading: &str) -> String {
    format!("{group}/{reading}")
}
