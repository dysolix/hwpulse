//! Background thread that polls all providers on a fixed interval and wakes the UI.
//! Between ticks nothing runs: the thread sleeps and egui only repaints on request.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use eframe::egui;

use crate::sensors::{self, Group};

pub const INTERVAL: Duration = Duration::from_secs(1);
/// Samples kept per reading (2 minutes at 1 s).
pub const HISTORY_LEN: usize = 120;

#[derive(Default)]
pub struct Model {
    pub groups: Vec<Group>,
    pub errors: Vec<String>,
    /// Keyed by `"{group.id}/{reading.id}"`.
    pub history: HashMap<String, VecDeque<f32>>,
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
            model.lock().unwrap().errors = errors;

            loop {
                let groups: Vec<Group> = providers.iter_mut().flat_map(|p| p.poll()).collect();
                {
                    let mut m = model.lock().unwrap();
                    for g in &groups {
                        for r in &g.readings {
                            let h = m.history.entry(format!("{}/{}", g.id, r.id)).or_default();
                            if h.len() == HISTORY_LEN {
                                h.pop_front();
                            }
                            h.push_back(r.value as f32);
                        }
                    }
                    m.groups = groups;
                }
                ctx.request_repaint();
                thread::sleep(INTERVAL);
            }
        })
        .expect("failed to spawn poller thread");

    shared
}
