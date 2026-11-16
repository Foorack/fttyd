//! Shared server state.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use tokio::sync::Notify;

use crate::cli::Config;
use crate::stats::Stats;

pub struct AppState {
    pub cfg: Arc<Config>,
    pub stats: Stats,
    clients: AtomicUsize,
    /// Signalled when `--once`/`--exit-no-conn` want the server to stop.
    pub shutdown: Notify,
}

impl AppState {
    pub fn new(cfg: Config, stats: Stats) -> Arc<Self> {
        Arc::new(Self {
            cfg: Arc::new(cfg),
            stats,
            clients: AtomicUsize::new(0),
            shutdown: Notify::new(),
        })
    }

    pub fn client_count(&self) -> usize {
        self.clients.load(Ordering::SeqCst)
    }

    /// Whether one more client may attach, honouring `--once` and `--max-clients`.
    pub fn can_accept_client(&self) -> bool {
        let current = self.client_count();
        if self.cfg.once && current > 0 {
            return false;
        }
        self.cfg.max_clients == 0 || current < self.cfg.max_clients
    }

    fn add_client(&self) {
        self.clients.fetch_add(1, Ordering::SeqCst);
    }

    fn remove_client(&self) {
        let remaining = self.clients.fetch_sub(1, Ordering::SeqCst).saturating_sub(1);
        if remaining == 0 && (self.cfg.once || self.cfg.exit_no_conn) {
            self.shutdown.notify_one();
        }
    }
}

/// Counts a connected client for as long as the session lives.
pub struct ClientGuard {
    state: Arc<AppState>,
}

impl ClientGuard {
    pub fn new(state: Arc<AppState>) -> Self {
        state.add_client();
        Self { state }
    }
}

impl Drop for ClientGuard {
    fn drop(&mut self) {
        self.state.remove_client();
    }
}

/// The identity a reverse proxy asserted through `-H`, if any.
#[derive(Debug, Clone, Default)]
pub struct AuthUser(pub Option<String>);

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> Config {
        Config { command: vec!["bash".into()], ..Config::default() }
    }

    #[test]
    fn counts_clients() {
        let state = AppState::new(cfg(), Stats::new());
        assert_eq!(state.client_count(), 0);
        {
            let _first = ClientGuard::new(state.clone());
            assert_eq!(state.client_count(), 1);
            let _second = ClientGuard::new(state.clone());
            assert_eq!(state.client_count(), 2);
        }
        assert_eq!(state.client_count(), 0);
    }

    #[test]
    fn max_clients_is_enforced() {
        let state = AppState::new(Config { max_clients: 1, ..cfg() }, Stats::new());
        assert!(state.can_accept_client());
        let _guard = ClientGuard::new(state.clone());
        assert!(!state.can_accept_client());
    }

    #[test]
    fn once_refuses_a_second_client() {
        let state = AppState::new(Config { once: true, ..cfg() }, Stats::new());
        assert!(state.can_accept_client());
        let _guard = ClientGuard::new(state.clone());
        assert!(!state.can_accept_client());
    }

    #[test]
    fn unlimited_by_default() {
        let state = AppState::new(cfg(), Stats::new());
        let _guards: Vec<ClientGuard> =
            (0..10).map(|_| ClientGuard::new(state.clone())).collect();
        assert!(state.can_accept_client());
        assert_eq!(state.client_count(), 10);
    }
}
