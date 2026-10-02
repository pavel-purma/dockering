//! Settings page (`Route::Settings { section }`, SET-001…070, ENG-104/105/109/110).
//!
//! Layout: the section nav is the app sidebar in Settings mode (SET-080, `shell::app_shell`),
//! so this page is only the scrolling content column of titled sections (`ui::section`) with
//! `Switch`, `NumberInput`, `Input`, and `Select` controls.
//!
//! The GPUI Kit `Settings` container isn't used: its virtualised page list drops off-screen
//! controls from the Tab order (KBD-072/092), its section can't follow the route (deep
//! links, back/forward), and it has no inline validation. The page uses the same building
//! blocks instead.
//!
//! Every change goes through [`AppState::update_config`] (the hub saves it, debounced) and
//! applies live: the theme here, everything else through the shell's config observer.

pub mod add_engine;
pub mod controls;
pub mod diagnostics;
pub mod engines;
pub mod mapping;

use std::collections::HashMap;

use gpui_kit::component::input::{Input, InputEvent, InputState, NumberInput};
use gpui_kit::component::select::{Select, SelectEvent, SelectState};
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{ActiveTheme, IndexPath, Sizable, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::{
    AnyElement, App, AppContext, Context, Entity, FocusHandle, Focusable, IntoElement, Render,
    ScrollHandle, SharedString, Subscription, Window, div, point, px,
};

pub use add_engine::AddEngineDialog;
pub use diagnostics::{copy_diagnostics, view_licenses};

use crate::nav::SettingsSection;
use crate::shell::shortcuts::ShortcutList;
use crate::state::{AppState, EngineListStore};
use crate::strings as s;
use crate::theme;
use crate::ui::page::PageView;
use controls::{BoolKey, Key, NumSpec, Spec};

/// Options of a choice setting's `Select`.
type Choice = Vec<&'static str>;

pub struct SettingsPage {
    section: SettingsSection,
    engine_list: Entity<EngineListStore>,
    /// The page root (not a Tab stop). Page-local dispatch (`dispatch_here`) starts here, and
    /// it is the Content region's default; the section nav lives in the shell sidebar.
    focus: FocusHandle,
    scroll: ScrollHandle,
    /// One non-tab-stop focus container per content block: the block holding focus is
    /// scrolled into view (keyboard users never tab into an invisible control).
    blocks: Vec<FocusHandle>,
    last_block: Option<usize>,
    inputs: HashMap<Key, Entity<InputState>>,
    selects: HashMap<Key, Entity<SelectState<Choice>>>,
    errors: HashMap<Key, SharedString>,
    /// *Group by* is set to *Label…* in the UI but no key has been entered yet.
    label_pending: bool,
    shortcuts: Entity<ShortcutList>,
    engines: engines::EnginesUi,
    /// The open (or last) *Add engine* dialog (tests drive it).
    add_dialog: Option<Entity<AddEngineDialog>>,
    _subs: Vec<Subscription>,
}

impl SettingsPage {
    pub fn new(
        section: SettingsSection,
        engine_list: Entity<EngineListStore>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let config = AppState::config(cx).clone();
        let mut subs = vec![
            cx.observe(&engine_list, |_, _, cx| cx.notify()),
            // Settings change elsewhere too (title-bar theme toggle, group-by menu, zoom).
            cx.observe_global_in::<AppState>(window, |this, window, cx| {
                this.sync_from_config(window, cx);
                cx.notify();
            }),
        ];
        let mut inputs = HashMap::new();
        let mut selects = HashMap::new();
        for section in SettingsSection::ALL {
            for &key in controls::keys_for(section) {
                match controls::spec(key) {
                    Spec::Choice(options) => {
                        let ix = controls::choice_index(key, &config);
                        let state = cx.new(|cx| {
                            SelectState::new(options.to_vec(), Some(IndexPath::new(ix)), window, cx)
                        });
                        subs.push(cx.subscribe_in(
                            &state,
                            window,
                            move |this, _, e: &SelectEvent<Choice>, window, cx| {
                                if let SelectEvent::Confirm(Some(v)) = e
                                    && let Some(ix) = options.iter().position(|o| o == v)
                                {
                                    this.set_choice(key, ix, window, cx);
                                }
                            },
                        ));
                        selects.insert(key, state);
                    }
                    Spec::Number(n) => {
                        let text = controls::fmt_num((n.get)(&config), n.integer);
                        let state = cx.new(|cx| {
                            InputState::new(window, cx)
                                .default_value(text)
                                .step(1.)
                                .min(n.min)
                                .max(n.max)
                        });
                        subs.push(
                            cx.subscribe_in(&state, window, move |this, input, e, _, cx| {
                                if matches!(e, InputEvent::Change | InputEvent::Blur) {
                                    let text = input.read(cx).value().to_string();
                                    this.commit_number(key, &text, cx);
                                }
                            }),
                        );
                        inputs.insert(key, state);
                    }
                    Spec::Text(t) => {
                        let text = (t.get)(&config);
                        let state = cx.new(|cx| {
                            let s = InputState::new(window, cx).default_value(text);
                            match t.placeholder {
                                Some(p) => s.placeholder(p),
                                None => s,
                            }
                        });
                        subs.push(
                            cx.subscribe_in(&state, window, move |this, input, e, _, cx| {
                                if matches!(e, InputEvent::Change) {
                                    let text = input.read(cx).value().to_string();
                                    this.commit_text(key, &text, cx);
                                }
                            }),
                        );
                        inputs.insert(key, state);
                    }
                }
            }
        }
        Self {
            section,
            engine_list,
            focus: cx.focus_handle(),
            scroll: ScrollHandle::new(),
            blocks: Vec::new(),
            last_block: None,
            inputs,
            selects,
            errors: HashMap::new(),
            label_pending: false,
            shortcuts: cx.new(|cx| ShortcutList::new(window, cx)),
            engines: engines::EnginesUi::new(cx),
            add_dialog: None,
            _subs: subs,
        }
    }

    // ── accessors (tests) ──────────────────────────────────────────────────────────────

    pub fn section(&self) -> SettingsSection {
        self.section
    }
    pub fn input(&self, key: Key) -> Option<&Entity<InputState>> {
        self.inputs.get(&key)
    }
    pub fn select(&self, key: Key) -> Option<&Entity<SelectState<Choice>>> {
        self.selects.get(&key)
    }
    pub fn error(&self, key: Key) -> Option<&SharedString> {
        self.errors.get(&key)
    }
    pub fn shortcuts(&self) -> &Entity<ShortcutList> {
        &self.shortcuts
    }
    pub fn focus(&self) -> &FocusHandle {
        &self.focus
    }
    pub fn engines_ui(&self) -> &engines::EnginesUi {
        &self.engines
    }
    pub fn add_dialog(&self) -> Option<&Entity<AddEngineDialog>> {
        self.add_dialog.as_ref()
    }

    /// Opens *Add engine* (ENG-105). The invoker is the section's *Add engine…* button, so
    /// focus returns there when the dialog closes (KBD-071).
    pub fn open_add_engine(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let h = self.engines.add_focus().clone();
        window.focus(&h, cx);
        let engines = self.engine_list.read(cx).engines().to_vec();
        self.add_dialog = Some(AddEngineDialog::open(&engines, window, cx));
    }

    /// The route moved to another section of this page (the shell keeps the entity).
    pub fn set_section(&mut self, section: SettingsSection, cx: &mut Context<Self>) {
        if self.section != section {
            self.section = section;
            self.last_block = None;
            self.scroll.set_offset(point(px(0.), px(0.)));
            cx.notify();
        }
    }

    // ── writes (SET-001…060) ───────────────────────────────────────────────────────────

    /// A switch changed.
    pub fn set_bool(&mut self, key: BoolKey, value: bool, cx: &mut Context<Self>) {
        AppState::update_config(cx, |c| key.set(c, value));
        cx.notify();
    }

    /// A select changed. Applied live (theme now; the shell picks up the rest).
    pub fn set_choice(&mut self, key: Key, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        if key == Key::GroupBy && ix == 2 {
            // Label: needs the key from the label-key field.
            self.label_pending = true;
            let text = self
                .inputs
                .get(&Key::LabelKey)
                .map(|i| i.read(cx).value().to_string())
                .unwrap_or_default();
            self.commit_label_key(&text, cx);
            if self.errors.contains_key(&Key::LabelKey)
                && let Some(i) = self.inputs.get(&Key::LabelKey)
            {
                i.update(cx, |i, cx| i.focus(window, cx));
            }
            cx.notify();
            return;
        }
        if key == Key::GroupBy {
            self.label_pending = false;
            self.errors.remove(&Key::LabelKey);
        }
        AppState::update_config(cx, |c| controls::set_choice(key, c, ix));
        if key == Key::Theme {
            let general = AppState::config(cx).general.clone();
            theme::apply(general.theme, general.ui_scale, Some(window), cx);
        }
        cx.notify();
    }

    fn commit_number(&mut self, key: Key, text: &str, cx: &mut Context<Self>) {
        let Spec::Number(spec) = controls::spec(key) else {
            return;
        };
        match controls::parse_num(text, &spec) {
            Ok(v) => {
                self.errors.remove(&key);
                if (spec.get)(AppState::config(cx)) != v {
                    AppState::update_config(cx, |c| (spec.set)(c, v));
                }
            }
            Err(e) => {
                self.errors.insert(key, e.into());
            }
        }
        cx.notify();
    }

    fn commit_text(&mut self, key: Key, text: &str, cx: &mut Context<Self>) {
        if key == Key::LabelKey {
            self.commit_label_key(text, cx);
            cx.notify();
            return;
        }
        let Spec::Text(spec) = controls::spec(key) else {
            return;
        };
        match (spec.validate)(text) {
            None => {
                self.errors.remove(&key);
                let v = text.trim().to_owned();
                if (spec.get)(AppState::config(cx)) != v {
                    AppState::update_config(cx, |c| (spec.set)(c, v));
                }
            }
            Some(e) => {
                self.errors.insert(key, e.into());
            }
        }
        cx.notify();
    }

    /// The label-key field only matters while *Group by* is *Label…* (CON-010).
    fn commit_label_key(&mut self, text: &str, cx: &mut Context<Self>) {
        let label_selected = self.label_pending
            || matches!(
                AppState::config(cx).containers.group_by,
                dk_core::grouping::GroupBy::Label(_)
            );
        if !label_selected {
            self.errors.remove(&Key::LabelKey);
            return;
        }
        let k = text.trim();
        if k.is_empty() {
            self.errors
                .insert(Key::LabelKey, s::SET_LABEL_KEY_REQUIRED.into());
            return;
        }
        self.errors.remove(&Key::LabelKey);
        self.label_pending = false;
        let g = dk_core::grouping::GroupBy::Label(k.to_owned());
        if AppState::config(cx).containers.group_by != g {
            AppState::update_config(cx, |c| c.containers.group_by = g);
        }
    }

    /// Shows changes made elsewhere. Fields with focus or a pending error keep what the user
    /// typed.
    fn sync_from_config(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let config = AppState::config(cx).clone();
        for (&key, state) in &self.selects {
            let ix = controls::choice_index(key, &config);
            let pending_label = key == Key::GroupBy && self.label_pending;
            if !pending_label && state.read(cx).selected_index(cx) != Some(IndexPath::new(ix)) {
                state.update(cx, |s, cx| {
                    s.set_selected_index(Some(IndexPath::new(ix)), window, cx)
                });
            }
        }
        for (&key, state) in &self.inputs {
            if self.errors.contains_key(&key) || state.focus_handle(cx).is_focused(window) {
                continue;
            }
            let want = match controls::spec(key) {
                Spec::Number(n) => controls::fmt_num((n.get)(&config), n.integer),
                Spec::Text(t) => (t.get)(&config),
                Spec::Choice(_) => continue,
            };
            // The label key is kept while grouping by something else.
            if key == Key::LabelKey && want.is_empty() {
                continue;
            }
            if state.read(cx).value() != want {
                state.update(cx, |s, cx| s.set_value(want, window, cx));
            }
        }
    }

    // ── content ────────────────────────────────────────────────────────────────────────

    fn select_ctl(&self, key: Key) -> AnyElement {
        match self.selects.get(&key) {
            Some(state) => div()
                .w(px(220.))
                .child(Select::new(state).small())
                .into_any_element(),
            None => div().into_any_element(),
        }
    }

    fn number_ctl(&self, key: Key, spec: &NumSpec, cx: &App) -> AnyElement {
        match self.inputs.get(&key) {
            Some(state) => h_flex()
                .gap_2()
                .items_center()
                .child(div().w(px(150.)).child(NumberInput::new(state).small()))
                .when(!spec.unit.is_empty(), |this| {
                    this.child(
                        div()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .child(spec.unit),
                    )
                })
                .into_any_element(),
            None => div().into_any_element(),
        }
    }

    fn text_ctl(&self, key: Key) -> AnyElement {
        match self.inputs.get(&key) {
            Some(state) => div()
                .w(px(260.))
                .child(Input::new(state).small())
                .into_any_element(),
            None => div().into_any_element(),
        }
    }

    /// The control for a [`Key`].
    fn ctl(&self, key: Key, cx: &App) -> AnyElement {
        match controls::spec(key) {
            Spec::Choice(_) => self.select_ctl(key),
            Spec::Number(n) => self.number_ctl(key, &n, cx),
            Spec::Text(_) => self.text_ctl(key),
        }
    }

    /// A row for a keyed setting: title, description, control, inline error.
    fn key_row(&self, key: Key, title: &'static str, desc: &'static str, cx: &App) -> AnyElement {
        setting_row(
            title,
            Some(desc),
            self.ctl(key, cx),
            self.errors.get(&key).cloned(),
            cx,
        )
    }

    fn bool_row(
        &self,
        key: BoolKey,
        title: &'static str,
        desc: &'static str,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let checked = key.get(AppState::config(cx));
        let ctl = Switch::new(key.id())
            .checked(checked)
            .accessibility_label(title)
            .on_click(cx.listener(move |this, v: &bool, _, cx| this.set_bool(key, *v, cx)))
            .into_any_element();
        setting_row(title, Some(desc), ctl, None, cx)
    }

    fn blocks_for(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Vec<AnyElement> {
        use SettingsSection as S;
        match self.section {
            S::General => vec![
                group(
                    s::SET_GROUP_APPEARANCE,
                    vec![self.key_row(Key::Theme, s::SET_THEME, s::SET_THEME_DESC, cx)],
                    cx,
                ),
                group(
                    s::SET_GROUP_BEHAVIOUR,
                    vec![
                        self.key_row(
                            Key::StartPage,
                            s::SET_START_PAGE,
                            s::SET_START_PAGE_DESC,
                            cx,
                        ),
                        self.bool_row(
                            BoolKey::ConfirmStopped,
                            s::SET_CONFIRM_STOPPED,
                            s::SET_CONFIRM_STOPPED_DESC,
                            cx,
                        ),
                        self.bool_row(
                            BoolKey::ShowNetworks,
                            s::SET_SHOW_NETWORKS,
                            s::SET_SHOW_NETWORKS_DESC,
                            cx,
                        ),
                    ],
                    cx,
                ),
            ],
            S::Containers => vec![
                group(
                    s::SET_GROUP_LIST,
                    vec![
                        self.key_row(Key::GroupBy, s::SET_GROUP_BY, s::SET_GROUP_BY_DESC, cx),
                        self.key_row(
                            Key::LabelKey,
                            s::SET_GROUP_LABEL_KEY,
                            s::SET_GROUP_LABEL_KEY_DESC,
                            cx,
                        ),
                        self.bool_row(
                            BoolKey::ShowStats,
                            s::SET_SHOW_STATS,
                            s::SET_SHOW_STATS_DESC,
                            cx,
                        ),
                    ],
                    cx,
                ),
                group(
                    s::SET_GROUP_UPDATES,
                    vec![self.key_row(Key::Polling, s::SET_POLLING, s::SET_POLLING_DESC, cx)],
                    cx,
                ),
            ],
            S::Logs => vec![group(
                s::SET_GROUP_VIEW,
                vec![
                    self.key_row(Key::LogsTail, s::SET_LOGS_TAIL, s::SET_LOGS_TAIL_DESC, cx),
                    self.key_row(Key::LogsMax, s::SET_LOGS_MAX, s::SET_LOGS_MAX_DESC, cx),
                    self.bool_row(
                        BoolKey::LogsTimestamps,
                        s::SET_LOGS_TIMESTAMPS,
                        s::SET_LOGS_TIMESTAMPS_DESC,
                        cx,
                    ),
                    self.bool_row(
                        BoolKey::LogsWrap,
                        s::SET_LOGS_WRAP,
                        s::SET_LOGS_WRAP_DESC,
                        cx,
                    ),
                ],
                cx,
            )],
            S::Terminal => vec![
                group(
                    s::SET_GROUP_FONT,
                    vec![
                        self.key_row(Key::FontFamily, s::SET_TERM_FONT, s::SET_TERM_FONT_DESC, cx),
                        setting_row(
                            s::SET_TERM_FONT_SIZE,
                            None,
                            self.ctl(Key::FontSize, cx),
                            self.errors.get(&Key::FontSize).cloned(),
                            cx,
                        ),
                    ],
                    cx,
                ),
                group(
                    s::SET_GROUP_SESSION,
                    vec![
                        self.key_row(Key::Shell, s::SET_TERM_SHELL, s::SET_TERM_SHELL_DESC, cx),
                        setting_row(
                            s::SET_TERM_SCROLLBACK,
                            None,
                            self.ctl(Key::Scrollback, cx),
                            self.errors.get(&Key::Scrollback).cloned(),
                            cx,
                        ),
                        self.key_row(
                            Key::External,
                            s::SET_TERM_EXTERNAL,
                            s::SET_TERM_EXTERNAL_DESC,
                            cx,
                        ),
                    ],
                    cx,
                ),
            ],
            S::Stats => vec![group(
                s::SET_GROUP_HISTORY,
                vec![
                    self.key_row(
                        Key::StatsHistory,
                        s::SET_STATS_HISTORY,
                        s::SET_STATS_HISTORY_DESC,
                        cx,
                    ),
                    self.bool_row(
                        BoolKey::AllCores,
                        s::SET_STATS_ALL_CORES,
                        s::SET_STATS_ALL_CORES_DESC,
                        cx,
                    ),
                ],
                cx,
            )],
            S::Diagnostics => diagnostics::blocks(self, window, cx),
            S::Engines => engines::blocks(self, window, cx),
            S::Keyboard => vec![
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(s::KEYBOARD_NOTE)
                    .into_any_element(),
                div()
                    .id("settings-shortcuts")
                    .child(self.shortcuts.clone())
                    .into_any_element(),
            ],
        }
    }

    fn description(&self) -> &'static str {
        use SettingsSection as S;
        match self.section {
            S::General => s::SET_DESC_GENERAL,
            S::Engines => s::SET_DESC_ENGINES,
            S::Containers => s::SET_DESC_CONTAINERS,
            S::Logs => s::SET_DESC_LOGS,
            S::Terminal => s::SET_DESC_TERMINAL,
            S::Stats => s::SET_DESC_STATS,
            S::Diagnostics => s::SET_DESC_DIAGNOSTICS,
            S::Keyboard => s::SET_DESC_KEYBOARD,
        }
    }

    /// Keeps the block that holds focus in view (Tab into an off-screen control).
    fn reveal_focused_block(&mut self, window: &Window, cx: &App) {
        let focused = self
            .blocks
            .iter()
            .position(|b| b.contains_focused(window, cx));
        if focused.is_some() && focused != self.last_block {
            // Child 0 of the scroll column is the section header.
            if let Some(ix) = focused {
                self.scroll.scroll_to_item(ix + 1);
            }
        }
        self.last_block = focused;
    }
}

/// Click handler that dispatches `action` from the page's own root node, so it
/// reaches the page's and the shell's handlers even when focus is elsewhere: GPUI Kit buttons
/// don't take focus on mouse down, and `Window::dispatch_action` starts at the focused node.
pub(crate) fn dispatch_here(
    page: &FocusHandle,
    action: impl gpui_kit::Action,
) -> impl Fn(&gpui_kit::ClickEvent, &mut Window, &mut App) + 'static {
    let page = page.clone();
    move |_, window, cx| page.dispatch_action(&action, window, cx)
}

/// A titled section of setting rows.
fn group(title: &'static str, rows: Vec<AnyElement>, cx: &App) -> AnyElement {
    crate::ui::section(title, v_flex().gap_3().children(rows), cx).into_any_element()
}

/// One setting: title and description on the left, the control (and its inline validation
/// message) on the right.
fn setting_row(
    title: &'static str,
    desc: Option<&'static str>,
    control: AnyElement,
    error: Option<SharedString>,
    cx: &App,
) -> AnyElement {
    h_flex()
        .w_full()
        .gap_6()
        .justify_between()
        .items_start()
        .child(
            v_flex()
                .flex_1()
                .min_w_0()
                .gap_0p5()
                .child(div().text_sm().child(title))
                .when_some(desc, |this, d| {
                    this.child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(d),
                    )
                }),
        )
        .child(
            v_flex()
                .flex_shrink_0()
                .items_end()
                .gap_1()
                .child(control)
                .when_some(error, |this, e| {
                    this.child(
                        div()
                            .max_w(px(260.))
                            .text_xs()
                            .text_color(cx.theme().danger)
                            .child(e),
                    )
                }),
        )
        .into_any_element()
}

pub fn new(
    section: SettingsSection,
    engines: Entity<EngineListStore>,
    window: &mut Window,
    cx: &mut App,
) -> Entity<SettingsPage> {
    cx.new(|cx| SettingsPage::new(section, engines, window, cx))
}

impl Focusable for SettingsPage {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl PageView for SettingsPage {
    /// The page root; the shell focuses its sidebar section nav instead on navigation
    /// (SET-080, KBD-024).
    fn primary_focus(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }

    /// `Mod+F` on Settings › Keyboard filters the shortcuts.
    fn focus_search(&mut self, window: &mut Window, cx: &mut App) {
        if self.section == SettingsSection::Keyboard {
            let list = self.shortcuts.clone();
            list.update(cx, |l, cx| l.focus_filter(window, cx));
        }
    }
}

/// Width cap of the settings column (header and groups share it, so their edges line up).
const CONTENT_MAX_W: gpui_kit::Pixels = px(900.);

impl Render for SettingsPage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.engines.sync_name_inputs(&self.engine_list, window, cx);
        let blocks = self.blocks_for(window, cx);
        while self.blocks.len() < blocks.len() {
            self.blocks.push(cx.focus_handle());
        }
        self.reveal_focused_block(window, cx);
        let header = v_flex()
            .w_full()
            .max_w(CONTENT_MAX_W)
            .gap_1()
            .pb_2()
            .child(
                div()
                    .text_xl()
                    .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                    .child(format!("{} › {}", s::PAGE_SETTINGS, self.section.label())),
            )
            .child(
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(self.description()),
            );
        let handles = self.blocks.clone();
        h_flex()
            .id("settings-page")
            .track_focus(&self.focus)
            .size_full()
            .on_action(cx.listener(engines::on_engine_op))
            .on_action(cx.listener(engines::on_rescan))
            .child(
                v_flex()
                    .id("settings-content")
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    .p_4()
                    .gap_4()
                    // The column is capped and centred in the space right of the sidebar.
                    .items_center()
                    .child(header)
                    .children(blocks.into_iter().zip(handles).enumerate().map(
                        |(ix, (block, handle))| {
                            div()
                                .id(("settings-block", ix))
                                .track_focus(&handle)
                                .w_full()
                                .max_w(CONTENT_MAX_W)
                                .child(block)
                        },
                    )),
            )
    }
}
