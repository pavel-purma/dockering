use std::time::Duration;

use gpui_kit::{App, AppContext, Context, Entity, Global, Task};
use time::OffsetDateTime;

/// One app-wide 30 s ticker for relative times (SHL-007). Views `cx.observe` it and
/// re-render; no per-row timers.
pub struct Ticker {
    now: OffsetDateTime,
    _task: Task<()>,
}

pub const TICK: Duration = Duration::from_secs(30);

struct GlobalTicker(Entity<Ticker>);
impl Global for GlobalTicker {}

impl Ticker {
    pub fn install(cx: &mut App) -> Entity<Ticker> {
        let entity = cx.new(|cx: &mut Context<Ticker>| Ticker {
            now: OffsetDateTime::now_utc(),
            _task: cx.spawn(async move |this, cx| {
                loop {
                    cx.background_executor().timer(TICK).await;
                    if this
                        .update(cx, |t, cx| {
                            t.now = OffsetDateTime::now_utc();
                            cx.notify();
                        })
                        .is_err()
                    {
                        break;
                    }
                }
            }),
        });
        cx.set_global(GlobalTicker(entity.clone()));
        entity
    }

    pub fn global(cx: &App) -> Option<Entity<Ticker>> {
        cx.try_global::<GlobalTicker>().map(|g| g.0.clone())
    }

    pub fn now(&self) -> OffsetDateTime {
        self.now
    }

    /// The ticker's "now", or the wall clock if none is installed.
    pub fn now_in(cx: &App) -> OffsetDateTime {
        Self::global(cx)
            .map(|t| t.read(cx).now)
            .unwrap_or_else(OffsetDateTime::now_utc)
    }
}
