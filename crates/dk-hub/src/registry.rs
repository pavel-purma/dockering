//! Engine registry (spec 20 §2): discovered + manual engines, merge/de-duplication (ENG-009),
//! visibility, and the per-engine runtime state the supervisor maintains (ENG-020…025).
//!
//! Everything here is synchronous and runs under the hub's registry mutex; it never awaits.

use std::collections::HashSet;
use std::sync::Arc;

use dk_core::engine::preference;
use dk_core::{
    DiscoveredEngine, Engine, EngineConfig, EngineEndpoint, EngineFactory, EngineId, EngineInfo,
    EngineKind, EngineOrigin, EngineState, EngineStatus,
};
use tokio::sync::{Notify, broadcast};
use tokio_util::sync::CancellationToken;

use crate::bridge::HubEvent;
use crate::config::EngineSettings;

/// A live connection of the active engine. `token` is cancelled when the connection is dropped
/// (engine switch, ping failure, disable, hub shutdown). Every `call`, `subscribe`, shared
/// event upstream and terminal actor bound to it races this token and ends with
/// `Unreachable("engine '<id>' disconnected")` when it fires.
#[derive(Clone)]
pub(crate) struct Conn {
    pub engine: Arc<dyn Engine>,
    pub token: CancellationToken,
    /// Distinguishes successive connections of the same engine.
    pub generation: u64,
}

pub(crate) struct Entry {
    pub config: EngineConfig,
    pub state: EngineState,
    pub info: Option<EngineInfo>,
    /// ENG-103 rank; manual entries use `preference::MANUAL`.
    pub preference: u8,
    pub show_only_when_all: bool,
    /// Whether a discovery result has classified this entry yet. Entries registered from
    /// stored config at startup haven't: their first merge sets `show_only_when_all` outright,
    /// later merges only ever clear it (ENG-114, a listed engine is never hidden by a rescan).
    pub classified: bool,
    /// Index into the hub's factory list.
    pub factory: Option<usize>,
    /// Last-known daemon identity (ENG-009), in memory only.
    pub daemon_id: Option<String>,
    /// Names of the other engines that reach the same daemon (ENG-009). Annotation only:
    /// an engine is never hidden for sharing a daemon (ENG-114).
    pub also_reachable_via: Vec<String>,
    pub conn: Option<Conn>,
}

impl Entry {
    pub fn kind(&self) -> Option<EngineKind> {
        self.config.endpoint.kind()
    }

    /// Whether the hub may ever contact this engine (ENG-010/025/112).
    pub fn contactable(&self) -> bool {
        self.config.enabled
            && self.factory.is_some()
            && !matches!(
                self.config.endpoint,
                EngineEndpoint::Ssh { .. } | EngineEndpoint::Unknown
            )
            && !matches!(self.state, EngineState::Unsupported { .. })
    }
}

/// The active engine's supervisor task.
pub(crate) struct Supervisor {
    pub id: EngineId,
    pub token: CancellationToken,
    pub retry: Arc<Notify>,
}

/// Visibility-relevant settings mirrored from `Config.engines`.
#[derive(Debug, Clone, Default)]
pub(crate) struct View {
    pub show_all_wsl_distros: bool,
    pub show_all_wslc_sessions: bool,
}

impl View {
    pub fn from_settings(s: &EngineSettings) -> Self {
        Self {
            show_all_wsl_distros: s.show_all_wsl_distros,
            show_all_wslc_sessions: s.show_all_wslc_sessions,
        }
    }
}

pub(crate) struct Registry {
    pub entries: Vec<Entry>,
    pub view: View,
    pub active: Option<EngineId>,
    pub supervisor: Option<Supervisor>,
    pub events: broadcast::Sender<HubEvent>,
}

/// Initial state of an engine that isn't connected (ENG-010/025/106/112).
pub(crate) fn initial_state(
    cfg: &EngineConfig,
    factory: Option<usize>,
    hint: Option<&EngineState>,
) -> EngineState {
    if !cfg.enabled {
        return EngineState::Disabled;
    }
    match &cfg.endpoint {
        EngineEndpoint::Ssh { .. } => {
            return EngineState::Unsupported {
                reason: "SSH Docker contexts aren't supported yet".into(),
            };
        }
        EngineEndpoint::Unknown => {
            return EngineState::Unsupported {
                reason: "unknown endpoint type (written by a newer Dockering?)".into(),
            };
        }
        _ => {}
    }
    if factory.is_none() {
        return EngineState::Unsupported {
            reason: "no backend for this endpoint on this OS".into(),
        };
    }
    match hint {
        Some(
            s @ (EngineState::Stopped
            | EngineState::Unsupported { .. }
            | EngineState::Failed { .. }),
        ) => s.clone(),
        _ => EngineState::Disconnected,
    }
}

/// Slug for a manual engine id: lowercase, non-alphanumerics → `-`, uniquified with `-2`, `-3`, ….
pub(crate) fn slug_id(name: &str, taken: impl Fn(&str) -> bool) -> EngineId {
    let mut base = String::new();
    for c in name.chars() {
        if c.is_ascii_alphanumeric() {
            base.push(c.to_ascii_lowercase());
        } else if !base.ends_with('-') {
            base.push('-');
        }
    }
    let base = base.trim_matches('-');
    let base = if base.is_empty() { "engine" } else { base };
    if !taken(base) {
        return EngineId::new(base);
    }
    (2u32..)
        .map(|n| format!("{base}-{n}"))
        .find(|c| !taken(c))
        .map(EngineId::new)
        .unwrap_or_else(|| EngineId::new(base))
}

/// The factory serving an endpoint: the first one whose `handles()` is true.
pub(crate) fn factory_for(
    factories: &[Arc<dyn EngineFactory>],
    endpoint: &EngineEndpoint,
) -> Option<usize> {
    if matches!(endpoint, EngineEndpoint::Unknown) {
        return None;
    }
    factories.iter().position(|f| f.handles(endpoint))
}

impl Registry {
    pub fn new(view: View, events: broadcast::Sender<HubEvent>) -> Self {
        Self {
            entries: Vec::new(),
            view,
            active: None,
            supervisor: None,
            events,
        }
    }

    pub fn idx(&self, id: &EngineId) -> Option<usize> {
        self.entries.iter().position(|e| &e.config.id == id)
    }

    pub fn get(&self, id: &EngineId) -> Option<&Entry> {
        self.entries.iter().find(|e| &e.config.id == id)
    }

    pub fn get_mut(&mut self, id: &EngineId) -> Option<&mut Entry> {
        self.entries.iter_mut().find(|e| &e.config.id == id)
    }

    pub fn is_active(&self, id: &EngineId) -> bool {
        self.active.as_ref() == Some(id)
    }

    /// Async health results may only update the connection that started the work.
    pub(crate) fn current_connection(&self, id: &EngineId, conn: &Conn) -> bool {
        self.is_active(id)
            && !conn.token.is_cancelled()
            && self.get(id).and_then(|e| e.conn.as_ref()).is_some_and(|c| {
                c.generation == conn.generation && Arc::ptr_eq(&c.engine, &conn.engine)
            })
    }

    /// Whether `engines()` / hub events include this entry. Hidden engines are included (the
    /// UI filters `config.hidden`, Settings needs them); the active engine is always included.
    pub fn visible(&self, e: &Entry) -> bool {
        if self.is_active(&e.config.id) {
            return true;
        }
        if e.show_only_when_all {
            return match e.kind() {
                Some(EngineKind::WslDistro) => self.view.show_all_wsl_distros,
                Some(EngineKind::Wslc) => self.view.show_all_wslc_sessions,
                _ => true,
            };
        }
        true
    }

    pub fn status(&self, e: &Entry) -> EngineStatus {
        EngineStatus {
            config: e.config.clone(),
            state: e.state.clone(),
            info: e.info.clone(),
            active: self.is_active(&e.config.id),
            also_reachable_via: e.also_reachable_via.clone(),
        }
    }

    pub fn snapshot(&self) -> Vec<EngineStatus> {
        self.entries
            .iter()
            .filter(|e| self.visible(e))
            .map(|e| self.status(e))
            .collect()
    }

    pub fn emit(&self, ev: HubEvent) {
        let _ = self.events.send(ev);
    }

    pub fn emit_status(&self, id: &EngineId) {
        if let Some(e) = self.get(id)
            && self.visible(e)
        {
            self.emit(HubEvent::StatusChanged(self.status(e)));
        }
    }

    pub fn emit_snapshot(&self) {
        self.emit(HubEvent::Snapshot(self.snapshot()));
    }

    /// Sets the state and emits `StatusChanged` if it changed.
    pub fn set_state(&mut self, id: &EngineId, state: EngineState) {
        let Some(e) = self.get_mut(id) else { return };
        if e.state != state {
            e.state = state;
            self.emit_status(id);
        }
    }

    /// Adds a stored/manual config (used at startup and by `add_engine`).
    pub fn insert_config(&mut self, cfg: EngineConfig, factory: Option<usize>) {
        let state = initial_state(&cfg, factory, None);
        // Manual engines, and stored overrides of discovered engines that haven't been
        // rediscovered yet, rank last (ENG-103).
        let preference = preference::MANUAL;
        self.entries.push(Entry {
            config: cfg,
            state,
            info: None,
            preference,
            show_only_when_all: false,
            classified: false,
            factory,
            daemon_id: None,
            also_reachable_via: Vec::new(),
            conn: None,
        });
    }

    /// Merges a discovery result (spec 20 §2, ENG-009/010). `stored` are the user's config
    /// entries (manual engines + overrides of discovered ones). Returns the ids of new entries.
    pub fn merge_discovered(
        &mut self,
        found: Vec<(usize, DiscoveredEngine)>,
        stored: &[EngineConfig],
    ) -> Vec<EngineId> {
        // 1. De-duplicate discovery by canonical endpoint; the preferred (lowest rank) wins,
        //    ties keep discovery order.
        let mut found = found;
        found.sort_by_key(|(_, d)| d.preference);
        let mut seen_endpoints = HashSet::new();
        let mut seen_ids = HashSet::new();
        let mut unique = Vec::new();
        for (fi, d) in found {
            let canon = d.config.endpoint.canonical();
            if !seen_ids.insert(d.config.id.clone()) || !seen_endpoints.insert(canon.clone()) {
                continue;
            }
            // Also against existing entries with a different id (manual entries win).
            let clash = self
                .entries
                .iter()
                .any(|e| e.config.id != d.config.id && e.config.endpoint.canonical() == canon);
            if clash {
                continue;
            }
            unique.push((fi, d));
        }

        // 2. Merge into the registry.
        let mut added = Vec::new();
        let found_ids: HashSet<EngineId> =
            unique.iter().map(|(_, d)| d.config.id.clone()).collect();
        for (fi, d) in unique {
            let stored_cfg = stored.iter().find(|s| s.id == d.config.id);
            let mut cfg = d.config.clone();
            cfg.origin = EngineOrigin::Discovered;
            let mut manual = false;
            if let Some(s) = stored_cfg {
                if s.origin == EngineOrigin::Manual {
                    cfg = s.clone();
                    manual = true;
                } else {
                    cfg.name = s.name.clone();
                    cfg.enabled = s.enabled;
                    cfg.hidden = s.hidden;
                }
            }
            let is_active = self.is_active(&cfg.id);
            match self.idx(&cfg.id) {
                Some(i) => {
                    let e = &mut self.entries[i];
                    let endpoint_changed = e.config.endpoint != cfg.endpoint;
                    if endpoint_changed {
                        e.daemon_id = None;
                    }
                    if !manual {
                        e.preference = d.preference;
                        // Sticky (ENG-114): once classified, a rescan may make a
                        // hidden-by-Show-all engine listed, but never hides one the user
                        // already sees (a distro whose Docker socket is down right now is
                        // not "no Docker"). The first classification after startup is taken
                        // as is, so a stored override of a no-Docker distro stays unlisted.
                        e.show_only_when_all = if e.classified {
                            e.show_only_when_all && d.show_only_when_all
                        } else {
                            d.show_only_when_all
                        };
                        e.classified = true;
                        e.factory = Some(fi);
                    }
                    e.config = cfg;
                    let busy = is_active && (e.conn.is_some() || !endpoint_changed);
                    if !busy {
                        e.state = initial_state(&e.config, e.factory, d.initial_state.as_ref());
                    }
                }
                None => {
                    let state = initial_state(&cfg, Some(fi), d.initial_state.as_ref());
                    added.push(cfg.id.clone());
                    self.entries.push(Entry {
                        config: cfg,
                        state,
                        info: None,
                        preference: if manual {
                            preference::MANUAL
                        } else {
                            d.preference
                        },
                        show_only_when_all: d.show_only_when_all && !manual,
                        classified: true,
                        factory: Some(fi),
                        daemon_id: None,
                        also_reachable_via: Vec::new(),
                        conn: None,
                    });
                }
            }
        }

        self.refresh_daemon_notes();

        // 3. Vanished discovered engines stay listed as unavailable (Disconnected).
        let active = self.active.clone();
        for e in &mut self.entries {
            if e.config.origin == EngineOrigin::Discovered
                && !found_ids.contains(&e.config.id)
                && active.as_ref() != Some(&e.config.id)
                && e.conn.is_none()
                && !matches!(
                    e.state,
                    EngineState::Disabled | EngineState::Unsupported { .. }
                )
            {
                e.state = EngineState::Disconnected;
            }
        }
        added
    }

    /// ENG-009/114: records the daemon identity `id` reported on connect, then refreshes the
    /// "same daemon as" notes. Never hides or removes an engine.
    pub fn dedupe_daemon(&mut self, id: &EngineId, daemon_id: &str) {
        let Some(me) = self.idx(id) else { return };
        self.entries[me].daemon_id = Some(daemon_id.to_owned());
        self.refresh_daemon_notes();
    }

    /// Recomputes every engine's `also_reachable_via` from the current daemon ids, names,
    /// enabled and hidden flags, and visibility, in both directions (ENG-009/114). Peers that
    /// are disabled, hidden, or not listed (*Show all …* off) are left out, so the note never
    /// names an engine the user can't see. Emits `StatusChanged` for each visible entry that changed.
    pub fn refresh_daemon_notes(&mut self) {
        let notes: Vec<Vec<String>> = (0..self.entries.len())
            .map(|i| {
                let Some(d) = self.entries[i].daemon_id.as_deref() else {
                    return Vec::new();
                };
                (0..self.entries.len())
                    .filter(|&o| {
                        let p = &self.entries[o];
                        o != i
                            && p.daemon_id.as_deref() == Some(d)
                            && p.config.enabled
                            && !p.config.hidden
                            && self.visible(p)
                    })
                    .map(|o| self.entries[o].config.name.clone())
                    .collect()
            })
            .collect();
        for (i, note) in notes.into_iter().enumerate() {
            if self.entries[i].also_reachable_via != note {
                self.entries[i].also_reachable_via = note;
                let eid = self.entries[i].config.id.clone();
                self.emit_status(&eid);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dk_core::{EngineOrigin, WslMode};

    fn cfg(id: &str, endpoint: EngineEndpoint) -> EngineConfig {
        EngineConfig {
            id: EngineId::new(id),
            name: id.to_uppercase(),
            endpoint,
            origin: EngineOrigin::Discovered,
            enabled: true,
            hidden: false,
        }
    }

    fn sock(p: &str) -> EngineEndpoint {
        EngineEndpoint::UnixSocket { path: p.into() }
    }

    fn reg() -> Registry {
        Registry::new(View::default(), broadcast::channel(16).0)
    }

    #[test]
    fn slug_lowercases_and_uniquifies() {
        let taken = ["my-engine", "my-engine-2"];
        assert_eq!(
            slug_id("My Engine", |s| taken.contains(&s)).as_str(),
            "my-engine-3"
        );
        assert_eq!(
            slug_id("  Docker @ Home ", |_| false).as_str(),
            "docker-home"
        );
        assert_eq!(slug_id("!!!", |_| false).as_str(), "engine");
    }

    #[test]
    fn eng_009_discovery_dedupes_by_canonical_endpoint_preferring_lower_rank() {
        let mut r = reg();
        let a = DiscoveredEngine::new(cfg("ctx", sock("/var/run/docker.sock")), 10);
        let b = DiscoveredEngine::new(cfg("local", sock("/var/run/docker.sock")), 20);
        let added = r.merge_discovered(vec![(0, b), (0, a)], &[]);
        assert_eq!(added, vec![EngineId::new("ctx")]);
        assert_eq!(r.entries.len(), 1);
    }

    #[test]
    fn eng_010_ssh_and_unknown_are_unsupported() {
        let mut r = reg();
        let s = DiscoveredEngine::new(
            cfg(
                "remote",
                EngineEndpoint::Ssh {
                    url: "ssh://h".into(),
                },
            ),
            10,
        );
        r.merge_discovered(vec![(0, s)], &[]);
        assert!(matches!(
            r.entries[0].state,
            EngineState::Unsupported { .. }
        ));
        assert!(!r.entries[0].contactable());
        let u = initial_state(&cfg("x", EngineEndpoint::Unknown), Some(0), None);
        assert!(matches!(u, EngineState::Unsupported { .. }));
    }

    #[test]
    fn stored_overrides_apply_to_discovered_and_manual_wins() {
        let mut r = reg();
        let mut over = cfg("a", sock("/a"));
        over.name = "Renamed".into();
        over.enabled = false;
        over.endpoint = sock("/ignored");
        let mut manual = cfg("b", sock("/manual-b"));
        manual.origin = EngineOrigin::Manual;
        r.merge_discovered(
            vec![
                (0, DiscoveredEngine::new(cfg("a", sock("/a")), 20)),
                (0, DiscoveredEngine::new(cfg("b", sock("/b")), 20)),
            ],
            &[over, manual.clone()],
        );
        let a = r.get(&EngineId::new("a")).unwrap();
        assert_eq!(a.config.name, "Renamed");
        assert_eq!(a.config.endpoint, sock("/a"));
        assert_eq!(a.state, EngineState::Disabled);
        assert_eq!(r.get(&EngineId::new("b")).unwrap().config, manual);
    }

    #[test]
    fn vanished_discovered_engine_stays_listed_disconnected() {
        let mut r = reg();
        let mut d = DiscoveredEngine::new(cfg("a", sock("/a")), 20);
        d.initial_state = Some(EngineState::Stopped);
        r.merge_discovered(vec![(0, d)], &[]);
        assert_eq!(r.entries[0].state, EngineState::Stopped);
        r.merge_discovered(vec![], &[]);
        assert_eq!(r.entries.len(), 1);
        assert_eq!(r.entries[0].state, EngineState::Disconnected);
    }

    #[test]
    fn show_only_when_all_respects_settings() {
        let mut r = reg();
        let mut d = DiscoveredEngine::new(
            cfg(
                "wsl-alpine",
                EngineEndpoint::WslDistro {
                    distro: "Alpine".into(),
                    mode: WslMode::DialStdio,
                },
            ),
            30,
        );
        d.show_only_when_all = true;
        r.merge_discovered(vec![(0, d)], &[]);
        assert!(r.snapshot().is_empty());
        r.view.show_all_wsl_distros = true;
        assert_eq!(r.snapshot().len(), 1);
    }

    #[test]
    fn eng_114_rescan_never_hides_a_listed_engine() {
        let wsl = |no_docker: bool| {
            let mut d = DiscoveredEngine::new(
                cfg(
                    "wsl-alpine",
                    EngineEndpoint::WslDistro {
                        distro: "Alpine".into(),
                        mode: WslMode::DialStdio,
                    },
                ),
                30,
            );
            d.show_only_when_all = no_docker;
            d
        };
        let mut r = reg();
        r.merge_discovered(vec![(0, wsl(false))], &[]);
        assert_eq!(r.snapshot().len(), 1);
        // Docker is down in the distro at rescan time: it's classified "no Docker", but the
        // user already sees it, so it stays listed.
        r.merge_discovered(vec![(0, wsl(true))], &[]);
        assert_eq!(r.snapshot().len(), 1);
        // The other way round: a hidden "no Docker" distro becomes listed once Docker is up.
        let mut r = reg();
        r.merge_discovered(vec![(0, wsl(true))], &[]);
        assert!(r.snapshot().is_empty());
        r.merge_discovered(vec![(0, wsl(false))], &[]);
        assert_eq!(r.snapshot().len(), 1);
    }

    #[test]
    fn eng_114_stored_override_of_a_no_docker_distro_stays_unlisted_after_restart() {
        let distro = EngineEndpoint::WslDistro {
            distro: "Alpine".into(),
            mode: WslMode::DialStdio,
        };
        let mut stored = cfg("wsl-alpine", distro.clone());
        stored.name = "My Alpine".into();
        let mut r = reg();
        r.insert_config(stored.clone(), Some(0));
        let mut d = DiscoveredEngine::new(cfg("wsl-alpine", distro), 30);
        d.show_only_when_all = true;
        r.merge_discovered(vec![(0, d)], &[stored]);
        let e = r.get(&EngineId::new("wsl-alpine")).unwrap();
        assert_eq!(e.config.name, "My Alpine");
        assert!(e.show_only_when_all);
        assert!(r.snapshot().is_empty(), "Show all is off");
    }

    #[test]
    fn eng_009_same_daemon_is_annotated_not_hidden() {
        let mut r = reg();
        r.merge_discovered(
            vec![
                (0, DiscoveredEngine::new(cfg("pipe", sock("/pipe")), 20)),
                (0, DiscoveredEngine::new(cfg("wsl", sock("/wsl")), 30)),
            ],
            &[],
        );
        r.dedupe_daemon(&EngineId::new("wsl"), "D");
        assert!(r.entries.iter().all(|e| e.also_reachable_via.is_empty()));
        r.dedupe_daemon(&EngineId::new("pipe"), "D");
        let ids: Vec<_> = r.snapshot().into_iter().map(|s| s.config.id).collect();
        assert_eq!(ids, vec![EngineId::new("pipe"), EngineId::new("wsl")]);
        assert_eq!(r.entries[0].also_reachable_via, vec!["WSL".to_string()]);
        assert_eq!(r.entries[1].also_reachable_via, vec!["PIPE".to_string()]);
    }
}
