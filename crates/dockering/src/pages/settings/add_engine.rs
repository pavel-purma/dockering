//! *Add engine* dialog (ENG-105, KBD-071/072).
//!
//! Generic over the factories' [`EngineConfigSchema`]s (spec 21 §8): a connection-type
//! select, the name, then one control per [`ConfigField`] by field kind: Text/Path/Port →
//! `Input` (Path with *Browse…*, which opens the OS file picker on Enter/Space), Bool →
//! `Switch`, Choice → `Select`. Known values (detected WSL distros, WSLC sessions) are offered
//! as quick-pick buttons. [`mapping`](super::mapping) turns the values into an
//! [`EngineConfig`]; nothing here branches on the engine kind.
//!
//! *Test* calls `hub.test_engine`; *Save* (`add_engine`) is enabled after a successful test
//! of the same values, or through an explicit *Save anyway*. Initial focus is the first field;
//! `Mod+Enter` submits; `Esc` cancels and focus returns to the invoker.

use std::collections::HashMap;

use dk_core::{ConfigFieldKind, EngineConfig, EngineConfigSchema, EngineInfo, EngineStatus};
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::select::{Select, SelectEvent, SelectState};
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{
    ActiveTheme, Disableable, Icon, IconName, IndexPath, Sizable, h_flex, v_flex,
};
use gpui_kit::prelude::*;
use gpui_kit::{
    AnyElement, App, AppContext, Context, Entity, FocusHandle, IntoElement, PathPromptOptions,
    Render, SharedString, Subscription, Task, Window, div, px,
};

use super::mapping::{self, Values};
use crate::state::AppState;
use crate::strings as s;
use crate::ui::form_dialog::{FormView, close, field, form_error, form_root, open_form_dialog};
use crate::ui::notify;
use crate::ui::widgets::focus_wrap;

/// Outcome of *Test* for the current values.
#[derive(Debug, Clone, PartialEq)]
pub enum DialogTest {
    Running,
    Ok(Box<EngineInfo>),
    Failed {
        message: SharedString,
        hint: Option<SharedString>,
    },
}

pub struct AddEngineDialog {
    schemas: Vec<EngineConfigSchema>,
    kind_select: Entity<SelectState<Vec<SharedString>>>,
    kind: usize,
    name: Entity<InputState>,
    /// Text inputs per schema (index) and field key.
    inputs: HashMap<(usize, String), Entity<InputState>>,
    /// Choice selects per schema and field key.
    choices: HashMap<(usize, String), Entity<SelectState<Vec<SharedString>>>>,
    /// Bool values per schema and field key.
    bools: HashMap<(usize, String), bool>,
    /// Values offered as quick picks per field key.
    known: HashMap<String, Vec<String>>,
    errors: mapping::Errors,
    test: Option<DialogTest>,
    /// Values the last successful test ran with (Save is enabled while they're unchanged).
    tested: Option<EngineConfig>,
    revision: u64,
    busy: bool,
    task: Option<Task<()>>,
    pick_task: Option<Task<()>>,
    test_focus: FocusHandle,
    save_focus: FocusHandle,
    save_anyway_focus: FocusHandle,
    cancel_focus: FocusHandle,
    focus_first: bool,
    _subs: Vec<Subscription>,
}

impl AddEngineDialog {
    /// Opens the dialog over the current window. Focus returns to the invoker on close.
    pub fn open(
        engines: &[EngineStatus],
        window: &mut Window,
        cx: &mut App,
    ) -> Entity<AddEngineDialog> {
        let schemas: Vec<EngineConfigSchema> = AppState::hub(cx)
            .engine_schemas()
            .into_iter()
            .filter(mapping::addable)
            .collect();
        let engines = engines.to_vec();
        let view = cx.new(|cx| Self::new(schemas, &engines, window, cx));
        open_form_dialog(s::ADD_ENGINE_TITLE, 560., view.clone(), window, cx);
        view
    }

    fn new(
        schemas: Vec<EngineConfigSchema>,
        engines: &[EngineStatus],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let labels: Vec<SharedString> = schemas.iter().map(|s| s.label.clone().into()).collect();
        let kind_select = cx.new(|cx| {
            SelectState::new(
                labels.clone(),
                (!labels.is_empty()).then(|| IndexPath::new(0)),
                window,
                cx,
            )
        });
        let name = cx.new(|cx| InputState::new(window, cx).placeholder(s::ENGINE_NAME_PLACEHOLDER));
        let mut subs = vec![
            cx.subscribe_in(
                &kind_select,
                window,
                |this, _, e: &SelectEvent<Vec<SharedString>>, _, cx| {
                    if let SelectEvent::Confirm(Some(label)) = e
                        && let Some(ix) =
                            this.schemas.iter().position(|s| s.label == label.as_ref())
                    {
                        this.kind = ix;
                        this.values_changed(cx);
                    }
                },
            ),
            cx.subscribe_in(&name, window, |this, _, e: &InputEvent, _, cx| {
                if matches!(e, InputEvent::Change) {
                    this.values_changed(cx);
                }
            }),
        ];
        let mut inputs = HashMap::new();
        let mut choices = HashMap::new();
        let mut bools = HashMap::new();
        let mut known = HashMap::new();
        for (si, schema) in schemas.iter().enumerate() {
            for f in &schema.fields {
                let key = (si, f.key.clone());
                match &f.kind {
                    ConfigFieldKind::Bool => {
                        bools.insert(key, mapping::is_true(&mapping::default_value(f)));
                    }
                    ConfigFieldKind::Choice { options } => {
                        let opts: Vec<SharedString> =
                            options.iter().map(|o| o.clone().into()).collect();
                        let ix = options
                            .iter()
                            .position(|o| *o == mapping::default_value(f))
                            .unwrap_or(0);
                        let state = cx
                            .new(|cx| SelectState::new(opts, Some(IndexPath::new(ix)), window, cx));
                        subs.push(cx.subscribe_in(
                            &state,
                            window,
                            |this, _, _: &SelectEvent<Vec<SharedString>>, _, cx| {
                                this.values_changed(cx)
                            },
                        ));
                        choices.insert(key, state);
                    }
                    _ => {
                        let placeholder = f.placeholder.clone().unwrap_or_default();
                        let state = cx.new(|cx| {
                            InputState::new(window, cx)
                                .placeholder(placeholder)
                                .default_value(mapping::default_value(f))
                        });
                        subs.push(cx.subscribe_in(
                            &state,
                            window,
                            |this, _, e: &InputEvent, _, cx| {
                                if matches!(e, InputEvent::Change) {
                                    this.values_changed(cx);
                                }
                            },
                        ));
                        inputs.insert(key, state);
                    }
                }
                let picks = mapping::suggestions(&f.key, engines);
                if !picks.is_empty() {
                    known.insert(f.key.clone(), picks);
                }
            }
        }
        Self {
            schemas,
            kind_select,
            kind: 0,
            name,
            inputs,
            choices,
            bools,
            known,
            errors: mapping::Errors::new(),
            test: None,
            tested: None,
            revision: 0,
            busy: false,
            task: None,
            pick_task: None,
            test_focus: cx.focus_handle(),
            save_focus: cx.focus_handle(),
            save_anyway_focus: cx.focus_handle(),
            cancel_focus: cx.focus_handle(),
            focus_first: true,
            _subs: subs,
        }
    }

    // ── accessors (tests) ──────────────────────────────────────────────────────────────

    pub fn schemas(&self) -> &[EngineConfigSchema] {
        &self.schemas
    }
    pub fn kind_select(&self) -> &Entity<SelectState<Vec<SharedString>>> {
        &self.kind_select
    }
    pub fn input(&self, key: &str) -> Option<&Entity<InputState>> {
        self.inputs.get(&(self.kind, key.to_owned()))
    }
    pub fn name_input(&self) -> &Entity<InputState> {
        &self.name
    }
    pub fn test_state(&self) -> Option<&DialogTest> {
        self.test.as_ref()
    }
    pub fn errors(&self) -> &mapping::Errors {
        &self.errors
    }
    pub fn can_save(&self, cx: &App) -> bool {
        !self.busy && self.tested.is_some() && self.tested.as_ref() == self.config(cx).ok().as_ref()
    }
    /// Rendered field keys of the current kind, in order.
    pub fn field_keys(&self) -> Vec<String> {
        self.schema()
            .map(|s| s.fields.iter().map(|f| f.key.clone()).collect())
            .unwrap_or_default()
    }
    pub fn known_values(&self, key: &str) -> &[String] {
        self.known.get(key).map(Vec::as_slice).unwrap_or(&[])
    }
    pub fn set_kind(&mut self, label: &str, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(ix) = self.schemas.iter().position(|s| s.label == label) {
            self.kind = ix;
            self.kind_select.update(cx, |s, cx| {
                s.set_selected_index(Some(IndexPath::new(ix)), window, cx)
            });
            self.values_changed(cx);
        }
    }

    fn schema(&self) -> Option<&EngineConfigSchema> {
        self.schemas.get(self.kind)
    }

    /// Current field values of the selected kind.
    pub fn values(&self, cx: &App) -> Values {
        let mut v = Values::new();
        let Some(schema) = self.schema() else {
            return v;
        };
        for f in &schema.fields {
            let key = (self.kind, f.key.clone());
            let value = match &f.kind {
                ConfigFieldKind::Bool => self.bools.get(&key).copied().unwrap_or(false).to_string(),
                ConfigFieldKind::Choice { .. } => self
                    .choices
                    .get(&key)
                    .and_then(|c| c.read(cx).selected_value().map(|v| v.to_string()))
                    .unwrap_or_default(),
                _ => self
                    .inputs
                    .get(&key)
                    .map(|i| i.read(cx).value().to_string())
                    .unwrap_or_default(),
            };
            v.insert(f.key.clone(), value);
        }
        v
    }

    /// The config the form describes, or its validation errors.
    pub fn config(&self, cx: &App) -> Result<EngineConfig, mapping::Errors> {
        let Some(schema) = self.schema() else {
            let mut e = mapping::Errors::new();
            e.insert(mapping::FORM.into(), s::NO_ENGINE_TYPES);
            return Err(e);
        };
        mapping::build(schema, &self.values(cx), &self.name.read(cx).value())
    }

    /// Any edit invalidates an earlier test result (stale guard: a running test's result is
    /// dropped too).
    fn values_changed(&mut self, cx: &mut Context<Self>) {
        if self.test.is_some() || !self.errors.is_empty() {
            self.revision += 1;
            self.test = None;
            self.errors.clear();
            if !self.busy {
                self.task = None;
            }
        }
        cx.notify();
    }

    fn set_bool(&mut self, key: &str, v: bool, cx: &mut Context<Self>) {
        self.bools.insert((self.kind, key.to_owned()), v);
        self.values_changed(cx);
    }

    fn set_text(&mut self, key: &str, v: String, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(i) = self.inputs.get(&(self.kind, key.to_owned())).cloned() {
            i.update(cx, |i, cx| {
                i.set_value(v, window, cx);
                i.focus(window, cx);
            });
            self.values_changed(cx);
        }
    }

    /// *Browse…* for a Path field: the OS file picker (KBD-072: Enter/Space open it).
    fn browse(&mut self, key: String, window: &mut Window, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some(s::BROWSE.into()),
        });
        self.pick_task = Some(cx.spawn_in(window, async move |this, cx| {
            let picked = rx.await.ok().and_then(Result::ok).flatten();
            let Some(path) = picked.and_then(|p| p.into_iter().next()) else {
                return;
            };
            this.update_in(cx, |this, window, cx| {
                this.set_text(&key, path.to_string_lossy().into_owned(), window, cx)
            })
            .ok();
        }));
    }

    /// *Test* (`hub.test_engine`).
    pub fn run_test(&mut self, cx: &mut Context<Self>) {
        let cfg = match self.config(cx) {
            Ok(c) => c,
            Err(e) => {
                self.errors = e;
                cx.notify();
                return;
            }
        };
        self.errors.clear();
        self.revision += 1;
        let rev = self.revision;
        self.test = Some(DialogTest::Running);
        self.tested = None;
        let call = AppState::hub(cx).test_engine(cfg.clone());
        self.task = Some(cx.spawn(async move |this, cx| {
            let r = call.await;
            this.update(cx, |this, cx| {
                if this.revision != rev {
                    return;
                }
                this.test = Some(match r {
                    Ok(info) => {
                        this.tested = Some(cfg);
                        DialogTest::Ok(Box::new(info))
                    }
                    Err(e) => DialogTest::Failed {
                        message: e.to_string().into(),
                        hint: e.hint().map(|h| h.to_owned().into()),
                    },
                });
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    /// *Save* / *Save anyway* (`hub.add_engine`). Without `force`, requires a successful test
    /// of the current values.
    pub fn save(&mut self, force: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        let cfg = match self.config(cx) {
            Ok(c) => c,
            Err(e) => {
                self.errors = e;
                cx.notify();
                return;
            }
        };
        if !force && self.tested.as_ref() != Some(&cfg) {
            self.errors.insert(mapping::FORM.into(), s::SAVE_NEEDS_TEST);
            cx.notify();
            return;
        }
        self.busy = true;
        self.errors.clear();
        let display = if cfg.name.is_empty() {
            cfg.endpoint.display()
        } else {
            cfg.name.clone()
        };
        let call = AppState::hub(cx).add_engine(cfg);
        self.task = Some(cx.spawn_in(window, async move |this, cx| {
            let r = call.await;
            this.update_in(cx, |this, window, cx| {
                this.busy = false;
                match r {
                    Ok(_) => {
                        close(window, cx);
                        notify::success(window, cx, s::engine_added(&display));
                    }
                    Err(e) => {
                        notify::engine_error(window, cx, s::ADD_ENGINE_FAILED, &e);
                        cx.notify();
                    }
                }
            })
            .ok();
        }));
        cx.notify();
    }

    fn render_field(
        &self,
        f: &dk_core::ConfigField,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let key = (self.kind, f.key.clone());
        let error = self.errors.get(&f.key).map(|e| SharedString::from(*e));
        let control: AnyElement = match &f.kind {
            ConfigFieldKind::Bool => {
                let checked = self.bools.get(&key).copied().unwrap_or(false);
                let k = f.key.clone();
                Switch::new(SharedString::from(format!("add-engine-{}", f.key)))
                    .checked(checked)
                    .label(f.label.clone())
                    .on_click(cx.listener(move |this, v: &bool, _, cx| this.set_bool(&k, *v, cx)))
                    .into_any_element()
            }
            ConfigFieldKind::Choice { .. } => match self.choices.get(&key) {
                Some(state) => div()
                    .w(px(220.))
                    .child(Select::new(state).small())
                    .into_any_element(),
                None => div().into_any_element(),
            },
            ConfigFieldKind::Path => match self.inputs.get(&key) {
                Some(state) => {
                    let k = f.key.clone();
                    let this = cx.entity().downgrade();
                    h_flex()
                        .gap_2()
                        .child(div().flex_1().child(Input::new(state).small()))
                        .child(
                            Button::new(SharedString::from(format!("add-engine-browse-{}", f.key)))
                                .small()
                                .outline()
                                .icon(IconName::FolderOpen)
                                .label(s::BROWSE)
                                .on_click(move |_, window, cx| {
                                    let k = k.clone();
                                    this.update(cx, |d, cx| d.browse(k, window, cx)).ok();
                                }),
                        )
                        .into_any_element()
                }
                None => div().into_any_element(),
            },
            ConfigFieldKind::Text | ConfigFieldKind::Port => match self.inputs.get(&key) {
                Some(state) => Input::new(state).small().into_any_element(),
                None => div().into_any_element(),
            },
        };
        let picks = self.known.get(&f.key).cloned().unwrap_or_default();
        let control = if picks.is_empty() {
            control
        } else {
            v_flex()
                .gap_1()
                .child(control)
                .child(
                    h_flex()
                        .gap_1()
                        .flex_wrap()
                        .items_center()
                        .child(
                            div()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child(s::KNOWN_VALUES),
                        )
                        .children(picks.into_iter().enumerate().map(|(ix, v)| {
                            let k = f.key.clone();
                            let value = v.clone();
                            Button::new(SharedString::from(format!(
                                "add-engine-pick-{}-{ix}",
                                f.key
                            )))
                            .xsmall()
                            .ghost()
                            .label(v)
                            .on_click(cx.listener(
                                move |this, _, window, cx| {
                                    this.set_text(&k, value.clone(), window, cx)
                                },
                            ))
                        })),
                )
                .into_any_element()
        };
        let _ = window;
        if matches!(f.kind, ConfigFieldKind::Bool) {
            // The switch carries its own label.
            return v_flex()
                .gap_1()
                .child(control)
                .when_some(error, |this, e| {
                    this.child(div().text_xs().text_color(cx.theme().danger).child(e))
                })
                .into_any_element();
        }
        let label: SharedString = if f.required {
            format!("{} *", f.label).into()
        } else {
            f.label.clone().into()
        };
        labelled(label, control, error, cx)
    }

    fn render_test(&self, cx: &App) -> Option<AnyElement> {
        let t = cx.theme();
        match self.test.as_ref()? {
            DialogTest::Running => None,
            DialogTest::Ok(i) => Some(
                h_flex()
                    .id("add-engine-test-ok")
                    .gap_1()
                    .items_center()
                    .text_sm()
                    .text_color(t.success)
                    .child(Icon::new(IconName::CircleCheck).small())
                    .child(s::test_ok(
                        &i.name,
                        &i.server_version,
                        i.api_version.as_deref(),
                        &i.os,
                        &i.arch,
                    ))
                    .into_any_element(),
            ),
            DialogTest::Failed { message, hint } => Some(
                v_flex()
                    .id("add-engine-test-failed")
                    .gap_0p5()
                    .p_2()
                    .rounded(t.radius)
                    .bg(t.danger.opacity(0.1))
                    .text_sm()
                    .child(
                        h_flex()
                            .gap_1()
                            .items_center()
                            .text_color(t.danger)
                            .child(Icon::new(IconName::CircleX).small())
                            .child(format!("{}: {message}", s::TEST_FAILED)),
                    )
                    .when_some(hint.clone(), |this, h| {
                        this.child(div().text_color(t.foreground).child(h))
                    })
                    .into_any_element(),
            ),
        }
    }
}

impl FormView for AddEngineDialog {
    /// `Enter` / `Mod+Enter`: activate the focused footer button, otherwise *Save* when the
    /// test passed, else *Test* (KBD-071).
    fn submit_key(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.cancel_focus.is_focused(window) {
            close(window, cx);
        } else if self.test_focus.is_focused(window) {
            self.run_test(cx);
        } else if self.save_anyway_focus.is_focused(window) {
            self.save(true, window, cx);
        } else if self.can_save(cx) {
            self.save(false, window, cx);
        } else {
            self.run_test(cx);
        }
    }
}

impl Render for AddEngineDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if std::mem::take(&mut self.focus_first) {
            self.kind_select.update(cx, |s, cx| s.focus(window, cx));
        }
        let testing = matches!(self.test, Some(DialogTest::Running));
        let can_save = self.can_save(cx);
        let failed = matches!(self.test, Some(DialogTest::Failed { .. }));
        let form_err = self
            .errors
            .get(mapping::FORM)
            .map(|e| SharedString::from(*e));
        let fields: Vec<dk_core::ConfigField> =
            self.schema().map(|s| s.fields.clone()).unwrap_or_default();
        let this = cx.entity().downgrade();
        let (t1, t2, s1, s2, a1, a2) = (
            this.clone(),
            this.clone(),
            this.clone(),
            this.clone(),
            this.clone(),
            this,
        );
        let footer = h_flex()
            .gap_2()
            .child(focus_wrap(
                "add-engine-test-wrap",
                &self.test_focus,
                Button::new("add-engine-test")
                    .outline()
                    .icon(crate::assets::Lucide::Plug)
                    .label(if testing { s::TESTING } else { s::TEST })
                    .loading(testing)
                    .on_click(move |_, _, cx| {
                        t1.update(cx, |d, cx| d.run_test(cx)).ok();
                    }),
                move |_, _, cx| {
                    t2.update(cx, |d, cx| d.run_test(cx)).ok();
                },
                window,
                cx,
            ))
            .child(div().flex_1())
            .child(focus_wrap(
                "add-engine-cancel-wrap",
                &self.cancel_focus,
                Button::new("add-engine-cancel")
                    .label(s::CANCEL)
                    .on_click(|_, window, cx| close(window, cx)),
                |_, window, cx| close(window, cx),
                window,
                cx,
            ))
            .when(failed, |this| {
                this.child(focus_wrap(
                    "add-engine-save-anyway-wrap",
                    &self.save_anyway_focus,
                    Button::new("add-engine-save-anyway")
                        .outline()
                        .label(s::SAVE_ANYWAY)
                        .on_click(move |_, window, cx| {
                            a1.update(cx, |d, cx| d.save(true, window, cx)).ok();
                        }),
                    move |_, window, cx| {
                        a2.update(cx, |d, cx| d.save(true, window, cx)).ok();
                    },
                    window,
                    cx,
                ))
            })
            .child(focus_wrap(
                "add-engine-save-wrap",
                &self.save_focus,
                Button::new("add-engine-save")
                    .primary()
                    .label(s::SAVE)
                    .loading(self.busy)
                    .disabled(!can_save)
                    .tooltip_with_action(
                        s::SAVE,
                        &crate::actions::dialog::ConfirmDestructive,
                        Some(crate::keymap::ctx::DIALOG),
                    )
                    .on_click(move |_, window, cx| {
                        s1.update(cx, |d, cx| d.save(false, window, cx)).ok();
                    }),
                move |_, window, cx| {
                    s2.update(cx, |d, cx| d.save(false, window, cx)).ok();
                },
                window,
                cx,
            ));
        let mut body = form_root("add-engine-dialog", cx);
        if self.schemas.is_empty() {
            body = body.child(div().text_sm().child(s::NO_ENGINE_TYPES));
        } else {
            body = body
                .child(field(
                    s::ENGINE_KIND,
                    div()
                        .w(px(260.))
                        .child(Select::new(&self.kind_select).small()),
                    None,
                    None,
                    cx,
                ))
                .child(field(
                    s::ENGINE_NAME,
                    Input::new(&self.name).small(),
                    None,
                    None,
                    cx,
                ))
                .children(fields.iter().map(|f| self.render_field(f, window, cx)));
        }
        body.children(self.render_test(cx))
            .children(form_error(form_err, cx))
            .child(footer)
    }
}

/// `form_dialog::field` with a runtime label (schema field labels come from the factories).
fn labelled(
    label: SharedString,
    control: impl IntoElement,
    error: Option<SharedString>,
    cx: &App,
) -> AnyElement {
    v_flex()
        .gap_1()
        .child(
            div()
                .text_sm()
                .font_weight(gpui_kit::FontWeight::MEDIUM)
                .child(label),
        )
        .child(control)
        .when_some(error, |this, e| {
            this.child(div().text_xs().text_color(cx.theme().danger).child(e))
        })
        .into_any_element()
}
