//! Background thread that polls all providers on a fixed interval and wakes the UI.
//! Between ticks nothing runs: the thread sleeps and egui only repaints on request.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

use eframe::egui;

use crate::sensors::{self, Group, ProcessRow};

/// Time between samples, set from the "Update speed" setting.
static INTERVAL_MS: AtomicU64 = AtomicU64::new(1000);
static PAUSED: AtomicBool = AtomicBool::new(false);

/// The time between samples; while paused, the one used before.
pub fn interval() -> Duration {
    Duration::from_millis(INTERVAL_MS.load(Ordering::Relaxed))
}

pub fn paused() -> bool {
    PAUSED.load(Ordering::Relaxed)
}

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
    /// Set by the UI while the Processes page is open; only then is the full list built.
    pub want_processes: bool,
    /// Every running process, while `want_processes` is set.
    pub processes: Option<Vec<ProcessRow>>,
}

/// The model plus a way to wake the poller between ticks, and the UI on new samples.
#[derive(Clone, Default)]
pub struct Shared {
    model: Arc<Mutex<Model>>,
    wake: Arc<Condvar>,
    ui: Arc<OnceLock<egui::Context>>,
}

impl Shared {
    pub fn lock(&self) -> MutexGuard<'_, Model> {
        self.model.lock().unwrap()
    }

    /// Lets new samples repaint the window; until then they wait for its first frame.
    pub fn attach(&self, ctx: &egui::Context) {
        let _ = self.ui.set(ctx.clone());
    }

    /// A new update interval, or `None` to pause; takes effect right away.
    pub fn set_interval(&self, interval: Option<Duration>) {
        if let Some(i) = interval {
            INTERVAL_MS.store(i.as_millis() as u64, Ordering::Relaxed);
        }
        PAUSED.store(interval.is_none(), Ordering::Relaxed);
        let _guard = self.model.lock();
        self.wake.notify_one();
    }

    /// Builds the process list right away instead of at the next tick (or releases it
    /// right away when no longer shown); call after setting [`Model::want_processes`].
    pub fn wake(&self) {
        self.wake.notify_one();
    }
}

/// Starts before the window exists, so the slow sensor setup (PDH alone takes ~0.3 s)
/// overlaps with creating it.
pub fn spawn() -> Shared {
    let shared = Shared::default();
    let (model, wake, ui) = (shared.model.clone(), shared.wake.clone(), shared.ui.clone());
    let repaint = move || {
        if let Some(ctx) = ui.get() {
            ctx.request_repaint();
        }
    };

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

            let mut listing = false;
            loop {
                let mut groups: Vec<Group> = providers.iter_mut().flat_map(|p| p.poll()).collect();
                groups.sort_by_key(|g| g.order);
                let want_processes = model.lock().unwrap().want_processes;
                let processes = want_processes.then(|| process_table(&mut providers));
                if listing && !want_processes {
                    providers.iter_mut().for_each(|p| p.close_process_table());
                }
                listing = want_processes;
                {
                    let mut m = model.lock().unwrap();
                    m.processes = processes;
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
                repaint();

                // Sleep until the next tick (or while paused). When the Processes page
                // opens meanwhile, build its list from the latest sample right away; when
                // the update speed changes, wait for the new interval instead.
                let ticked = Instant::now();
                let mut m = model.lock().unwrap();
                loop {
                    let wanted = |m: &mut Model| m.want_processes && m.processes.is_none();
                    let (ms, was_paused) = (INTERVAL_MS.load(Ordering::Relaxed), paused());
                    let timeout = if was_paused {
                        Duration::from_secs(3600)
                    } else {
                        (ticked + Duration::from_millis(ms)).saturating_duration_since(Instant::now())
                    };
                    let unchanged = || INTERVAL_MS.load(Ordering::Relaxed) == ms && paused() == was_paused;
                    let closed = |m: &mut Model| listing && !m.want_processes;
                    let (guard, result) =
                        wake.wait_timeout_while(m, timeout, |m| !wanted(m) && !closed(m) && unchanged()).unwrap();
                    m = guard;
                    if closed(&mut m) {
                        providers.iter_mut().for_each(|p| p.close_process_table());
                        listing = false;
                    } else if wanted(&mut m) {
                        drop(m);
                        let table = process_table(&mut providers);
                        listing = true;
                        m = model.lock().unwrap();
                        m.processes = Some(table);
                        repaint();
                    } else if result.timed_out() && !was_paused {
                        break;
                    }
                }
            }
        })
        .expect("failed to spawn poller thread");

    shared
}

fn process_table(providers: &mut [Box<dyn sensors::SensorProvider>]) -> Vec<ProcessRow> {
    providers.iter_mut().find_map(|p| p.process_table()).unwrap_or_default()
}
