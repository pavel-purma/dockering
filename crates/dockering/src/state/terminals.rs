//! `TerminalRegistry` (TRM-008): terminal sessions keyed by (engine, container) that
//! survive navigating to other tabs and pages of the same engine for up to 10 minutes.
//!
//! The hub owns the sessions themselves (terminal actors, spec 10 §3.3); this global keeps
//! the UI side (the `TerminalView` entities, their input senders and output tasks) so a
//! revisit re-attaches the same sessions. Closed on engine switch and container removal.

use std::collections::HashMap;
use std::time::Duration;

use dk_core::EngineId;
use gpui_kit::{App, Global, Task};

/// How long sessions outlive their Terminal tab (TRM-008).
pub const IDLE_TIMEOUT: Duration = Duration::from_secs(10 * 60);

type Key = (EngineId, String);

/// Something the registry can hold for a container: the terminal tab's sessions.
pub trait ParkedSessions: 'static {
    /// Close every session (TermCmd::Close) and drop the UI side.
    fn close(&mut self, cx: &mut App);
}

struct Parked {
    sessions: Box<dyn ParkedSessions>,
    /// Closes the sessions after [`IDLE_TIMEOUT`]; dropping it cancels the timer.
    _idle: Option<Task<()>>,
}

#[derive(Default)]
pub struct TerminalRegistry {
    parked: HashMap<Key, Parked>,
    /// Container ids with sessions currently shown by a mounted Terminal tab.
    live: HashMap<Key, usize>,
}

impl Global for TerminalRegistry {}

impl TerminalRegistry {
    fn get(cx: &mut App) -> &mut TerminalRegistry {
        if cx.try_global::<TerminalRegistry>().is_none() {
            cx.set_global(TerminalRegistry::default());
        }
        cx.global_mut::<TerminalRegistry>()
    }

    /// Parks a container's sessions when its Terminal tab unmounts (page left).
    pub fn park(engine: &EngineId, id: &str, sessions: Box<dyn ParkedSessions>, cx: &mut App) {
        let key = (engine.clone(), id.to_owned());
        let timer_key = key.clone();
        let idle = cx.spawn(async move |cx| {
            cx.background_executor().timer(IDLE_TIMEOUT).await;
            cx.update(|cx| {
                if let Some(mut p) = Self::get(cx).parked.remove(&timer_key) {
                    p.sessions.close(cx);
                }
            });
        });
        let reg = Self::get(cx);
        if let Some(mut old) = reg.parked.insert(
            key,
            Parked {
                sessions,
                _idle: Some(idle),
            },
        ) {
            old.sessions.close(cx);
        }
    }

    /// Takes the parked sessions of a container back (Terminal tab mounted again).
    pub fn take(engine: &EngineId, id: &str, cx: &mut App) -> Option<Box<dyn ParkedSessions>> {
        let key = (engine.clone(), id.to_owned());
        Self::get(cx).parked.remove(&key).map(|p| p.sessions)
    }

    pub fn has_parked(engine: &EngineId, id: &str, cx: &App) -> bool {
        cx.try_global::<TerminalRegistry>()
            .is_some_and(|r| r.parked.contains_key(&(engine.clone(), id.to_owned())))
    }

    pub fn parked_count(cx: &App) -> usize {
        cx.try_global::<TerminalRegistry>()
            .map_or(0, |r| r.parked.len())
    }

    /// Container removed (TRM-008, CDT-080).
    pub fn close_container(engine: &EngineId, id: &str, cx: &mut App) {
        let key = (engine.clone(), id.to_owned());
        if let Some(mut p) = Self::get(cx).parked.remove(&key) {
            p.sessions.close(cx);
        }
    }

    /// Engine switch (TRM-008): close every parked session not of `keep`.
    pub fn close_other_engines(keep: Option<&EngineId>, cx: &mut App) {
        let reg = Self::get(cx);
        let gone: Vec<Key> = reg
            .parked
            .keys()
            .filter(|(e, _)| Some(e) != keep)
            .cloned()
            .collect();
        let mut closing = Vec::new();
        for k in gone {
            if let Some(p) = reg.parked.remove(&k) {
                closing.push(p);
            }
        }
        reg.live.retain(|(e, _), _| Some(e) == keep);
        for mut p in closing {
            p.sessions.close(cx);
        }
    }
}
