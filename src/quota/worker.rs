//! The quota cycle. `Engine` decides, provider by provider, whether to ask
//! the network and what the snapshot says afterwards; it is pure apart from
//! the closures it is given, so the scheduling rules are tested without
//! threads, files or sockets. `run` is the thin thread loop around it.

use std::collections::HashMap;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use super::cache::{self, Paths};
use super::creds::{self, CredEnv, Credential, Detection};
use super::http::{Http, WinHttp};
use super::model::{FetchError, Fetched, ProviderId, ProviderSnapshot, ProviderState, Snapshot};
use super::sqlite::Sqlite;
use super::time::now_unix;
use super::{credman, providers, Shared};

/// Seconds between cycles.
pub const INTERVAL: i64 = 300;
/// A manual refresh is honoured at most this often.
pub const MANUAL_GAP: i64 = 30;
/// An observing window retries the lock this often.
const LEAD_RETRY: i64 = 30;
/// After a network failure (often: just woke from sleep, network not up yet)
/// the next cycle comes this soon instead of a full interval later.
pub const QUICK_RETRY: i64 = 60;

#[derive(Default, serde::Serialize, serde::Deserialize)]
struct Memory {
    /// 429s in a row.
    failures: u32,
    paused_until: i64,
    /// The login that was refused (401/403); not retried until it changes.
    refused: Option<u64>,
}

/// `Some(true)`/`Some(false)`: switched on/off by the user; `None`: automatic.
pub type Prefs = HashMap<ProviderId, Option<bool>>;

#[derive(Default, serde::Serialize, serde::Deserialize)]
pub struct Engine {
    memory: HashMap<ProviderId, Memory>,
    last_cycle: Option<i64>,
    last_manual: Option<i64>,
    /// A refresh was asked for and no cycle has served it yet; one asked
    /// inside the gap waits for it to pass instead of being dropped.
    manual_pending: bool,
    retry_at: Option<i64>,
    /// The snapshot built so far, so a panic part-way through a cycle still
    /// leaves the providers that answered on screen.
    #[serde(skip)]
    partial: Option<Snapshot>,
}

impl Engine {
    fn read(path: &std::path::Path) -> Option<Self> {
        let bytes = crate::fsutil::read_limited(path, 64 * 1024).ok()?;
        let mut engine: Self = serde_json::from_slice(&bytes).ok()?;
        // Hand-edited metadata and a backwards clock jump must not overflow
        // deadline arithmetic or disable polling indefinitely.
        let now = now_unix();
        engine.last_cycle = engine.last_cycle.map(|at| at.clamp(0, now));
        engine.last_manual = engine.last_manual.map(|at| at.clamp(0, now));
        engine.retry_at = engine.retry_at.map(|at| at.clamp(0, now + QUICK_RETRY));
        for memory in engine.memory.values_mut() {
            memory.paused_until = memory.paused_until.clamp(0, now + super::model::MAX_PAUSE);
        }
        Some(engine)
    }

    fn save(&self, path: &std::path::Path) {
        let result = serde_json::to_vec(self)
            .map_err(std::io::Error::other)
            .and_then(|bytes| crate::fsutil::atomic_write(path, &bytes));
        if let Err(error) = result {
            log::warn!("quota: cannot save schedule: {error}");
        }
    }

    fn begin_cycle(&mut self, now: i64) {
        self.last_cycle = Some(now);
        self.retry_at = None;
        if self.manual_pending {
            self.manual_pending = false;
            self.last_manual = Some(now);
        }
    }

    fn manual_deadline(&self) -> i64 {
        self.last_manual.map_or(0, |at| at.saturating_add(MANUAL_GAP))
    }
}

/// Scheduling metadata contains only timestamps, counters and token-change
/// fingerprints, never credentials. Observers need it even before a fetch ends.
pub(super) fn manual_deadline(path: &std::path::Path) -> i64 {
    Engine::read(path).map_or(0, |engine| engine.manual_deadline())
}

impl Engine {
    /// Seconds a manual refresh asked for inside the gap still has to wait.
    /// The gap exists so a held-down button cannot hammer the providers, but a
    /// click that is going to be deferred has to say so instead of looking like
    /// a button that did nothing.
    #[cfg(test)]
    pub fn manual_wait(&self, now: i64) -> i64 {
        match self.last_manual {
            Some(at) => (MANUAL_GAP - (now - at)).max(0),
            None => 0,
        }
    }

    /// Whether a cycle should run now. `snapshot_age` is the age of the newest
    /// data any window wrote (None: there is none), so a window that becomes
    /// leader does not re-poll what another one fetched a minute ago.
    pub fn due(&mut self, now: i64, snapshot_age: Option<i64>, manual: bool) -> bool {
        self.manual_pending |= manual;
        if self.manual_pending && self.last_manual.is_none_or(|at| now - at >= MANUAL_GAP) {
            return true;
        }
        if self.retry_at.is_some_and(|at| now >= at) {
            return true;
        }
        match self.last_cycle {
            Some(at) => now - at >= INTERVAL,
            None => snapshot_age.is_none_or(|age| age >= INTERVAL),
        }
    }

    pub fn cycle(
        &mut self,
        now: i64,
        prefs: &Prefs,
        previous: &Snapshot,
        detect: impl Fn(ProviderId) -> Detection,
        fetch: impl Fn(ProviderId, &Credential) -> Result<Fetched, FetchError>,
    ) -> Snapshot {
        self.begin_cycle(now);
        self.partial = Some(Snapshot::default());
        for id in ProviderId::ALL {
            let entry = self.cycle_one(id, now, prefs, previous, &detect, &fetch);
            if let Some(entry) = entry {
                if let Some(partial) = &mut self.partial {
                    partial.providers.push(entry);
                }
            }
        }
        self.partial.take().unwrap_or_default()
    }

    /// The snapshot built so far, if a panic cut the cycle short.
    fn take_partial(&mut self) -> Option<Snapshot> {
        self.partial.take()
    }

    /// One provider's entry, or None when it is not shown at all.
    fn cycle_one(
        &mut self,
        id: ProviderId,
        now: i64,
        prefs: &Prefs,
        previous: &Snapshot,
        detect: &impl Fn(ProviderId) -> Detection,
        fetch: &impl Fn(ProviderId, &Credential) -> Result<Fetched, FetchError>,
    ) -> Option<ProviderSnapshot> {
        {
            let old = previous.get(id);
            let keep = |state: ProviderState, source: String, plan: Option<String>| ProviderSnapshot {
                id,
                plan: plan.or_else(|| old.and_then(|o| o.plan.clone())),
                source,
                state,
                windows: old.map(|o| o.windows.clone()).unwrap_or_default(),
                balances: old.map(|o| o.balances.clone()).unwrap_or_default(),
                fetched_at: old.and_then(|o| o.fetched_at),
                checked_at: now,
            };
            // The cache already carries Retry-After. A new leader or a restart
            // must honour it rather than immediately buying another 429.
            let memory = self.memory.entry(id).or_insert_with(|| Memory {
                paused_until: old
                    .and_then(|o| match o.state {
                        ProviderState::RateLimited { retry_at } => Some(retry_at.min(now + super::model::MAX_PAUSE)),
                        _ => None,
                    })
                    .unwrap_or(0),
                ..Memory::default()
            });
            let enabled = prefs.get(&id).copied().flatten().unwrap_or(true);
            let entry = match detect(id) {
                Detection::Missing => {
                    *memory = Memory::default();
                    None
                }
                Detection::Unreadable => {
                    if enabled {
                        self.retry_at = Some(now + QUICK_RETRY);
                    }
                    let state = if memory.paused_until > now {
                        ProviderState::RateLimited { retry_at: memory.paused_until }
                    } else {
                        ProviderState::StoreUnreadable
                    };
                    Some(keep(state, old.map(|o| o.source.clone()).unwrap_or_default(), None))
                }
                Detection::Expired { source, .. } => Some(keep(ProviderState::AuthExpired, source.label(), None)),
                Detection::Found(credential) => {
                    let source = credential.source.label();
                    let previous_state = old.map(|o| o.state.clone()).unwrap_or(ProviderState::Idle);
                    if memory.refused.is_some_and(|m| m != credential.marker) {
                        memory.refused = None;
                    }
                    if !enabled {
                        Some(keep(previous_state, source, credential.plan))
                    } else if memory.refused.is_some() {
                        Some(keep(ProviderState::AuthExpired, source, credential.plan))
                    } else if memory.paused_until > now {
                        Some(keep(
                            ProviderState::RateLimited { retry_at: memory.paused_until },
                            source,
                            credential.plan,
                        ))
                    } else {
                        match fetch(id, &credential) {
                            Ok(fetched) => {
                                memory.failures = 0;
                                memory.paused_until = 0;
                                let fetched = fetched.redacted(credential.secret.expose());
                                Some(ProviderSnapshot {
                                    id,
                                    plan: fetched.plan.or(credential.plan),
                                    source,
                                    state: ProviderState::Ok,
                                    windows: fetched.windows,
                                    balances: fetched.balances,
                                    fetched_at: Some(now),
                                    checked_at: now,
                                })
                            }
                            Err(error) => {
                                if matches!(error, FetchError::Network(_)) {
                                    self.retry_at = Some(now + QUICK_RETRY);
                                }
                                // A provider's own error text is written to disk
                                // and drawn in the GUI: a compromised or echoing
                                // endpoint must not get the key onto either.
                                let error = error.redacted(credential.secret.expose());
                                let state = error.into_state(now, memory.failures);
                                match &state {
                                    ProviderState::AuthExpired => memory.refused = Some(credential.marker),
                                    ProviderState::RateLimited { retry_at } => {
                                        memory.failures = memory.failures.saturating_add(1);
                                        memory.paused_until = *retry_at;
                                    }
                                    _ => memory.failures = 0,
                                }
                                Some(keep(state, source, credential.plan))
                            }
                        }
                    }
                }
            };
            entry
        }
    }
}

/// Keys typed into ANVIL, read fresh every cycle (six local calls).
fn own_keys() -> HashMap<ProviderId, String> {
    ProviderId::ALL
        .into_iter()
        .filter(|id| id.accepts_own_key())
        .filter_map(|id| credman::read(&credman::target(id)).map(|key| (id, key)))
        .collect()
}

/// The thread body: observe `quota.json`, lead when the lock is free, poll on
/// schedule. `Shared::stop` is checked between cycles, so a request in flight
/// finishes first, bounded by the WinHTTP timeouts.
pub(super) fn run(shared: Arc<Shared>, paths: Paths, user_agent: String) {
    let sqlite = Sqlite::load().map(Arc::new);
    if sqlite.is_none() {
        log::info!("quota: winsqlite3.dll unavailable; OMP and OpenCode 2 logins are skipped");
    }
    let mut engine = Engine::default();
    let mut leader = None;
    let mut http: Option<WinHttp> = None;
    let mut last_lead_try: Option<i64> = None;
    let mut seen_snapshot = None;
    let mut seen_refresh = cache::mtime(&paths.refresh);
    while !shared.stop.load(Ordering::Relaxed) {
        let now = now_unix();
        let snapshot_mtime = cache::mtime(&paths.snapshot);
        if snapshot_mtime != seen_snapshot {
            seen_snapshot = snapshot_mtime;
            if let Some(snapshot) = cache::read(&paths.snapshot) {
                shared.publish(snapshot);
            }
        }
        if leader.is_none() && last_lead_try.is_none_or(|at| now - at >= LEAD_RETRY) {
            last_lead_try = Some(now);
            leader = cache::try_lead(&paths.lock);
            if leader.is_some() {
                engine = Engine::read(&paths.schedule).unwrap_or_default();
            }
        }
        let refresh_mtime = cache::mtime(&paths.refresh);
        let manual = refresh_mtime != seen_refresh;
        seen_refresh = refresh_mtime;
        if leader.is_some() {
            let age = shared.snapshot().checked_at().map(|at| now - at);
            let due = engine.due(now, age, manual);
            if manual {
                engine.save(&paths.schedule); // Keep a deferred request across leadership changes.
            }
            if due {
                if http.is_none() {
                    http = WinHttp::new(&user_agent).map_err(|e| log::warn!("quota: WinHTTP: {e}")).ok();
                }
                if let (Some(http), Some(prefs)) = (&http, shared.prefs()) {
                    engine.begin_cycle(now);
                    engine.save(&paths.schedule); // Publish the gap before any blocking HTTP call.
                    shared.manual_wait.store(engine.manual_deadline().max(0) as u64, Ordering::Relaxed);
                    let previous = shared.snapshot();
                    let env = CredEnv::from_process(now, sqlite.clone(), own_keys());
                    let cycle = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        engine.cycle(
                            now,
                            &prefs,
                            &previous,
                            |id| creds::detect(id, &env),
                            |id, credential| providers::fetch(id, http as &dyn Http, credential, now),
                        )
                    }));
                    match cycle {
                        Ok(next) => {
                            if let Err(e) = cache::write(&paths.snapshot, &next) {
                                log::warn!("quota: cannot write {}: {e}", paths.snapshot.display());
                            }
                            seen_snapshot = cache::mtime(&paths.snapshot);
                            shared.publish(next);
                        }
                        Err(_) => {
                            // A panic must not cost every provider its snapshot
                            // and a five-minute wait: serve what did complete.
                            log::error!("quota: a cycle panicked; the worker continues");
                            if let Some(partial) = engine.take_partial() {
                                if let Err(e) = cache::write(&paths.snapshot, &partial) {
                                    log::warn!("quota: cannot write {}: {e}", paths.snapshot.display());
                                }
                                seen_snapshot = cache::mtime(&paths.snapshot);
                                shared.publish(partial);
                            }
                        }
                    }
                    engine.save(&paths.schedule);
                }
            }
        }
        let deadline = if leader.is_some() { engine.manual_deadline() } else { manual_deadline(&paths.schedule) };
        shared.manual_wait.store(deadline.max(0) as u64, Ordering::Relaxed);
        std::thread::sleep(Duration::from_secs(1));
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use super::*;
    use crate::quota::creds::{Secret, Source};
    use crate::quota::model::Window;

    fn login(marker: u64) -> Detection {
        Detection::Found(Credential {
            secret: Secret::new("k"),
            source: Source::AnvilKey,
            plan: None,
            account: None,
            org: None,
            server: None,
            expires_at: None,
            marker,
        })
    }

    #[test]
    fn corrupt_schedule_timestamps_cannot_overflow_or_postpone_cycles_forever() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("schedule.json");
        let engine = Engine {
            last_cycle: Some(i64::MIN),
            last_manual: Some(i64::MAX),
            retry_at: Some(i64::MAX),
            memory: HashMap::from([(ProviderId::Zai, Memory { paused_until: i64::MAX, ..Default::default() })]),
            ..Default::default()
        };
        engine.save(&path);
        let mut loaded = Engine::read(&path).unwrap();
        let now = now_unix();
        assert!(loaded.due(now, None, false));
        assert!(loaded.manual_deadline() <= now + MANUAL_GAP);
        assert!(loaded.memory[&ProviderId::Zai].paused_until <= now + crate::quota::model::MAX_PAUSE);
    }

    #[test]
    fn scheduling_survives_restart_and_observers_see_the_same_gap() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::in_dir(dir.path());
        let mut engine = Engine::default();
        assert!(engine.due(1000, None, true));
        let first =
            engine.cycle(1000, &Prefs::new(), &Snapshot::default(), only(ProviderId::Zai, || login(1)), |_, _| {
                Err(FetchError::Status { code: 401, retry_after: None })
            });
        engine.save(&paths.schedule);
        assert_eq!(manual_deadline(&paths.schedule), 1030, "another window reads the leader's gap");
        let mut restarted = Engine::read(&paths.schedule).unwrap();
        assert_eq!(restarted.manual_wait(1010), 20);
        restarted.cycle(1300, &Prefs::new(), &first, only(ProviderId::Zai, || login(1)), |_, _| {
            panic!("refused login was retried after restart");
        });
        let changed = restarted.cycle(1301, &Prefs::new(), &first, only(ProviderId::Zai, || login(2)), |_, _| ok());
        assert_eq!(changed.get(ProviderId::Zai).unwrap().state, ProviderState::Ok);
        let limited = restarted.cycle(1400, &Prefs::new(), &changed, only(ProviderId::Zai, || login(2)), |_, _| {
            Err(FetchError::Status { code: 429, retry_after: None })
        });
        restarted.save(&paths.schedule);
        let mut restarted = Engine::read(&paths.schedule).unwrap();
        let twice = restarted.cycle(1460, &Prefs::new(), &limited, only(ProviderId::Zai, || login(2)), |_, _| {
            Err(FetchError::Status { code: 429, retry_after: None })
        });
        assert_eq!(twice.get(ProviderId::Zai).unwrap().state, ProviderState::RateLimited { retry_at: 1580 });
        assert!(!std::fs::read_to_string(&paths.schedule).unwrap().contains("secret"));
    }

    fn only(id: ProviderId, detection: impl Fn() -> Detection) -> impl Fn(ProviderId) -> Detection {
        move |asked| if asked == id { detection() } else { Detection::Missing }
    }

    fn ok() -> Result<Fetched, FetchError> {
        Ok(Fetched::windows(Some("Pro".into()), vec![Window::new("5h", "5ч", 10.0, None)]))
    }

    #[test]
    fn first_cycle_waits_for_stale_data_then_follows_the_interval() {
        let mut engine = Engine::default();
        assert!(engine.due(1_000, None, false), "no data at all");
        let mut engine = Engine::default();
        assert!(!engine.due(1_000, Some(60), false), "another window fetched a minute ago");
        assert!(engine.due(1_000, Some(INTERVAL), false));
        engine.cycle(1_000, &Prefs::new(), &Snapshot::default(), |_| Detection::Missing, |_, _| ok());
        assert!(!engine.due(1_000 + INTERVAL - 1, None, false));
        assert!(engine.due(1_000 + INTERVAL, None, false));
    }

    #[test]
    fn a_network_failure_brings_the_next_cycle_closer() {
        let mut engine = Engine::default();
        let down = |_: ProviderId, _: &Credential| Err(FetchError::Network("no route".into()));
        engine.cycle(1_000, &Prefs::new(), &Snapshot::default(), only(ProviderId::Zai, || login(1)), down);
        assert!(!engine.due(1_000 + QUICK_RETRY - 1, None, false));
        assert!(engine.due(1_000 + QUICK_RETRY, None, false));
        engine.cycle(
            1_000 + QUICK_RETRY,
            &Prefs::new(),
            &Snapshot::default(),
            only(ProviderId::Zai, || login(1)),
            |_, _| ok(),
        );
        assert!(!engine.due(1_000 + 2 * QUICK_RETRY, None, false), "back to the normal interval");
    }

    fn run(engine: &mut Engine, now: i64) {
        engine.cycle(now, &Prefs::new(), &Snapshot::default(), |_| Detection::Missing, |_, _| ok());
    }

    #[test]
    fn manual_refreshes_are_rate_limited() {
        let mut engine = Engine::default();
        run(&mut engine, 1_000);
        assert!(engine.due(1_010, None, true));
        run(&mut engine, 1_010);
        assert!(!engine.due(1_020, None, true), "within the manual gap");
        assert!(engine.due(1_010 + MANUAL_GAP, None, true));
    }

    /// Two keys saved back to back ask for two refreshes: the second must wait
    /// out the gap and then run, not vanish until the next 5-minute cycle.
    #[test]
    fn a_refresh_asked_inside_the_gap_runs_once_the_gap_has_passed() {
        let mut engine = Engine::default();
        run(&mut engine, 1_000);
        assert!(engine.due(1_010, None, true));
        run(&mut engine, 1_010);
        assert!(!engine.due(1_020, None, true), "inside the gap: deferred");
        assert!(!engine.due(1_030, None, false), "still inside the gap");
        assert!(engine.due(1_010 + MANUAL_GAP, None, false), "the deferred request runs without being asked again");
        run(&mut engine, 1_010 + MANUAL_GAP);
        assert!(!engine.due(1_010 + MANUAL_GAP + 1, None, false), "and only once");
    }

    /// A cycle the worker had to skip (config.json unreadable for a moment)
    /// keeps the request pending instead of swallowing it.
    #[test]
    fn a_manual_request_survives_a_skipped_cycle() {
        let mut engine = Engine::default();
        run(&mut engine, 1_000);
        assert!(engine.due(1_010, None, true));
        // No cycle ran (prefs could not be read): the next tick still owes one.
        assert!(engine.due(1_011, None, false));
        run(&mut engine, 1_011);
        assert!(!engine.due(1_012, None, false));
    }

    #[test]
    fn missing_logins_disappear_and_disabled_providers_are_never_fetched() {
        let mut engine = Engine::default();
        let calls = Cell::new(0);
        let fetch = |_: ProviderId, _: &Credential| {
            calls.set(calls.get() + 1);
            ok()
        };
        let prefs = Prefs::from([(ProviderId::Zai, Some(false))]);
        let snapshot = engine.cycle(1, &prefs, &Snapshot::default(), only(ProviderId::Zai, || login(1)), fetch);
        assert_eq!(calls.get(), 0);
        assert_eq!(snapshot.get(ProviderId::Zai).map(|p| &p.state), Some(&ProviderState::Idle));
        assert!(snapshot.get(ProviderId::Claude).is_none());
        let snapshot = engine.cycle(2, &Prefs::new(), &snapshot, only(ProviderId::Zai, || login(1)), fetch);
        assert_eq!(calls.get(), 1);
        let zai = snapshot.get(ProviderId::Zai).unwrap();
        assert_eq!(
            (&zai.state, zai.plan.as_deref(), zai.fetched_at, zai.windows.len()),
            (&ProviderState::Ok, Some("Pro"), Some(2), 1)
        );
    }

    #[test]
    fn rate_limits_pause_and_failures_keep_old_windows() {
        let mut engine = Engine::default();
        let first =
            engine.cycle(10, &Prefs::new(), &Snapshot::default(), only(ProviderId::Kimi, || login(1)), |_, _| ok());
        let calls = Cell::new(0);
        let limited = |_: ProviderId, _: &Credential| {
            calls.set(calls.get() + 1);
            Err(FetchError::Status { code: 429, retry_after: Some(120) })
        };
        let second = engine.cycle(20, &Prefs::new(), &first, only(ProviderId::Kimi, || login(1)), limited);
        let kimi = second.get(ProviderId::Kimi).unwrap();
        assert_eq!(
            (&kimi.state, kimi.windows.len(), kimi.fetched_at),
            (&ProviderState::RateLimited { retry_at: 140 }, 1, Some(10))
        );
        let third = engine.cycle(100, &Prefs::new(), &second, only(ProviderId::Kimi, || login(1)), limited);
        assert_eq!(calls.get(), 1, "paused until retry_at");
        assert_eq!(third.get(ProviderId::Kimi).unwrap().state, ProviderState::RateLimited { retry_at: 140 });
        let failed = engine.cycle(200, &Prefs::new(), &third, only(ProviderId::Kimi, || login(1)), |_, _| {
            Err(FetchError::Network("timeout".into()))
        });
        let kimi = failed.get(ProviderId::Kimi).unwrap();
        assert_eq!((&kimi.state, kimi.windows.len()), (&ProviderState::UpdateFailed { reason: "timeout".into() }, 1));
    }

    #[test]
    fn a_refused_login_is_not_retried_until_it_changes() {
        let mut engine = Engine::default();
        let calls = Cell::new(0);
        let refuse = |_: ProviderId, _: &Credential| {
            calls.set(calls.get() + 1);
            Err(FetchError::Status { code: 401, retry_after: None })
        };
        let s = engine.cycle(1, &Prefs::new(), &Snapshot::default(), only(ProviderId::Claude, || login(7)), refuse);
        let s = engine.cycle(400, &Prefs::new(), &s, only(ProviderId::Claude, || login(7)), refuse);
        assert_eq!(calls.get(), 1);
        assert_eq!(s.get(ProviderId::Claude).unwrap().state, ProviderState::AuthExpired);
        engine.cycle(800, &Prefs::new(), &s, only(ProviderId::Claude, || login(8)), refuse);
        assert_eq!(calls.get(), 2, "a new login is tried again");
    }

    #[test]
    fn expired_logins_are_shown_without_a_request() {
        let mut engine = Engine::default();
        let detection = || Detection::Expired { source: Source::ClaudeCode, marker: 1 };
        let s = engine.cycle(1, &Prefs::new(), &Snapshot::default(), only(ProviderId::Claude, detection), |_, _| {
            panic!("no request")
        });
        let claude = s.get(ProviderId::Claude).unwrap();
        assert_eq!((&claude.state, claude.source.as_str()), (&ProviderState::AuthExpired, "Claude Code"));
    }

    #[test]
    fn one_failing_provider_does_not_stop_the_others() {
        let mut engine = Engine::default();
        let fetch = |id: ProviderId, _: &Credential| {
            if id == ProviderId::Zai {
                Err(FetchError::Format("bad".into()))
            } else {
                ok()
            }
        };
        let s = engine.cycle(1, &Prefs::new(), &Snapshot::default(), |_| login(1), fetch);
        assert_eq!(s.providers.len(), ProviderId::ALL.len());
        assert_eq!(s.get(ProviderId::Zai).unwrap().state, ProviderState::FormatError { reason: "bad".into() });
        assert!(s.providers.iter().filter(|p| p.id != ProviderId::Zai).all(|p| p.state == ProviderState::Ok));
    }

    #[test]
    fn successful_responses_cannot_echo_the_key_to_the_cache() {
        use crate::quota::model::{Balance, BalanceKind, Unit};
        let mut engine = Engine::default();
        let secret = "fake-test-secret";
        let detect = || {
            let Detection::Found(mut c) = login(1) else { unreachable!() };
            c.secret = Secret::new(secret);
            Detection::Found(c)
        };
        let fetch = |_: ProviderId, _: &Credential| {
            let mut balance = Balance::new(secret, secret, 1.0, Unit::Usd, BalanceKind::Remaining);
            balance.detail = Some(format!("detail {secret}"));
            Ok(Fetched {
                plan: Some(secret.into()),
                windows: vec![Window::new(secret, secret, 1.0, None)],
                balances: vec![balance],
            })
        };
        let s = engine.cycle(1, &Prefs::new(), &Snapshot::default(), only(ProviderId::Zai, detect), fetch);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("quota.json");
        cache::write(&path, &s).unwrap();
        assert!(!std::fs::read_to_string(&path).unwrap().contains(secret));
        assert_eq!(s.get(ProviderId::Zai).unwrap().state, ProviderState::Ok);
    }

    #[test]
    fn store_failures_preserve_data_and_rate_limit_memory() {
        let mut engine = Engine::default();
        let first =
            engine.cycle(1, &Prefs::new(), &Snapshot::default(), only(ProviderId::Zai, || login(1)), |_, _| ok());
        let second = engine.cycle(2, &Prefs::new(), &first, only(ProviderId::Zai, || Detection::Unreadable), |_, _| {
            panic!("no credential, no request")
        });
        let p = second.get(ProviderId::Zai).unwrap();
        assert_eq!(
            (&p.state, p.fetched_at, &p.windows),
            (&ProviderState::StoreUnreadable, Some(1), &first.providers[0].windows)
        );
        assert!(!engine.due(2 + QUICK_RETRY - 1, None, false));
        assert!(engine.due(2 + QUICK_RETRY, None, false));
        let limited = engine.cycle(100, &Prefs::new(), &second, only(ProviderId::Zai, || login(1)), |_, _| {
            Err(FetchError::Status { code: 429, retry_after: Some(120) })
        });
        let busy =
            engine.cycle(110, &Prefs::new(), &limited, only(ProviderId::Zai, || Detection::Unreadable), |_, _| {
                panic!("no request while busy")
            });
        let mut restarted = Engine::default();
        let paused = restarted.cycle(150, &Prefs::new(), &busy, only(ProviderId::Zai, || login(1)), |_, _| {
            panic!("restart must honour persisted Retry-After")
        });
        assert_eq!(paused.providers[0].state, ProviderState::RateLimited { retry_at: 220 });
    }

    #[test]
    fn a_non_429_failure_breaks_the_backoff_streak_and_manual_wait_counts_down() {
        let mut engine = Engine::default();
        let limited = |_: ProviderId, _: &Credential| Err(FetchError::Status { code: 429, retry_after: None });
        let detect = only(ProviderId::Zai, || login(1));
        let first = engine.cycle(1, &Prefs::new(), &Snapshot::default(), &detect, limited);
        let second = engine.cycle(100, &Prefs::new(), &first, &detect, |_, _| Err(FetchError::Network("test".into())));
        let third = engine.cycle(200, &Prefs::new(), &second, &detect, limited);
        assert_eq!(third.providers[0].state, ProviderState::RateLimited { retry_at: 260 });
        assert!(engine.due(300, None, true));
        run(&mut engine, 300);
        assert_eq!(engine.manual_wait(300), MANUAL_GAP);
        assert_eq!(engine.manual_wait(320), 10);
        assert_eq!(engine.manual_wait(340), 0);
    }

    #[test]
    fn a_disabled_provider_with_an_unreadable_store_does_not_speed_up_network_polling() {
        let mut engine = Engine::default();
        let prefs = Prefs::from([(ProviderId::Zai, Some(false))]);
        engine.cycle(1, &prefs, &Snapshot::default(), only(ProviderId::Zai, || Detection::Unreadable), |_, _| {
            panic!("disabled provider cannot be requested")
        });
        assert!(!engine.due(1 + QUICK_RETRY, None, false));
    }
}
