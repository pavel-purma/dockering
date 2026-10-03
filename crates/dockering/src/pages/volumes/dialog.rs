//! *Create volume* dialog (VOL-004, KBD-072): name (optional), driver (default `local`),
//! driver options and labels as repeating rows → `create_volume`.

use std::collections::BTreeMap;

use dk_core::validate::validate_name;
use dk_core::{EngineId, VolumeSpec};
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::{Sizable, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::{
    App, AppContext, Context, Entity, FocusHandle, IntoElement, Render, SharedString, Task,
    WeakEntity, Window,
};

use crate::pages::images::forms::parse_pairs;
use crate::state::{AppState, Collection, EngineStore};
use crate::strings as s;
use crate::ui::form_dialog::{
    FormView, close, field, footer, form_error, form_root, open_form_dialog,
};
use crate::ui::kv_rows::{KvRows, KvRowsSpec, KvValue};
use crate::ui::notify;

pub const OPTIONS_SPEC: KvRowsSpec = KvRowsSpec {
    id: "vol-options",
    title: s::DRIVER_OPTIONS,
    key_placeholder: s::ENV_KEY,
    value_placeholder: s::ENV_VALUE,
    separator: "=",
    add_label: s::ADD_OPTION,
    flag: None,
};

pub const LABELS_SPEC: KvRowsSpec = KvRowsSpec {
    id: "vol-labels",
    title: s::LABELS,
    key_placeholder: s::ENV_KEY,
    value_placeholder: s::ENV_VALUE,
    separator: "=",
    add_label: s::ADD_LABEL,
    flag: None,
};

/// Validation errors of the form.
#[derive(Debug, Default, PartialEq)]
pub struct SpecErrors {
    pub name: Option<&'static str>,
    pub driver: Option<&'static str>,
    pub options: Vec<Option<&'static str>>,
    pub labels: Vec<Option<&'static str>>,
}

/// Builds the `VolumeSpec` (VOL-004) or returns per-field errors. Pure.
pub fn volume_spec(
    name: &str,
    driver: &str,
    options: &[KvValue],
    labels: &[KvValue],
) -> Result<VolumeSpec, SpecErrors> {
    let name = name.trim();
    let driver = driver.trim();
    let mut errors = SpecErrors {
        name: (!name.is_empty() && validate_name(name).is_err()).then_some(s::ERR_NAME),
        driver: (!driver.is_empty()
            && (driver.starts_with('-')
                || !driver
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || "._-/:".contains(c))))
        .then_some(s::ERR_DRIVER),
        ..Default::default()
    };
    let (opts, oe) = parse_pairs(options);
    let (lbls, le) = parse_pairs(labels);
    errors.options = oe;
    errors.labels = le;
    let ok = errors.name.is_none()
        && errors.driver.is_none()
        && errors.options.iter().all(Option::is_none)
        && errors.labels.iter().all(Option::is_none);
    if !ok {
        return Err(errors);
    }
    Ok(VolumeSpec {
        name: (!name.is_empty()).then(|| name.to_owned()),
        driver: Some(if driver.is_empty() { "local" } else { driver }.to_owned()),
        driver_opts: opts.into_iter().collect::<BTreeMap<_, _>>(),
        labels: lbls.into_iter().collect::<BTreeMap<_, _>>(),
    })
}

pub struct CreateVolumeDialog {
    engine: EngineId,
    store: WeakEntity<EngineStore>,
    name: Entity<InputState>,
    driver: Entity<InputState>,
    name_error: Option<SharedString>,
    driver_error: Option<SharedString>,
    options: Entity<KvRows>,
    labels: Entity<KvRows>,
    error: Option<SharedString>,
    busy: bool,
    task: Option<Task<()>>,
    cancel_focus: FocusHandle,
    ok_focus: FocusHandle,
    focus_first: bool,
}

impl CreateVolumeDialog {
    pub fn open(
        engine: EngineId,
        store: WeakEntity<EngineStore>,
        window: &mut Window,
        cx: &mut App,
    ) -> Entity<CreateVolumeDialog> {
        let view = cx.new(|cx| CreateVolumeDialog {
            engine,
            store,
            name: cx.new(|cx| InputState::new(window, cx).placeholder(s::OPTIONAL_RANDOM)),
            driver: cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder("local")
                    .default_value("local")
            }),
            name_error: None,
            driver_error: None,
            options: cx.new(|cx| KvRows::new(OPTIONS_SPEC, cx)),
            labels: cx.new(|cx| KvRows::new(LABELS_SPEC, cx)),
            error: None,
            busy: false,
            task: None,
            cancel_focus: cx.focus_handle(),
            ok_focus: cx.focus_handle(),
            focus_first: true,
        });
        open_form_dialog(s::CREATE_VOLUME_TITLE, 520., view.clone(), window, cx);
        view
    }

    pub fn name_input(&self) -> &Entity<InputState> {
        &self.name
    }
    pub fn labels(&self) -> &Entity<KvRows> {
        &self.labels
    }
    pub fn options(&self) -> &Entity<KvRows> {
        &self.options
    }
    pub fn error(&self) -> Option<&SharedString> {
        self.error.as_ref()
    }

    pub fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        let name = self.name.read(cx).value().to_string();
        let driver = self.driver.read(cx).value().to_string();
        let options = self.options.read(cx).values(cx);
        let labels = self.labels.read(cx).values(cx);
        let errs = |v: &[Option<&'static str>]| -> Vec<Option<SharedString>> {
            v.iter().map(|e| e.map(SharedString::from)).collect()
        };
        let spec = match volume_spec(&name, &driver, &options, &labels) {
            Ok(spec) => spec,
            Err(e) => {
                self.name_error = e.name.map(Into::into);
                self.driver_error = e.driver.map(Into::into);
                self.options
                    .update(cx, |r, cx| r.set_errors(errs(&e.options), cx));
                self.labels
                    .update(cx, |r, cx| r.set_errors(errs(&e.labels), cx));
                cx.notify();
                return;
            }
        };
        self.name_error = None;
        self.driver_error = None;
        self.busy = true;
        self.error = None;
        cx.notify();
        let hub = AppState::hub(cx);
        let call = hub.call(
            &self.engine,
            move |e| async move { e.create_volume(spec).await },
        );
        let name = name.trim().to_owned();
        self.task = Some(cx.spawn_in(window, async move |this, cx| {
            let r = call.await;
            this.update_in(cx, |this, window, cx| {
                this.busy = false;
                match r {
                    Ok(v) => {
                        this.store
                            .update(cx, |s, cx| s.refetch(Collection::Volumes, cx))
                            .ok();
                        close(window, cx);
                        notify::success(window, cx, s::created_volume(&v.name));
                    }
                    Err(e) => {
                        this.error =
                            Some(format!("{}: {e}", s::create_volume_failed(&name)).into());
                        cx.notify();
                    }
                }
            })
            .ok();
        }));
    }
}

impl FormView for CreateVolumeDialog {
    fn submit_key(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.cancel_focus.is_focused(window) {
            close(window, cx);
            return;
        }
        for rows in [self.options.clone(), self.labels.clone()] {
            if rows.update(cx, |r, cx| r.activate_focused(window, cx)) {
                return;
            }
        }
        self.submit(window, cx);
    }
}

impl Render for CreateVolumeDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if std::mem::take(&mut self.focus_first) {
            self.name.update(cx, |i, cx| i.focus(window, cx));
        }
        let this = cx.entity().downgrade();
        form_root("create-volume-dialog", cx)
            .child(field(
                s::NAME,
                Input::new(&self.name).small(),
                None,
                self.name_error.clone(),
                cx,
            ))
            .child(field(
                s::DRIVER,
                Input::new(&self.driver).small(),
                None,
                self.driver_error.clone(),
                cx,
            ))
            .child(
                v_flex()
                    .id("vol-rows")
                    .gap_3()
                    .max_h(gpui_kit::px(320.))
                    .overflow_y_scroll()
                    .child(self.options.clone())
                    .child(self.labels.clone()),
            )
            .children(form_error(self.error.clone(), cx))
            .child(footer(
                &self.cancel_focus,
                &self.ok_focus,
                s::CREATE,
                self.busy,
                close,
                move |window, cx| {
                    this.update(cx, |d, cx| d.submit(window, cx)).ok();
                },
                cx,
            ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kv(k: &str, v: &str) -> KvValue {
        KvValue {
            key: k.into(),
            value: v.into(),
            flag: false,
        }
    }

    #[test]
    fn vol_004_spec() {
        let spec = volume_spec(
            "",
            " ",
            &[kv("type", "tmpfs")],
            &[kv("team", "a"), kv("", "")],
        )
        .unwrap();
        assert_eq!(spec.name, None);
        assert_eq!(spec.driver.as_deref(), Some("local"));
        assert_eq!(
            spec.driver_opts.get("type").map(String::as_str),
            Some("tmpfs")
        );
        assert_eq!(spec.labels.len(), 1);
        let e = volume_spec("-bad", "--x", &[kv("a b", "1")], &[]).unwrap_err();
        assert_eq!(e.name, Some(s::ERR_NAME));
        assert_eq!(e.driver, Some(s::ERR_DRIVER));
        assert_eq!(e.options, vec![Some(s::ERR_ENV_KEY)]);
    }
}
