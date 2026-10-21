//! Setup offers from the hub (ENG-150): fetched when a page needs them, refreshed on *Rescan*.

use dk_core::SetupOffer;
use dk_hub::HubHandle;
use gpui_kit::{App, AppContext, Context, Entity, Global, Task};

pub struct SetupStore {
    hub: HubHandle,
    offers: Vec<SetupOffer>,
    loaded: bool,
    requested: bool,
    /// Request id: only the newest answer is applied.
    revision: u64,
    task: Option<Task<()>>,
}

struct GlobalSetup(Entity<SetupStore>);
impl Global for GlobalSetup {}

impl SetupStore {
    pub fn install(hub: HubHandle, cx: &mut App) -> Entity<SetupStore> {
        let entity = cx.new(|_| SetupStore {
            hub,
            offers: Vec::new(),
            loaded: false,
            requested: false,
            revision: 0,
            task: None,
        });
        cx.set_global(GlobalSetup(entity.clone()));
        entity
    }

    pub fn global(cx: &App) -> Option<Entity<SetupStore>> {
        cx.try_global::<GlobalSetup>().map(|g| g.0.clone())
    }

    pub fn offers(&self) -> &[SetupOffer] {
        &self.offers
    }

    /// The first answer has arrived (before that a page shows a loading row).
    pub fn loaded(&self) -> bool {
        self.loaded
    }

    /// The offer with `id`; an empty `id` means the first one.
    pub fn offer(&self, id: &str) -> Option<&SetupOffer> {
        if id.is_empty() {
            self.offers.first()
        } else {
            self.offers.iter().find(|o| o.id == id)
        }
    }

    /// Fetches once, the first time a page asks.
    pub fn ensure(&mut self, cx: &mut Context<Self>) {
        if !self.requested {
            self.refresh(cx);
        }
    }

    /// Fetches again. The previous offers stay visible meanwhile (SHL-004).
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        self.requested = true;
        self.revision += 1;
        let rev = self.revision;
        let call = self.hub.setup_offers();
        self.task = Some(cx.spawn(async move |this, cx| {
            let result = call.await;
            this.update(cx, |this, cx| {
                if this.revision != rev {
                    return;
                }
                match result {
                    Ok(offers) => this.offers = offers,
                    Err(e) => tracing::debug!(%e, "setup offers unavailable"),
                }
                this.loaded = true;
                cx.notify();
            })
            .ok();
        }));
    }
}
