//! Image dialogs: Pull (IMG-004), Run (IMG-005, KBD-072), Tag (IMG-011). Each is a
//! [`FormView`] opened with [`open_form_dialog`]: first field focused on open, Enter /
//! `Mod+Enter` submit, Esc cancels, errors shown inline.

use dk_core::{Capabilities, EngineId};
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::{ActiveTheme, Sizable, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::{
    Action, App, AppContext, Context, Entity, FocusHandle, Focusable, IntoElement, Render,
    SharedString, Task, WeakEntity, Window, div,
};

use super::forms::{self, RunInput};
use super::ops;
use crate::actions::Navigate;
use crate::nav::{ContainerTab, Route};
use crate::pages::resources::pull::PullManager;
use crate::state::{AppState, Collection, EngineStore};
use crate::strings as s;
use crate::ui::form_dialog::{
    FormView, close, field, footer, form_error, form_root, open_form_dialog,
};
use crate::ui::kv_rows::{KvRows, KvRowsSpec};
use crate::ui::notify;

/// Dispatches `action` from `invoker` (a handle in the page) so it reaches the shell even
/// though the dialog's own focus isn't inside the shell (KBD-007/KBD-002).
fn dispatch_from(
    invoker: &Option<FocusHandle>,
    action: Box<dyn Action>,
    window: &mut Window,
    cx: &mut App,
) {
    match invoker {
        Some(h) => h.dispatch_action(action.as_ref(), window, cx),
        None => window.dispatch_action(action, cx),
    }
}

// ── Pull (IMG-004) ──────────────────────────────────────────────────────────────────────

pub struct PullDialog {
    engine: EngineId,
    caps: Capabilities,
    store: WeakEntity<EngineStore>,
    reference: Entity<InputState>,
    error: Option<SharedString>,
    cancel_focus: FocusHandle,
    ok_focus: FocusHandle,
    focus_first: bool,
}

impl PullDialog {
    pub fn open(
        engine: EngineId,
        caps: Capabilities,
        store: WeakEntity<EngineStore>,
        prefill: Option<String>,
        window: &mut Window,
        cx: &mut App,
    ) -> Entity<PullDialog> {
        let view = cx.new(|cx| {
            let reference = cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder("nginx:latest")
                    .default_value(prefill.unwrap_or_default())
            });
            PullDialog {
                engine,
                caps,
                store,
                reference,
                error: None,
                cancel_focus: cx.focus_handle(),
                ok_focus: cx.focus_handle(),
                focus_first: true,
            }
        });
        open_form_dialog(s::PULL_IMAGE_TITLE, 440., view.clone(), window, cx);
        view
    }

    pub fn reference_input(&self) -> &Entity<InputState> {
        &self.reference
    }

    pub fn error(&self) -> Option<&SharedString> {
        self.error.as_ref()
    }

    /// Validates, hands the pull to the app-wide [`PullManager`] (it outlives this dialog,
    /// SHL-012), and closes.
    pub fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let reference = self.reference.read(cx).value().trim().to_owned();
        if let Err(e) = forms::check_reference(&reference) {
            self.error = Some(e.into());
            cx.notify();
            return;
        }
        let store = self.store.clone();
        let window_handle = window.window_handle();
        let manager = PullManager::global(cx);
        let (engine, caps) = (self.engine.clone(), self.caps);
        manager.update(cx, |m, cx| {
            m.start(
                engine,
                reference,
                caps,
                window_handle,
                move |_, cx| {
                    store
                        .update(cx, |s, cx| s.refetch(Collection::Images, cx))
                        .ok();
                },
                cx,
            )
        });
        close(window, cx);
    }
}

impl FormView for PullDialog {
    fn submit_key(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.cancel_focus.is_focused(window) {
            close(window, cx);
        } else {
            self.submit(window, cx);
        }
    }
}

impl Render for PullDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if std::mem::take(&mut self.focus_first) {
            self.reference.update(cx, |i, cx| i.focus(window, cx));
        }
        let this2 = cx.entity().downgrade();
        form_root("pull-dialog", cx)
            .child(field(
                s::PULL_REFERENCE,
                Input::new(&self.reference).small(),
                Some(s::PULL_REFERENCE_HINT),
                self.error.clone(),
                cx,
            ))
            .child(footer(
                &self.cancel_focus,
                &self.ok_focus,
                s::PULL,
                false,
                close,
                move |window, cx| {
                    this2.update(cx, |d, cx| d.submit(window, cx)).ok();
                },
                window,
                cx,
            ))
    }
}

// ── Run (IMG-005) ───────────────────────────────────────────────────────────────────────

pub const PORTS_SPEC: KvRowsSpec = KvRowsSpec {
    id: "run-ports",
    title: s::PORTS,
    key_placeholder: s::PORT_HOST,
    value_placeholder: s::PORT_CONTAINER,
    separator: ":",
    add_label: s::ADD_PORT,
    flag: None,
};

pub const ENV_SPEC: KvRowsSpec = KvRowsSpec {
    id: "run-env",
    title: s::ENV_VARS,
    key_placeholder: s::ENV_KEY,
    value_placeholder: s::ENV_VALUE,
    separator: "=",
    add_label: s::ADD_ENV,
    flag: None,
};

pub const MOUNTS_SPEC: KvRowsSpec = KvRowsSpec {
    id: "run-mounts",
    title: s::MOUNTS,
    key_placeholder: s::MOUNT_SOURCE,
    value_placeholder: s::MOUNT_TARGET,
    separator: ":",
    add_label: s::ADD_MOUNT,
    flag: Some(s::READ_ONLY),
};

pub struct RunDialog {
    engine: EngineId,
    image: String,
    store: WeakEntity<EngineStore>,
    invoker: Option<FocusHandle>,
    name: Entity<InputState>,
    name_error: Option<SharedString>,
    ports: Entity<KvRows>,
    env: Entity<KvRows>,
    mounts: Entity<KvRows>,
    auto_remove: bool,
    auto_remove_focus: FocusHandle,
    error: Option<SharedString>,
    busy: bool,
    task: Option<Task<()>>,
    cancel_focus: FocusHandle,
    ok_focus: FocusHandle,
    focus_first: bool,
}

impl RunDialog {
    pub fn open(
        engine: EngineId,
        image: String,
        store: WeakEntity<EngineStore>,
        window: &mut Window,
        cx: &mut App,
    ) -> Entity<RunDialog> {
        let invoker = window.focused(cx);
        let title = s::run_title(&image);
        let view = cx.new(|cx| RunDialog {
            engine,
            image,
            store,
            invoker,
            name: cx.new(|cx| InputState::new(window, cx).placeholder(s::OPTIONAL_RANDOM)),
            name_error: None,
            ports: cx.new(|cx| KvRows::new(PORTS_SPEC, cx)),
            env: cx.new(|cx| KvRows::new(ENV_SPEC, cx)),
            mounts: cx.new(|cx| KvRows::new(MOUNTS_SPEC, cx)),
            auto_remove: false,
            auto_remove_focus: cx.focus_handle(),
            error: None,
            busy: false,
            task: None,
            cancel_focus: cx.focus_handle(),
            ok_focus: cx.focus_handle(),
            focus_first: true,
        });
        open_form_dialog(title, 560., view.clone(), window, cx);
        view
    }

    pub fn name_input(&self) -> &Entity<InputState> {
        &self.name
    }
    pub fn ports(&self) -> &Entity<KvRows> {
        &self.ports
    }
    pub fn env(&self) -> &Entity<KvRows> {
        &self.env
    }
    pub fn mounts(&self) -> &Entity<KvRows> {
        &self.mounts
    }
    pub fn error(&self) -> Option<&SharedString> {
        self.error.as_ref()
    }
    pub fn set_auto_remove(&mut self, v: bool, cx: &mut Context<Self>) {
        self.auto_remove = v;
        cx.notify();
    }

    /// Validates and runs; on success navigates to the new container (IMG-005), on failure
    /// shows the engine error inline and keeps the dialog open.
    pub fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        let name = self.name.read(cx).value().trim().to_owned();
        let ports = self.ports.read(cx).values(cx);
        let env = self.env.read(cx).values(cx);
        let mounts = self.mounts.read(cx).values(cx);
        let result = forms::run_spec(RunInput {
            image: &self.image,
            name: &name,
            ports: &ports,
            env: &env,
            mounts: &mounts,
            auto_remove: self.auto_remove,
        });
        let errs = |v: &[Option<&'static str>]| -> Vec<Option<SharedString>> {
            v.iter().map(|e| e.map(SharedString::from)).collect()
        };
        let spec = match result {
            Ok(spec) => {
                self.name_error = None;
                for rows in [&self.ports, &self.env, &self.mounts] {
                    rows.update(cx, |r, cx| {
                        let n = r.len();
                        r.set_errors(vec![None; n], cx)
                    });
                }
                spec
            }
            Err(e) => {
                self.name_error = e.name.map(Into::into);
                self.ports
                    .update(cx, |r, cx| r.set_errors(errs(&e.ports), cx));
                self.env.update(cx, |r, cx| r.set_errors(errs(&e.env), cx));
                self.mounts
                    .update(cx, |r, cx| r.set_errors(errs(&e.mounts), cx));
                cx.notify();
                return;
            }
        };
        self.busy = true;
        self.error = None;
        cx.notify();
        let hub = AppState::hub(cx);
        let call = ops::run(&hub, &self.engine, spec);
        let image = self.image.clone();
        self.task = Some(cx.spawn_in(window, async move |this, cx| {
            let result = call.await;
            this.update_in(cx, |this, window, cx| {
                this.busy = false;
                match result {
                    Ok(id) => {
                        this.store
                            .update(cx, |s, cx| s.refetch(Collection::Containers, cx))
                            .ok();
                        let invoker = this.invoker.clone();
                        close(window, cx);
                        dispatch_from(
                            &invoker,
                            Box::new(Navigate {
                                route: Route::ContainerDetail {
                                    id,
                                    tab: ContainerTab::Overview,
                                },
                            }),
                            window,
                            cx,
                        );
                    }
                    Err(e) => {
                        this.error = Some(format!("{}: {e}", s::run_failed(&image)).into());
                        cx.notify();
                    }
                }
            })
            .ok();
        }));
    }
}

impl FormView for RunDialog {
    fn submit_key(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.cancel_focus.is_focused(window) {
            close(window, cx);
            return;
        }
        if self.auto_remove_focus.is_focused(window) {
            self.auto_remove = !self.auto_remove;
            cx.notify();
            return;
        }
        for rows in [self.ports.clone(), self.env.clone(), self.mounts.clone()] {
            if rows.update(cx, |r, cx| r.activate_focused(window, cx)) {
                return;
            }
        }
        self.submit(window, cx);
    }
}

impl Render for RunDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if std::mem::take(&mut self.focus_first) {
            self.name.update(cx, |i, cx| i.focus(window, cx));
        }
        let this = cx.entity().downgrade();
        let auto_remove = self.auto_remove;
        form_root("run-dialog", cx)
            .child(field(
                s::CONTAINER_NAME,
                Input::new(&self.name).small(),
                None,
                self.name_error.clone(),
                cx,
            ))
            .child(
                v_flex()
                    .id("run-rows")
                    .gap_3()
                    .max_h(gpui_kit::px(360.))
                    .overflow_y_scroll()
                    .child(self.ports.clone())
                    .child(self.env.clone())
                    .child(self.mounts.clone()),
            )
            .child(
                div()
                    .id("auto-remove-wrap")
                    .track_focus(&self.auto_remove_focus.clone().tab_stop(true))
                    .rounded(cx.theme().radius)
                    .on_key_down(cx.listener(|this, e: &gpui_kit::KeyDownEvent, _, cx| {
                        if e.keystroke.key == "space" && !e.keystroke.modifiers.modified() {
                            this.auto_remove = !this.auto_remove;
                            cx.stop_propagation();
                            cx.notify();
                        }
                    }))
                    .child(
                        Checkbox::new("auto-remove")
                            .label(s::REMOVE_WHEN_STOPPED)
                            .checked(auto_remove)
                            .tab_stop(false)
                            .on_click(cx.listener(|this, v: &bool, _, cx| {
                                this.auto_remove = *v;
                                cx.notify();
                            })),
                    ),
            )
            .children(form_error(self.error.clone(), cx))
            .child(footer(
                &self.cancel_focus,
                &self.ok_focus,
                s::START,
                self.busy,
                close,
                move |window, cx| {
                    this.update(cx, |d, cx| d.submit(window, cx)).ok();
                },
                window,
                cx,
            ))
    }
}

// ── Tag (IMG-011) ───────────────────────────────────────────────────────────────────────

pub struct TagDialog {
    engine: EngineId,
    image_id: String,
    store: WeakEntity<EngineStore>,
    repo: Entity<InputState>,
    tag: Entity<InputState>,
    error: Option<SharedString>,
    busy: bool,
    task: Option<Task<()>>,
    cancel_focus: FocusHandle,
    ok_focus: FocusHandle,
    focus_first: bool,
}

impl TagDialog {
    pub fn open(
        engine: EngineId,
        image_id: String,
        repo_hint: Option<String>,
        store: WeakEntity<EngineStore>,
        window: &mut Window,
        cx: &mut App,
    ) -> Entity<TagDialog> {
        let view = cx.new(|cx| TagDialog {
            engine,
            image_id,
            store,
            repo: cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder("myrepo/app")
                    .default_value(repo_hint.unwrap_or_default())
            }),
            tag: cx.new(|cx| InputState::new(window, cx).placeholder("latest")),
            error: None,
            busy: false,
            task: None,
            cancel_focus: cx.focus_handle(),
            ok_focus: cx.focus_handle(),
            focus_first: true,
        });
        open_form_dialog(s::TAG_IMAGE_TITLE, 440., view.clone(), window, cx);
        view
    }

    pub fn inputs(&self) -> (&Entity<InputState>, &Entity<InputState>) {
        (&self.repo, &self.tag)
    }

    pub fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        let repo = self.repo.read(cx).value().to_string();
        let tag = self.tag.read(cx).value().to_string();
        let (repo, tag) = match forms::tag_target(&repo, &tag) {
            Ok(v) => v,
            Err(e) => {
                self.error = Some(e.into());
                cx.notify();
                return;
            }
        };
        self.busy = true;
        self.error = None;
        cx.notify();
        let hub = AppState::hub(cx);
        let reference = format!("{repo}:{tag}");
        let call = ops::tag(&hub, &self.engine, self.image_id.clone(), repo, tag);
        self.task = Some(cx.spawn_in(window, async move |this, cx| {
            let result = call.await;
            this.update_in(cx, |this, window, cx| {
                this.busy = false;
                match result {
                    Ok(()) => {
                        this.store
                            .update(cx, |s, cx| s.refetch(Collection::Images, cx))
                            .ok();
                        close(window, cx);
                        notify::success(window, cx, s::tagged(&reference));
                    }
                    Err(e) => {
                        this.error = Some(e.to_string().into());
                        cx.notify();
                    }
                }
            })
            .ok();
        }));
    }
}

impl FormView for TagDialog {
    fn submit_key(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.cancel_focus.is_focused(window) {
            close(window, cx);
        } else {
            self.submit(window, cx);
        }
    }
}

impl Render for TagDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if std::mem::take(&mut self.focus_first) {
            self.repo.update(cx, |i, cx| i.focus(window, cx));
        }
        let this = cx.entity().downgrade();
        form_root("tag-dialog", cx)
            .child(field(
                s::REPOSITORY,
                Input::new(&self.repo).small(),
                None,
                None,
                cx,
            ))
            .child(field(s::TAG, Input::new(&self.tag).small(), None, None, cx))
            .children(form_error(self.error.clone(), cx))
            .child(footer(
                &self.cancel_focus,
                &self.ok_focus,
                s::TAG,
                self.busy,
                close,
                move |window, cx| {
                    this.update(cx, |d, cx| d.submit(window, cx)).ok();
                },
                window,
                cx,
            ))
    }
}

impl Focusable for RunDialog {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.name.focus_handle(cx)
    }
}
