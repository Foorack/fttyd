//! Host statistics: sampled once for the whole process and fanned out to every
//! session, so the sidebar costs the same whether one person or twenty are connected.

use std::sync::Arc;
use std::time::Duration;

use axum::response::sse::{Event, Sse};
use serde::Serialize;
use sysinfo::{Disks, System};
use tokio::sync::watch;

/// One sample. Bytes, seconds and percent values, all as reported by the kernel.
#[derive(Debug, Clone, Serialize)]
pub struct Snapshot {
    /// Busy CPU percentage across all cores, 0-100.
    pub cpu: f32,
    pub cpu_count: usize,
    pub mem_total: u64,
    pub mem_used: u64,
    pub swap_total: u64,
    pub swap_used: u64,
    pub load_one: f64,
    pub load_five: f64,
    pub load_fifteen: f64,
    pub uptime: u64,
    pub disks: Vec<DiskUsage>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DiskUsage {
    pub mount: String,
    pub file_system: String,
    pub total: u64,
    pub available: u64,
}

pub struct Stats {
    sender: Arc<watch::Sender<Arc<Snapshot>>>,
}

impl Stats {
    /// Takes one sample. CPU percentages need two reads separated by an interval, so
    /// this deliberately reads twice; without that the first published sample would
    /// always claim 0% and look broken.
    pub fn new() -> Self {
        let mut system = System::new();
        system.refresh_cpu_usage();
        system.refresh_memory();
        std::thread::sleep(Duration::from_millis(200));
        let disks = Disks::new_with_refreshed_list();
        let (sender, _) = watch::channel(Arc::new(sample(&mut system, &disks)));
        Self { sender: Arc::new(sender) }
    }

    /// Starts the background sampler. Call once, from a running runtime.
    pub fn spawn(self, interval: Duration) -> Self {
        let sender = self.sender.clone();
        tokio::spawn(async move {
            let mut system = System::new();
            let mut disks = Disks::new_with_refreshed_list();
            system.refresh_cpu_usage();
            system.refresh_memory();
            let mut ticker = tokio::time::interval(interval);
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                ticker.tick().await;
                system.refresh_cpu_usage();
                system.refresh_memory();
                disks.refresh(true);
                // A send error only means nobody is watching right now.
                let _ = sender.send(Arc::new(sample(&mut system, &disks)));
            }
        });
        self
    }

    pub fn subscribe(&self) -> watch::Receiver<Arc<Snapshot>> {
        self.sender.subscribe()
    }
}

impl Default for Stats {
    fn default() -> Self {
        Self::new()
    }
}

fn sample(system: &mut System, disks: &Disks) -> Snapshot {
    let load = System::load_average();
    Snapshot {
        cpu: system.global_cpu_usage(),
        cpu_count: system.cpus().len(),
        mem_total: system.total_memory(),
        mem_used: system.used_memory(),
        swap_total: system.total_swap(),
        swap_used: system.used_swap(),
        load_one: load.one,
        load_five: load.five,
        load_fifteen: load.fifteen,
        uptime: System::uptime(),
        disks: disks
            .iter()
            .map(|disk| DiskUsage {
                mount: disk.mount_point().to_string_lossy().into_owned(),
                file_system: disk.file_system().to_string_lossy().into_owned(),
                total: disk.total_space(),
                available: disk.available_space(),
            })
            .collect(),
    }
}

/// `GET /api/stats` — one event per sample, newest value first.
pub fn stream(
    receiver: watch::Receiver<Arc<Snapshot>>,
) -> Sse<impl futures_util::Stream<Item = Result<Event, std::convert::Infallible>>> {
    let pending = Some(receiver.borrow().clone());

    let stream = futures_util::stream::unfold((receiver, pending), |(mut receiver, pending)| async move {
        match pending {
            Some(snapshot) => Some((Ok(event(&snapshot)), (receiver, None))),
            None => {
                receiver.changed().await.ok()?;
                let snapshot = receiver.borrow_and_update().clone();
                Some((Ok(event(&snapshot)), (receiver, None)))
            }
        }
    });

    Sse::new(stream).keep_alive(axum::response::sse::KeepAlive::default())
}

fn event(snapshot: &Snapshot) -> Event {
    Event::default().event("stats").data(serde_json::to_string(snapshot).unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn samples_report_plausible_values() {
        let stats = Stats::new();
        let snapshot = stats.subscribe().borrow().clone();
        assert!(snapshot.cpu >= 0.0 && snapshot.cpu <= 100.0, "cpu: {}", snapshot.cpu);
        assert!(snapshot.cpu_count >= 1);
        assert!(snapshot.mem_total > 0);
        assert!(snapshot.mem_used <= snapshot.mem_total);
        assert!(!snapshot.disks.is_empty(), "at least the root filesystem is listed");
    }

    #[tokio::test]
    async fn subscribers_receive_updates() {
        let stats = Stats::new().spawn(Duration::from_millis(20));
        let mut receiver = stats.subscribe();
        receiver.borrow_and_update();
        tokio::time::timeout(Duration::from_secs(2), receiver.changed())
            .await
            .expect("a new sample should arrive")
            .expect("channel stays open");
    }

    #[tokio::test]
    async fn snapshot_serialises_with_expected_fields() {
        let stats = Stats::new();
        let snapshot = stats.subscribe().borrow().clone();
        let json = serde_json::to_value(&*snapshot).unwrap();
        for field in ["cpu", "cpu_count", "mem_total", "mem_used", "disks", "uptime", "load_one"] {
            assert!(json.get(field).is_some(), "missing {field} in {json}");
        }
    }
}
