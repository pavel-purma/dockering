//! Focus regions (KBD-005): Title bar, Sidebar, Toolbar, Content, Status bar. `F6` /
//! `Shift+F6` cycle; each region remembers its last-focused element.

use gpui_kit::{App, FocusHandle, Window};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Region {
    TitleBar,
    Sidebar,
    Content,
    StatusBar,
}

impl Region {
    pub const ORDER: [Region; 4] = [
        Region::TitleBar,
        Region::Sidebar,
        Region::Content,
        Region::StatusBar,
    ];
}

/// One region: its container focus handle (contains_focused test), the default target, and
/// the remembered last-focused child.
pub struct RegionSlot {
    pub region: Region,
    /// Handle tracked by the region's container element.
    pub container: FocusHandle,
    /// Where focus goes when entering the region with nothing remembered.
    pub default: FocusHandle,
    pub last: Option<FocusHandle>,
}

pub struct Regions {
    slots: Vec<RegionSlot>,
}

impl Regions {
    pub fn new(slots: Vec<RegionSlot>) -> Self {
        Self { slots }
    }

    pub fn container(&self, region: Region) -> Option<&FocusHandle> {
        self.slots
            .iter()
            .find(|s| s.region == region)
            .map(|s| &s.container)
    }

    pub fn set_default(&mut self, region: Region, handle: FocusHandle) {
        if let Some(s) = self.slots.iter_mut().find(|s| s.region == region) {
            s.default = handle;
        }
    }

    /// The region currently containing focus.
    pub fn current(&self, window: &Window, cx: &App) -> Option<Region> {
        self.slots
            .iter()
            .find(|s| s.container.contains_focused(window, cx))
            .map(|s| s.region)
    }

    /// Remembers the focused element of its region (called on every focus change).
    pub fn remember(&mut self, window: &Window, cx: &App) {
        let Some(focused) = window.focused(cx) else {
            return;
        };
        for s in &mut self.slots {
            if s.container.contains_focused(window, cx) {
                s.last = Some(focused.clone());
            }
        }
    }

    /// Focuses the next (`forward`) or previous region, restoring its last focus (KBD-005).
    pub fn cycle(&mut self, forward: bool, window: &mut Window, cx: &mut App) -> Option<Region> {
        self.remember(window, cx);
        let n = self.slots.len();
        if n == 0 {
            return None;
        }
        let cur = self
            .current(window, cx)
            .and_then(|r| self.slots.iter().position(|s| s.region == r));
        let start = match (cur, forward) {
            (Some(i), true) => (i + 1) % n,
            (Some(i), false) => (i + n - 1) % n,
            (None, true) => 0,
            (None, false) => n - 1,
        };
        let slot = &self.slots[start];
        let target = slot
            .last
            .clone()
            .filter(|h| slot.container.contains(h, window))
            .unwrap_or_else(|| slot.default.clone());
        window.focus(&target, cx);
        Some(slot.region)
    }
}
