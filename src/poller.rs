//! Background thread that polls all providers on a fixed interval and wakes the UI.
//! Between ticks nothing runs: the thread sleeps and egui only repaints on request.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use eframe::egui;

use crate::sensors::{self, Group};

pub const INTERVAL: Duration = Duration::from_secs(1);
/// Samples shown per reading (2 minutes at 1 s). One more is kept, so the oldest
/// can slide out of the charts while a new one slides in.
pub const HISTORY_LEN: usize = 120;

#[derive(Default)]
pub struct Model {
    pub groups: Vec<Group>,
    pub errors: Vec<String>,
    /// E.g. the mainboard name, shown under the app title.
    pub machine: Option<String>,
    /// Keyed by `"{group.id}/{reading.id}"`.
    pub history: HashMap<String, VecDeque<f32>>,
    /// When the latest samples arrived, for animating the charts in between.
    pub updated: Option<Instant>,
}

pub type Shared = Arc<Mutex<Model>>;

pub fn spawn(ctx: egui::Context) -> Shared {
    let shared: Shared = Arc::default();
    let model = shared.clone();

    thread::Builder::new()
        .name("sensor-poller".into())
        .spawn(move || {
            // Providers are created on this thread so they never need to be `Send`.
            let (mut providers, errors) = sensors::create_providers();
            {
                let mut m = model.lock().unwrap();
                m.errors = errors;
                m.machine = providers.iter().find_map(|p| p.machine());
            }

            loop {
                let mut groups: Vec<Group> = providers.iter_mut().flat_map(|p| p.poll()).collect();
                groups.sort_by_key(|g| g.order);
                {
                    let mut m = model.lock().unwrap();
                    let samples = groups.iter().flat_map(|g| {
                        let readings = g.readings.iter().map(|r| (format!("{}/{}", g.id, r.id), r.value));
                        let items = g.breakdown.iter().flat_map(|b| &b.items);
                        readings.chain(items.map(|i| (format!("{}/#{}", g.id, i.label), i.value)))
                    });

                    let mut seen = HashSet::new();
                    for (key, value) in samples {
                        let h = m.history.entry(key.clone()).or_default();
                        if h.len() > HISTORY_LEN {
                            h.pop_front();
                        }
                        h.push_back(value as f32);
                        seen.insert(key);
                    }
                    // Forget components that stopped reporting (e.g. an unplugged drive),
                    // so a reconnect starts a fresh graph instead of one with a hidden gap.
                    m.history.retain(|key, _| seen.contains(key));
                    m.groups = groups;
                    m.updated = Some(Instant::now());
                }
                ctx.request_repaint();
                thread::sleep(INTERVAL);
            }
        })
        .expect("failed to spawn poller thread");

    shared
}
