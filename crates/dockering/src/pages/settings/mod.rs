//! TODO(phase-2): Settings (`docs/spec/features/settings.md`, SET-001…070).
//!
//! Route `Settings { section }` works (Mod+, / palette / title-bar button). Build each
//! section with GPUI Kit `setting`/`form` components; read/write through
//! `AppState::update_config` (persisted by the hub, debounced). `Keyboard` (SET-070)
//! should reuse `shell::shortcuts::ShortcutList` (the same view as the shortcut reference).

use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::{ActiveTheme, Selectable, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::{
    App, AppContext, Context, Entity, FocusHandle, Focusable, IntoElement, Render, Window, div,
};

use crate::actions::Navigate;
use crate::nav::{Route, SettingsSection};
use crate::shell::shortcuts::ShortcutList;
use crate::strings as s;
use crate::ui::page::PageView;

pub struct SettingsPage {
    section: SettingsSection,
    focus: FocusHandle,
    shortcuts: Option<Entity<ShortcutList>>,
}

impl SettingsPage {
    pub fn new(section: SettingsSection, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let shortcuts = (section == SettingsSection::Keyboard)
            .then(|| cx.new(|cx| ShortcutList::new(window, cx)));
        Self {
            section,
            focus: cx.focus_handle().tab_stop(true),
            shortcuts,
        }
    }
}

pub fn new(section: SettingsSection, window: &mut Window, cx: &mut App) -> Entity<SettingsPage> {
    cx.new(|cx| SettingsPage::new(section, window, cx))
}

impl Focusable for SettingsPage {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl PageView for SettingsPage {
    fn primary_focus(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for SettingsPage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let current = self.section;
        let focused = self.focus.is_focused(window);
        h_flex()
            .id("settings-page")
            .size_full()
            .child(
                v_flex()
                    .id("settings-nav")
                    .w(gpui_kit::px(200.))
                    .h_full()
                    .p_2()
                    .gap_1()
                    .border_r_1()
                    .border_color(cx.theme().border)
                    .children(SettingsSection::ALL.iter().map(|sec| {
                        let sec = *sec;
                        Button::new(("settings-section", sec as usize))
                            .ghost()
                            .w_full()
                            .label(sec.label())
                            .selected(sec == current)
                            .on_click(move |_, window, cx| {
                                window.dispatch_action(
                                    Box::new(Navigate {
                                        route: Route::Settings { section: sec },
                                    }),
                                    cx,
                                )
                            })
                    })),
            )
            .child(
                v_flex()
                    .flex_1()
                    .p_4()
                    .gap_3()
                    .child(
                        div()
                            .text_xl()
                            .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                            .child(format!("{} › {}", s::PAGE_SETTINGS, current.label())),
                    )
                    .map(|this| match &self.shortcuts {
                        Some(list) => this.child(list.clone()),
                        None => this.child(
                            div()
                                .id("settings-body")
                                .track_focus(&self.focus)
                                .p_3()
                                .rounded(cx.theme().radius)
                                .map(|el| crate::ui::focus_ring(el, focused, cx))
                                .text_color(cx.theme().muted_foreground)
                                .child(s::COMING_SOON),
                        ),
                    }),
            )
    }
}
