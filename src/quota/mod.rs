//! Provider quotas, fetched by ANVIL itself: no CLI has to run. A background
//! thread in every window observes the shared `quota.json`; the window that
//! holds `quota.lock` also polls every enabled provider every 30 seconds (or
//! on request) with the logins it finds on disk and the keys typed into ANVIL.
//! The UI thread only reads the published snapshot.

pub mod cache;
pub mod credman;
pub mod creds;
pub mod http;
pub mod model;
pub mod providers;
pub mod sqlite;
pub mod time;
pub mod view;
mod worker;

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

pub use cache::Paths;
pub use model::{ProviderId, Snapshot};
pub use worker::Prefs;

use crate::config::QuotaConfig;

pub(crate) struct Shared {
    stop: AtomicBool,
    version: AtomicU64,
    snapshot: Mutex<Snapshot>,
    /// Reads the provider switches from config.json at the start of every
    /// cycle, so a provider switched off in any window gets no request; `None`
    /// (the file cannot be read right now) skips the cycle.
    prefs: Box<dyn Fn() -> Option<Prefs> + Send + Sync>,
    repaint: Box<dyn Fn() + Send + Sync>,
    /// Unix deadline of the shared manual-refresh gap, observed by every window.
    manual_wait: AtomicU64,
}

impl Shared {
    fn publish(&self, snapshot: Snapshot) {
        *self.snapshot.lock().unwrap_or_else(PoisonError::into_inner) = snapshot;
        self.version.fetch_add(1, Ordering::Relaxed);
        (self.repaint)();
    }

    fn snapshot(&self) -> Snapshot {
        self.snapshot.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }

    fn prefs(&self) -> Option<Prefs> {
        (self.prefs)()
    }
}

/// The running quota worker of this window. Dropping it sets a flag the worker
/// checks between cycles, so the thread ends after the cycle in flight rather
/// than within a second — a request against a black-holed network still has to
/// run out its WinHTTP timeouts. The UI never waits for it.
pub struct QuotaHandle {
    shared: Arc<Shared>,
    paths: Paths,
}

impl QuotaHandle {
    /// Read-only snapshot for the development capture; no worker or credential mutation.
    #[cfg(debug_assertions)]
    pub fn read_only(snapshot: Snapshot, folder: &std::path::Path) -> Self {
        Self {
            paths: Paths::in_dir(folder),
            shared: Arc::new(Shared {
                stop: AtomicBool::new(false),
                version: AtomicU64::new(1),
                snapshot: Mutex::new(snapshot),
                prefs: Box::new(|| None),
                repaint: Box::new(|| {}),
                manual_wait: AtomicU64::new(0),
            }),
        }
    }

    pub fn start(
        paths: Paths,
        prefs: impl Fn() -> Option<Prefs> + Send + Sync + 'static,
        repaint: impl Fn() + Send + Sync + 'static,
    ) -> QuotaHandle {
        let shared = Arc::new(Shared {
            stop: AtomicBool::new(false),
            version: AtomicU64::new(0),
            snapshot: Mutex::new(cache::read(&paths.snapshot).unwrap_or_default()),
            prefs: Box::new(prefs),
            repaint: Box::new(repaint),
            manual_wait: AtomicU64::new(worker::manual_deadline(&paths.schedule).max(0) as u64),
        });
        let thread_shared = Arc::clone(&shared);
        let thread_paths = paths.clone();
        let user_agent = format!("ANVIL/{}", env!("CARGO_PKG_VERSION"));
        if let Err(e) = std::thread::Builder::new()
            .name("anvil-quota".into())
            .spawn(move || worker::run(thread_shared, thread_paths, user_agent))
        {
            log::warn!("quota: cannot start the worker: {e}");
        }
        QuotaHandle { shared, paths }
    }

    /// Bumped on every new snapshot, so the UI can skip unchanged frames.
    pub fn version(&self) -> u64 {
        self.shared.version.load(Ordering::Relaxed)
    }

    pub fn snapshot(&self) -> Snapshot {
        self.shared.snapshot()
    }

    /// Asks whichever window leads (this one or another) for a cycle now.
    /// Answers the seconds the request still has to wait: a request inside the
    /// anti-hammer gap is served later, and the caller says so rather than
    /// pretending the click did nothing.
    pub fn refresh(&self) -> i64 {
        let wait = self.manual_wait();
        if let Err(e) = cache::request_refresh(&self.paths.refresh) {
            log::warn!("quota: cannot request a refresh: {e}");
        }
        wait
    }

    fn manual_wait(&self) -> i64 {
        (self.shared.manual_wait.load(Ordering::Relaxed) as i64 - time::now_unix()).max(0)
    }
}

impl Drop for QuotaHandle {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Relaxed);
    }
}

pub fn prefs_from(config: &QuotaConfig) -> Prefs {
    ProviderId::ALL.into_iter().map(|id| (id, config.provider_enabled(id.key()))).collect()
}
