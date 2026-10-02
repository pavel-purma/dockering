//! Terminal tab (TRM-001, 004…009, 011/012, KBD-060…063).
//!
//! - Not running → "Start the container to open a terminal" + *Start* (TRM-001).
//! - Sub-tabs (TRM-007): `+` / `Mod+Shift+T` new, `Mod+Shift+W` close, `Ctrl+PgUp/PgDn`
//!   switch. Each sub-tab is a `dk_terminal::TerminalView` wired to a hub `TerminalHandle`
//!   (output → `feed`, `Input` → `TermCmd::Data`, `Resize` → `TermCmd::Resize` only with
//!   `EXEC_RESIZE` (TRM-005), exit → `set_exited`, `ReconnectRequested` → a new session in
//!   the same sub-tab (TRM-006)).
//! - Shell picker (auto / sh / bash / zsh / ash / custom) + optional user (TRM-004).
//! - Sessions survive leaving the tab or page for 10 minutes through
//!   [`crate::state::TerminalRegistry`] (TRM-008).
//! - `Mod+Shift+F6` (macOS also `Cmd+Esc`) leaves the terminal to the detail tab bar (KBD-061).

use dk_core::{Capabilities, EngineId, ExecRequest};
use dk_hub::{HubHandle, TermCmd};
use dk_terminal::{TerminalConfig, TerminalEvent, TerminalView};
use futures::StreamExt;
use futures::channel::mpsc;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::tab::{Tab, TabBar};
use gpui_kit::component::{ActiveTheme, Disableable, IconName, Sizable, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::{
    Action as _, AnyElement, App, AppContext, Context, Entity, FocusHandle, Focusable, IntoElement,
    Render, Subscription, Task, Window, div, px,
};

use super::state::{ContainerDetailState, DetailEvent};
use crate::actions::term_ext::{OpenExternal, Reconnect, SelectSession};
use crate::actions::{container, term};
use crate::keymap::ctx;
use crate::state::{AppState, ParkedSessions, TerminalRegistry};
use crate::strings as s;
use crate::ui::notify;

/// Shell choices (TRM-004).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shell {
    Auto,
    Sh,
    Bash,
    Zsh,
    Ash,
    Custom,
}

impl Shell {
    pub const ALL: [Shell; 6] = [
        Shell::Auto,
        Shell::Sh,
        Shell::Bash,
        Shell::Zsh,
        Shell::Ash,
        Shell::Custom,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Shell::Auto => s::SHELL_AUTO,
            Shell::Sh => "sh",
            Shell::Bash => "bash",
            Shell::Zsh => "zsh",
            Shell::Ash => "ash",
            Shell::Custom => s::SHELL_CUSTOM,
        }
    }

    /// From `config.terminal.default_shell` (empty = auto).
    pub fn from_config(v: &str) -> (Shell, String) {
        match v.trim() {
            "" | "auto" => (Shell::Auto, String::new()),
            "sh" | "/bin/sh" => (Shell::Sh, String::new()),
            "bash" | "/bin/bash" => (Shell::Bash, String::new()),
            "zsh" | "/bin/zsh" => (Shell::Zsh, String::new()),
            "ash" | "/bin/ash" => (Shell::Ash, String::new()),
            other => (Shell::Custom, other.to_owned()),
        }
    }

    /// The exec argv (no shell interpolation of user input, NFR-022: a custom command is
    /// split on whitespace into argv).
    pub fn argv(self, custom: &str) -> Vec<String> {
        match self {
            Shell::Auto => ExecRequest::default_shell_cmd(),
            Shell::Sh => vec!["/bin/sh".into()],
            Shell::Bash => vec!["/bin/bash".into()],
            Shell::Zsh => vec!["/bin/zsh".into()],
            Shell::Ash => vec!["/bin/ash".into()],
            Shell::Custom => {
                let v: Vec<String> = custom.split_whitespace().map(str::to_owned).collect();
                if v.is_empty() {
                    ExecRequest::default_shell_cmd()
                } else {
                    v
                }
            }
        }
    }
}

/// One sub-tab: the terminal view and its hub session (TRM-007).
pub struct Session {
    pub view: Entity<TerminalView>,
    pub shell: Shell,
    pub custom: String,
    pub user: String,
    /// `None` while connecting / after the process exited.
    input: Option<mpsc::Sender<TermCmd>>,
    pub hub_id: Option<u64>,
    pub connecting: bool,
    /// Output pump + exit watcher; replaced on reconnect (cancels the old one).
    io: Option<Task<()>>,
    open: Option<Task<()>>,
    _subs: Vec<Subscription>,
    /// Bumped per (re)connect; stale results are dropped (NFR-005).
    generation: u64,
}

impl Session {
    fn send(&mut self, cmd: TermCmd) {
        if let Some(tx) = self.input.as_mut()
            && tx.try_send(cmd).is_err()
        {
            tracing::debug!("terminal input queue full or closed");
        }
    }

    /// Tells the hub actor to close (it ends the process) and stops pumping output.
    fn close(&mut self) {
        self.send(TermCmd::Close);
        self.input = None;
        self.io = None;
        self.open = None;
    }

    /// `close` + mark the view ended (it may stay visible, read-only).
    fn close_and_mark(&mut self, cx: &mut App) {
        self.close();
        self.view.update(cx, |v, cx| {
            if !v.is_exited() {
                v.set_exited(None, cx);
            }
        });
    }
}

/// The sessions of one container: parked in the registry while the tab is unmounted.
pub struct Sessions {
    pub list: Vec<Session>,
    pub active: usize,
}

impl ParkedSessions for Sessions {
    fn close(&mut self, cx: &mut App) {
        for s in &mut self.list {
            s.close_and_mark(cx);
        }
        self.list.clear();
    }

    fn into_any(self: Box<Self>) -> Box<dyn std::any::Any> {
        self
    }
}

pub struct TerminalTab {
    state: Entity<ContainerDetailState>,
    engine: EngineId,
    container: String,
    hub: HubHandle,
    sessions: Vec<Session>,
    active: usize,
    tab_bar: FocusHandle,
    sub_tabs_focus: FocusHandle,
    shell: Shell,
    shell_focus: FocusHandle,
    user: Entity<InputState>,
    custom: Entity<InputState>,
    /// Focus the active terminal after the next render (new session, sub-tab switch).
    focus_pending: bool,
    /// The tab was reached by arrowing through the detail tab bar: don't steal its focus.
    keep_tab_bar_focus: bool,
    external_task: Option<Task<()>>,
    _subs: Vec<Subscription>,
}

impl TerminalTab {
    pub fn new(
        state: Entity<ContainerDetailState>,
        tab_bar: FocusHandle,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let (engine, container) = {
            let st = state.read(cx);
            (st.engine().clone(), st.id().to_owned())
        };
        let cfg = AppState::config(cx).terminal.clone();
        let (shell, custom_cmd) = Shell::from_config(&cfg.default_shell);
        let user = cx.new(|cx| InputState::new(window, cx).placeholder(s::TERMINAL_USER));
        let custom = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(s::TERMINAL_CUSTOM_CMD)
                .default_value(custom_cmd)
        });
        let subs = vec![
            cx.observe(&state, |_, _, cx| cx.notify()),
            cx.subscribe_in(&state, window, Self::on_detail_event),
            // TRM-008: unmounting (leaving the page) parks the sessions in the registry.
            cx.on_release(|this: &mut Self, cx| this.park(cx)),
        ];
        let mut this = Self {
            state,
            hub: AppState::hub(cx),
            engine,
            container,
            sessions: Vec::new(),
            active: 0,
            tab_bar,
            sub_tabs_focus: cx.focus_handle().tab_stop(true),
            shell,
            shell_focus: cx.focus_handle().tab_stop(true),
            user,
            custom,
            focus_pending: false,
            keep_tab_bar_focus: false,
            external_task: None,
            _subs: subs,
        };
        // Re-attach parked sessions (TRM-008).
        if let Some(parked) = TerminalRegistry::take(&this.engine, &this.container, cx) {
            this.adopt(parked, window, cx);
        }
        this
    }

    // ── accessors (tests) ──────────────────────────────────────────────────────────────

    pub fn sessions(&self) -> &[Session] {
        &self.sessions
    }
    pub fn active(&self) -> usize {
        self.active
    }
    pub fn active_view(&self) -> Option<&Entity<TerminalView>> {
        self.sessions.get(self.active).map(|s| &s.view)
    }
    pub fn set_shell(&mut self, shell: Shell) {
        self.shell = shell;
    }

    /// The detail page tells us the tab bar keeps focus (arrow-key tab switching, KBD-040).
    pub fn keep_tab_bar_focus(&mut self) {
        self.keep_tab_bar_focus = true;
        self.focus_pending = false;
    }

    fn caps(&self, cx: &App) -> Capabilities {
        self.state.read(cx).capabilities(cx)
    }

    fn external_available(&self, cx: &App) -> bool {
        self.state.read(cx).external_terminal_available(cx)
    }

    fn running(&self, cx: &App) -> bool {
        self.state.read(cx).is_running(cx)
    }

    fn terminal_config(cx: &App) -> TerminalConfig {
        let c = &AppState::config(cx).terminal;
        TerminalConfig {
            font_family: c.font_family.clone(),
            font_size: if c.font_size > 0.0 { c.font_size } else { 13.0 },
            scrollback_lines: c.scrollback_lines as usize,
        }
    }

    // ── sessions ───────────────────────────────────────────────────────────────────────

    /// Opens the first session when the tab is shown on a running container.
    /// Opens the first session when the tab is shown on a running container. Focus moves
    /// into it unless the user is arrowing through the detail tab bar (KBD-040/007).
    pub fn ensure_session(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.sessions.is_empty()
            && self.running(cx)
            && self.caps(cx).contains(Capabilities::EXEC_TTY)
            && !self.state.read(cx).is_removed()
        {
            let keep_bar = std::mem::replace(&mut self.keep_tab_bar_focus, false);
            self.new_session(window, cx);
            if keep_bar {
                self.focus_pending = false;
            }
        }
    }

    fn new_session(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.running(cx) || !self.caps(cx).contains(Capabilities::EXEC_TTY) {
            return;
        }
        let config = Self::terminal_config(cx);
        let view = cx.new(|cx| TerminalView::new(config, window, cx));
        let sub = cx.subscribe_in(&view, window, Self::on_terminal_event);
        let custom = self.custom.read(cx).value().to_string();
        let user = self.user.read(cx).value().trim().to_owned();
        self.sessions.push(Session {
            view,
            shell: self.shell,
            custom,
            user,
            input: None,
            hub_id: None,
            connecting: false,
            io: None,
            open: None,
            _subs: vec![sub],
            generation: 0,
        });
        self.active = self.sessions.len() - 1;
        let ix = self.active;
        self.connect(ix, cx);
        self.focus_pending = true;
        cx.notify();
    }

    /// (Re)connects session `ix` with its shell/user (TRM-004/006).
    fn connect(&mut self, ix: usize, cx: &mut Context<Self>) {
        let resize = self.caps(cx).contains(Capabilities::EXEC_RESIZE);
        let Some(sess) = self.sessions.get_mut(ix) else {
            return;
        };
        sess.close();
        sess.generation += 1;
        let generation = sess.generation;
        sess.connecting = true;
        let (cols, rows) = sess.view.read(cx).size();
        let req = ExecRequest {
            cmd: sess.shell.argv(&sess.custom),
            user: (!sess.user.is_empty()).then(|| sess.user.clone()),
            cols,
            rows,
            ..Default::default()
        };
        let view = sess.view.clone();
        view.update(cx, |v, cx| v.reset(cx));
        let call = self.hub.open_terminal(&self.engine, &self.container, req);
        let _ = resize;
        sess.open = Some(cx.spawn(async move |this, cx| {
            let result = call.await;
            this.update(cx, |this, cx| {
                let Some(ix) = this.index_of_generation(&view, generation) else {
                    return; // closed or reconnected meanwhile
                };
                match result {
                    Ok(handle) => this.attach(ix, handle, cx),
                    Err(e) => {
                        let sess = &mut this.sessions[ix];
                        sess.connecting = false;
                        let msg = format!("\r\n{}: {e}\r\n", s::TERMINAL_OPEN_FAILED);
                        view.update(cx, |v, cx| {
                            v.feed(msg.as_bytes(), cx);
                            v.set_exited(None, cx);
                        });
                    }
                }
                cx.notify();
            })
            .ok();
        }));
    }

    fn index_of_generation(&self, view: &Entity<TerminalView>, generation: u64) -> Option<usize> {
        self.sessions
            .iter()
            .position(|s| s.view == *view && s.generation == generation)
    }

    /// Wires a hub `TerminalHandle` to session `ix`: output → `feed` (the view coalesces at
    /// 4 ms, TRM-010), exit → `set_exited`.
    fn attach(&mut self, ix: usize, handle: dk_hub::TerminalHandle, cx: &mut Context<Self>) {
        let resize = self.caps(cx).contains(Capabilities::EXEC_RESIZE);
        let sess = &mut self.sessions[ix];
        sess.connecting = false;
        sess.hub_id = Some(handle.session_id);
        let mut input = handle.input;
        // The view may have been resized while connecting (TRM-005).
        if resize {
            let (cols, rows) = sess.view.read(cx).size();
            let _ = input.try_send(TermCmd::Resize { cols, rows });
        }
        sess.input = Some(input);
        let view = sess.view.clone();
        let mut output = handle.output;
        let exit = handle.exit;
        sess.io = Some(cx.spawn(async move |_, cx| {
            while let Some(item) = output.next().await {
                match item {
                    Ok(bytes) => view.update(cx, |v, cx| v.feed(&bytes, cx)),
                    Err(e) => {
                        tracing::debug!(%e, "terminal output error");
                        break;
                    }
                }
            }
            let code = exit.await.ok().flatten();
            view.update(cx, |v, cx| v.set_exited(code, cx));
        }));
    }

    fn on_terminal_event(
        &mut self,
        view: &Entity<TerminalView>,
        event: &TerminalEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(ix) = self.sessions.iter().position(|s| s.view == *view) else {
            return;
        };
        match event {
            TerminalEvent::Input(bytes) => self.sessions[ix].send(TermCmd::Data(bytes.clone())),
            TerminalEvent::Resize { cols, rows } => {
                if self.caps(cx).contains(Capabilities::EXEC_RESIZE) {
                    self.sessions[ix].send(TermCmd::Resize {
                        cols: *cols,
                        rows: *rows,
                    });
                }
            }
            TerminalEvent::ReconnectRequested => {
                if self.running(cx) {
                    self.connect(ix, cx);
                }
            }
            TerminalEvent::Title(_) | TerminalEvent::Bell => {}
        }
    }

    fn close_session(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        if ix >= self.sessions.len() {
            return;
        }
        let mut sess = self.sessions.remove(ix);
        sess.close();
        if self.active >= self.sessions.len() {
            self.active = self.sessions.len().saturating_sub(1);
        }
        if self.sessions.is_empty() {
            // Focus returns to the tab bar (KBD-007).
            window.focus(&self.tab_bar, cx);
        } else {
            self.focus_pending = true;
        }
        cx.notify();
    }

    fn adopt(
        &mut self,
        parked: Box<dyn ParkedSessions>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Ok(sessions) = parked.into_any().downcast::<Sessions>() else {
            return;
        };
        let Sessions { list, active } = *sessions;
        self.sessions = list;
        self.active = active.min(self.sessions.len().saturating_sub(1));
        // Re-subscribe to the views' events with this entity (the old subscriptions died
        // with the previous tab).
        for s in &mut self.sessions {
            s._subs = vec![cx.subscribe_in(&s.view, window, Self::on_terminal_event)];
        }
        self.focus_pending = !self.sessions.is_empty();
    }

    /// TRM-008: hand the sessions to the registry (10-minute idle timer).
    fn park(&mut self, cx: &mut App) {
        if self.sessions.is_empty() || self.state.read(cx).is_removed() {
            for s in &mut self.sessions {
                s.close();
            }
            return;
        }
        let list = std::mem::take(&mut self.sessions);
        let parked = Sessions {
            list,
            active: self.active,
        };
        TerminalRegistry::park(&self.engine, &self.container, Box::new(parked), cx);
    }

    fn on_detail_event(
        &mut self,
        _: &Entity<ContainerDetailState>,
        event: &DetailEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if *event == DetailEvent::Removed {
            // CDT-080: terminals become read-only and close.
            for s in &mut self.sessions {
                s.close();
                s.view.update(cx, |v, cx| v.set_read_only(true, cx));
            }
            cx.notify();
        }
    }

    // ── actions ────────────────────────────────────────────────────────────────────────

    fn on_new(&mut self, _: &term::NewSession, window: &mut Window, cx: &mut Context<Self>) {
        self.new_session(window, cx);
    }

    fn on_close(&mut self, _: &term::CloseSession, window: &mut Window, cx: &mut Context<Self>) {
        let ix = self.active;
        self.close_session(ix, window, cx);
    }

    fn step(&mut self, delta: isize, cx: &mut Context<Self>) {
        let n = self.sessions.len() as isize;
        if n == 0 {
            return;
        }
        self.active = ((self.active as isize + delta).rem_euclid(n)) as usize;
        self.focus_pending = true;
        cx.notify();
    }

    fn on_next(&mut self, _: &term::NextSession, _: &mut Window, cx: &mut Context<Self>) {
        self.step(1, cx);
    }

    fn on_prev(&mut self, _: &term::PrevSession, _: &mut Window, cx: &mut Context<Self>) {
        self.step(-1, cx);
    }

    fn on_select(&mut self, a: &SelectSession, _: &mut Window, cx: &mut Context<Self>) {
        if a.ix < self.sessions.len() {
            self.active = a.ix;
            self.focus_pending = true;
            cx.notify();
        }
    }

    /// Restart the active sub-tab with the current shell / user choice (TRM-004).
    fn on_reconnect(&mut self, _: &Reconnect, window: &mut Window, cx: &mut Context<Self>) {
        let custom = self.custom.read(cx).value().to_string();
        let user = self.user.read(cx).value().trim().to_owned();
        let shell = self.shell;
        let ix = self.active;
        match self.sessions.get_mut(ix) {
            Some(sess) => {
                sess.shell = shell;
                sess.custom = custom;
                sess.user = user;
                self.connect(ix, cx);
                self.focus_pending = true;
            }
            None => self.new_session(window, cx),
        }
        cx.notify();
    }

    /// TRM-009 (optional): launch the configured terminal app with `docker exec -it <id> sh`.
    /// v1 limitation: only for engines whose transport the host Docker CLI reaches
    /// ([`HOST_DOCKER_EXEC_TRANSPORTS`]); otherwise the action is not offered and is a no-op.
    fn on_external(&mut self, _: &OpenExternal, window: &mut Window, cx: &mut Context<Self>) {
        if !self.external_available(cx) {
            // Reachable from the command palette even where the button/menu item is hidden.
            notify::info(window, cx, s::TERMINAL_EXTERNAL_UNAVAILABLE);
            return;
        }
        let template = AppState::config(cx).terminal.external_terminal.clone();
        let Some(argv) = external_argv(&template, &self.container) else {
            return;
        };
        let call = self.hub.launch(argv);
        self.external_task = Some(cx.spawn_in(window, async move |_, cx| {
            if let Err(e) = call.await {
                cx.update(|w, cx| notify::engine_error(w, cx, s::TERMINAL_EXTERNAL_FAILED, &e))
                    .ok();
            }
        }));
    }

    fn cycle_shell(&mut self, delta: isize, cx: &mut Context<Self>) {
        let all = Shell::ALL;
        let ix = all.iter().position(|s| *s == self.shell).unwrap_or(0) as isize;
        self.shell = all[((ix + delta).rem_euclid(all.len() as isize)) as usize];
        cx.notify();
    }

    // ── rendering ──────────────────────────────────────────────────────────────────────

    fn render_not_running(&self, cx: &mut Context<Self>) -> AnyElement {
        let removed = self.state.read(cx).is_removed();
        let supported = self.caps(cx).contains(Capabilities::EXEC_TTY);
        let msg = if supported {
            s::TERMINAL_START_HINT
        } else {
            s::TERMINAL_UNSUPPORTED
        };
        v_flex()
            .id("terminal-not-running")
            .size_full()
            .items_center()
            .justify_center()
            .gap_3()
            .child(
                div()
                    .text_color(cx.theme().muted_foreground)
                    .child(IconName::SquareTerminal),
            )
            .child(div().text_sm().child(msg))
            .when(supported && !removed, |this| {
                this.child(
                    Button::new("terminal-start")
                        .primary()
                        .icon(IconName::Play)
                        .label(s::ACTION_START)
                        .tooltip_with_action(
                            s::ACTION_START,
                            &container::StartStop,
                            Some(ctx::DETAIL_HEADER),
                        )
                        .on_click(|_, w, cx| w.dispatch_action(Box::new(container::StartStop), cx)),
                )
            })
            .into_any_element()
    }

    fn render_toolbar(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let sub_focused = self.sub_tabs_focus.is_focused(window);
        let shell_focused = self.shell_focus.is_focused(window);
        let external = self.external_available(cx);
        let active = self.active;
        let tabs =
            TabBar::new("terminal-sessions")
                .segmented()
                .small()
                .selected_index(active)
                .on_click(cx.listener(|this, ix: &usize, _, cx| {
                    this.active = *ix;
                    this.focus_pending = true;
                    cx.notify();
                }))
                .children(self.sessions.iter().enumerate().map(|(i, sess)| {
                    Tab::new().label(s::terminal_session(i + 1, sess.shell.label()))
                }));
        let shell = self.shell;
        h_flex()
            .id("terminal-toolbar")
            .gap_2()
            .items_center()
            .flex_wrap()
            .child(
                div()
                    .id("terminal-subtabs")
                    .track_focus(&self.sub_tabs_focus)
                    .rounded(cx.theme().radius)
                    .map(|el| crate::ui::focus_ring(el, sub_focused, cx))
                    .on_key_down(cx.listener(|this, e: &gpui_kit::KeyDownEvent, _, cx| {
                        if e.keystroke.modifiers.modified() {
                            return;
                        }
                        match e.keystroke.key.as_str() {
                            "left" => this.step(-1, cx),
                            "right" => this.step(1, cx),
                            "enter" | "space" => this.focus_pending = true,
                            _ => return,
                        }
                        cx.stop_propagation();
                        cx.notify();
                    }))
                    .when(!self.sessions.is_empty(), |el| el.child(tabs)),
            )
            .child(
                Button::new("terminal-new")
                    .ghost()
                    .small()
                    .icon(IconName::Plus)
                    .tooltip_with_action(s::TERMINAL_NEW, &term::NewSession, Some(ctx::TERMINAL))
                    .on_click(|_, w, cx| w.dispatch_action(Box::new(term::NewSession), cx)),
            )
            .child(
                Button::new("terminal-close")
                    .ghost()
                    .small()
                    .icon(IconName::Close)
                    .disabled(self.sessions.is_empty())
                    .tooltip_with_action(
                        s::TERMINAL_CLOSE,
                        &term::CloseSession,
                        Some(ctx::TERMINAL),
                    )
                    .on_click(|_, w, cx| w.dispatch_action(Box::new(term::CloseSession), cx)),
            )
            .child(div().flex_1())
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(s::TERMINAL_SHELL),
            )
            .child(
                // Shell picker: a focusable segmented control (←/→), like the stats window.
                div()
                    .id("terminal-shell")
                    .track_focus(&self.shell_focus)
                    .rounded(cx.theme().radius)
                    .map(|el| crate::ui::focus_ring(el, shell_focused, cx))
                    .on_key_down(cx.listener(|this, e: &gpui_kit::KeyDownEvent, _, cx| {
                        if e.keystroke.modifiers.modified() {
                            return;
                        }
                        match e.keystroke.key.as_str() {
                            "left" => this.cycle_shell(-1, cx),
                            "right" => this.cycle_shell(1, cx),
                            _ => return,
                        }
                        cx.stop_propagation();
                    }))
                    .child(
                        TabBar::new("terminal-shell-picker")
                            .segmented()
                            .small()
                            .selected_index(
                                Shell::ALL.iter().position(|s| *s == shell).unwrap_or(0),
                            )
                            .on_click(cx.listener(|this, ix: &usize, _, cx| {
                                this.shell = Shell::ALL[*ix];
                                cx.notify();
                            }))
                            .children(Shell::ALL.iter().map(|s| Tab::new().label(s.label()))),
                    ),
            )
            .when(shell == Shell::Custom, |this| {
                this.child(div().w(px(200.)).child(Input::new(&self.custom).small()))
            })
            .child(div().w(px(140.)).child(Input::new(&self.user).small()))
            .child(
                Button::new("terminal-reconnect")
                    .small()
                    .outline()
                    .icon(IconName::RotateCw)
                    .label(s::TERMINAL_RECONNECT)
                    .tooltip(s::CMD_TERM_RECONNECT)
                    .on_click(|_, w, cx| w.dispatch_action(Box::new(Reconnect), cx)),
            )
            .when(external, |this| {
                this.child(
                    Button::new("terminal-external")
                        .small()
                        .ghost()
                        .icon(IconName::ExternalLink)
                        .tooltip(s::TERMINAL_EXTERNAL)
                        .on_click(|_, w, cx| w.dispatch_action(Box::new(OpenExternal), cx)),
                )
            })
            .into_any_element()
    }
}

/// Engine transports (`EngineInfo.transport`) where `docker exec` run on the host reaches the
/// engine: the Docker CLI talks to the same pipe / socket / TCP endpoint.
///
/// TRM-009 is optional (MAY); v1 limitation: the external terminal is only offered for these.
/// WSL bridge engines (`bridge`, needs `wsl -d <distro> docker exec`), WSLC (`com` / `cli`,
/// needs `wslc container exec`) and unknown transports hide it. This is decided from the
/// transport, never from `EngineKind` (ENG-030); a per-engine `exec_command_hint` in
/// `EngineInfo` would lift the limitation without UI changes.
pub const HOST_DOCKER_EXEC_TRANSPORTS: &[&str] = &["npipe", "unix", "tcp", "tls"];

/// Whether the external terminal (TRM-009) can be offered for an engine with `info`.
pub fn external_terminal_supported(info: Option<&dk_core::EngineInfo>) -> bool {
    info.and_then(|i| i.transport.as_deref())
        .is_some_and(|t| HOST_DOCKER_EXEC_TRANSPORTS.contains(&t))
}

/// Splits a command template into words with simple shell-like quoting: whitespace separates
/// words; `"..."` and `'...'` keep spaces (quotes removed; adjacent text joins the word, e.g.
/// `--title="My shell"`). No escapes, variables or globbing: the result is an argv, never a
/// shell string (NFR-022). Each word records whether any part was quoted, so a quoted
/// `"{cmd}"` stays a literal. `None` on an unterminated quote.
pub fn split_template(template: &str) -> Option<Vec<(String, bool)>> {
    let mut words = Vec::new();
    let mut cur = String::new();
    let mut in_word = false;
    let mut quoted = false;
    let mut quote: Option<char> = None;
    for ch in template.chars() {
        match quote {
            Some(q) if ch == q => quote = None,
            Some(_) => cur.push(ch),
            None if ch == '"' || ch == '\'' => {
                quote = Some(ch);
                in_word = true;
                quoted = true;
            }
            None if ch.is_whitespace() => {
                if in_word {
                    words.push((std::mem::take(&mut cur), quoted));
                    in_word = false;
                    quoted = false;
                }
            }
            None => {
                cur.push(ch);
                in_word = true;
            }
        }
    }
    if quote.is_some() {
        return None;
    }
    if in_word {
        words.push((cur, quoted));
    }
    Some(words)
}

/// `external_terminal` template → argv (TRM-009). The template is split with
/// [`split_template`]; an unquoted `{cmd}` word is replaced by the exec argv
/// (`docker exec -it <id> sh`); without one the exec argv is appended.
pub fn external_argv(template: &str, container: &str) -> Option<Vec<String>> {
    let exec = ["docker", "exec", "-it", container, "sh"].map(str::to_owned);
    let parts = split_template(template)?;
    if parts.is_empty() || dk_core::validate::validate_id_or_name(container).is_err() {
        return None;
    }
    let mut out = Vec::new();
    let mut placed = false;
    for (p, quoted) in parts {
        if p == "{cmd}" && !quoted {
            out.extend(exec.iter().cloned());
            placed = true;
        } else {
            out.push(p);
        }
    }
    if !placed {
        out.extend(exec);
    }
    Some(out)
}

impl Focusable for TerminalTab {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.active_view()
            .map(|v| v.focus_handle(cx))
            .unwrap_or_else(|| self.sub_tabs_focus.clone())
    }
}

impl Render for TerminalTab {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let running = self.running(cx);
        if self.sessions.is_empty() && running {
            self.ensure_session(window, cx);
        }
        if std::mem::take(&mut self.focus_pending)
            && let Some(v) = self.active_view()
        {
            let h = v.focus_handle(cx);
            window.focus(&h, cx);
        }
        let leave =
            crate::keymap::hint_for(dk_terminal::actions::LeaveTerminal.name(), ctx::TERMINAL)
                .unwrap_or_default();
        let body: AnyElement = if self.sessions.is_empty() {
            self.render_not_running(cx)
        } else {
            let views: Vec<AnyElement> = self
                .sessions
                .iter()
                .enumerate()
                .map(|(i, s)| {
                    // Keep every view mounted (alive) but only show the active one.
                    div()
                        .id(("terminal-view", i))
                        .size_full()
                        .when(i != self.active, |el| el.hidden())
                        .child(s.view.clone())
                        .into_any_element()
                })
                .collect();
            let connecting = self.sessions.get(self.active).is_some_and(|s| s.connecting);
            div()
                .size_full()
                .relative()
                .children(views)
                .when(connecting, |el| {
                    el.child(
                        div()
                            .absolute()
                            .top(px(8.))
                            .right(px(12.))
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(s::TERMINAL_CONNECTING),
                    )
                })
                .into_any_element()
        };
        let toolbar = self.render_toolbar(window, cx);
        v_flex()
            .id("terminal-tab")
            .size_full()
            .gap_2()
            .on_action(cx.listener(Self::on_new))
            .on_action(cx.listener(Self::on_close))
            .on_action(cx.listener(Self::on_next))
            .on_action(cx.listener(Self::on_prev))
            .on_action(cx.listener(Self::on_select))
            .on_action(cx.listener(Self::on_reconnect))
            .on_action(cx.listener(Self::on_external))
            .child(toolbar)
            .child(div().flex_1().min_h_0().child(body))
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(s::terminal_leave_hint(&leave)),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trm_004_shell_argv() {
        assert_eq!(Shell::Auto.argv(""), ExecRequest::default_shell_cmd());
        assert_eq!(Shell::Bash.argv(""), vec!["/bin/bash".to_owned()]);
        assert_eq!(
            Shell::Custom.argv("/bin/sh -l"),
            vec!["/bin/sh".to_owned(), "-l".to_owned()]
        );
        assert_eq!(Shell::from_config("zsh").0, Shell::Zsh);
        assert_eq!(Shell::from_config("").0, Shell::Auto);
        assert_eq!(
            Shell::from_config("fish -l"),
            (Shell::Custom, "fish -l".to_owned())
        );
    }

    #[test]
    fn trm_009_external_argv() {
        let id = "a1b2c3d4e5f6";
        assert_eq!(
            external_argv("wt.exe -- {cmd}", id),
            Some(
                ["wt.exe", "--", "docker", "exec", "-it", id, "sh"]
                    .map(str::to_owned)
                    .to_vec()
            )
        );
        assert_eq!(
            external_argv("x-terminal-emulator -e", id).map(|v| v.len()),
            Some(7)
        );
        assert_eq!(external_argv("  ", id), None);
        assert_eq!(external_argv("wt.exe {cmd}", "bad; rm -rf /"), None);
    }

    fn words(t: &str) -> Option<Vec<String>> {
        split_template(t).map(|w| w.into_iter().map(|(s, _)| s).collect())
    }

    #[test]
    fn trm_009_template_quoting() {
        // Double and single quotes keep spaces; quotes are removed; no expansion.
        assert_eq!(
            words(r#""C:\Program Files\Alacritty\alacritty.exe" -e {cmd}"#),
            Some(vec![
                r"C:\Program Files\Alacritty\alacritty.exe".to_owned(),
                "-e".into(),
                "{cmd}".into()
            ])
        );
        assert_eq!(
            words("wt.exe --title 'My shell $HOME' {cmd}"),
            Some(vec![
                "wt.exe".to_owned(),
                "--title".into(),
                "My shell $HOME".into(),
                "{cmd}".into()
            ])
        );
        // Quotes join adjacent text into one word; an empty quoted word is kept.
        assert_eq!(
            words(r#"term --title="a b"x '' {cmd}"#),
            Some(vec![
                "term".to_owned(),
                "--title=a bx".into(),
                String::new(),
                "{cmd}".into()
            ])
        );
        // The other quote character is literal inside quotes.
        assert_eq!(
            words(r#"say "it's" 'a "b"'"#),
            Some(vec!["say".to_owned(), "it's".into(), r#"a "b""#.into()])
        );
        assert_eq!(words("  a\tb  "), Some(vec!["a".to_owned(), "b".into()]));
        assert_eq!(words(r#"term "unterminated"#), None);
        assert_eq!(words("   "), Some(vec![]));
    }

    #[test]
    fn trm_009_external_argv_with_quotes() {
        let id = "a1b2c3d4e5f6";
        assert_eq!(
            external_argv(
                r#""C:\Program Files\WezTerm\wezterm.exe" start -- {cmd}"#,
                id
            ),
            Some(
                [
                    r"C:\Program Files\WezTerm\wezterm.exe",
                    "start",
                    "--",
                    "docker",
                    "exec",
                    "-it",
                    id,
                    "sh"
                ]
                .map(str::to_owned)
                .to_vec()
            )
        );
        // A quoted "{cmd}" is a literal argument; the exec argv is then appended.
        assert_eq!(
            external_argv(r#"echo "{cmd}""#, id),
            Some(
                ["echo", "{cmd}", "docker", "exec", "-it", id, "sh"]
                    .map(str::to_owned)
                    .to_vec()
            )
        );
        // Shell metacharacters are plain argv text, never interpreted.
        assert_eq!(
            external_argv("wt.exe ; rm -rf / {cmd}", id).map(|v| v[1].clone()),
            Some(";".to_owned())
        );
        assert_eq!(external_argv(r#"wt.exe "{cmd}"#, id), None, "unterminated");
    }

    fn info_with(transport: Option<&str>) -> dk_core::EngineInfo {
        dk_core::EngineInfo {
            name: "e".into(),
            kind: dk_core::EngineKind::Docker,
            transport: transport.map(str::to_owned),
            transport_note: None,
            server_version: String::new(),
            api_version: None,
            os: "linux".into(),
            arch: "amd64".into(),
            kernel: None,
            cpus: None,
            mem_total: None,
            containers: Default::default(),
            images: 0,
            storage_driver: None,
            root_dir: None,
            daemon_id: None,
            list_stats_limit: 20,
            capabilities: dk_core::Capabilities::all(),
        }
    }

    #[test]
    fn trm_009_offered_only_where_host_docker_exec_works() {
        for t in ["npipe", "unix", "tcp", "tls"] {
            assert!(
                external_terminal_supported(Some(&info_with(Some(t)))),
                "{t} should offer the external terminal"
            );
        }
        // WSL bridge, WSLC COM / CLI, unknown and missing transports: hidden (v1 limitation).
        for t in ["bridge", "com", "cli", "fake", "xpc", ""] {
            assert!(
                !external_terminal_supported(Some(&info_with(Some(t)))),
                "{t} must hide the external terminal"
            );
        }
        assert!(!external_terminal_supported(Some(&info_with(None))));
        assert!(!external_terminal_supported(None), "no info yet");
    }
}
