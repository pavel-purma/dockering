//! Settings › Updates (SET-090, UPD-009): the automatic-check switch, current version, last
//! check, *Check now* with an inline result, and release notes. When an administrator policy
//! turns updates off the section says so and its controls are disabled.

use dk_hub::{UpdateCheck, UpdateStatus};
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::group_box::{GroupBox, GroupBoxVariants};
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{ActiveTheme, Disableable, IconName, Sizable, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::{AnyElement, Context, IntoElement, SharedString, Window, div};

use super::SettingsPage;
use super::controls::BoolKey;
use crate::actions::{CheckForUpdates, ViewReleaseNotes};
use crate::state::{AppState, ManualCheck, UpdateStore};
use crate::strings as s;

/// What the section can do, from the hub status and the setting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    /// Policy: everything disabled, with a note.
    Policy,
    /// Not compiled in, `DOCKERING_DISABLE_UPDATES`, or demo, while the switch is on.
    Unavailable,
    Normal,
}

pub(super) fn blocks(
    this: &mut SettingsPage,
    _window: &mut Window,
    cx: &mut Context<SettingsPage>,
) -> Vec<AnyElement> {
    let store = UpdateStore::global(cx);
    let status = store
        .as_ref()
        .map(|s| s.read(cx).status().clone())
        .unwrap_or(UpdateStatus::Disabled { by_policy: false });
    let manual = store.as_ref().and_then(|s| s.read(cx).manual().cloned());
    let check_on = BoolKey::CheckUpdates.get(AppState::config(cx));
    let mode = match status {
        UpdateStatus::Disabled { by_policy: true } => Mode::Policy,
        UpdateStatus::Disabled { by_policy: false } if check_on => Mode::Unavailable,
        _ => Mode::Normal,
    };
    let state = AppState::ui_state(cx).updates;

    let switch = Switch::new(BoolKey::CheckUpdates.id())
        .checked(check_on && mode != Mode::Policy)
        .disabled(mode == Mode::Policy)
        .accessibility_label(s::UPD_AUTO_CHECK)
        .on_click(cx.listener(|this, v: &bool, _, cx| this.set_bool(BoolKey::CheckUpdates, *v, cx)))
        .into_any_element();

    let checking = matches!(manual, Some(ManualCheck::Running));
    let check_button = Button::new("upd-check-now")
        .small()
        .outline()
        .icon(IconName::Redo)
        .label(s::UPD_CHECK_NOW)
        .loading(checking)
        .disabled(checking || mode != Mode::Normal)
        .tooltip_with_action(s::CMD_CHECK_FOR_UPDATES, &CheckForUpdates, None)
        .debug_selector(|| "upd-check-now".into())
        .on_click(super::dispatch_here(&this.nav_focus, CheckForUpdates));

    let result: Option<(SharedString, bool)> = match &manual {
        Some(ManualCheck::Done(UpdateCheck::UpToDate)) => Some((s::UPD_UP_TO_DATE.into(), false)),
        Some(ManualCheck::Done(UpdateCheck::Available { version })) => {
            Some((s::upd_available(version).into(), false))
        }
        Some(ManualCheck::Done(UpdateCheck::Disabled)) => Some((s::UPD_UNAVAILABLE.into(), false)),
        Some(ManualCheck::Failed(e)) => Some((format!("{} {e}", s::UPD_CHECK_FAILED).into(), true)),
        Some(ManualCheck::Running) | None => None,
    };
    let check_row = v_flex()
        .items_end()
        .gap_1()
        .child(
            h_flex()
                .gap_2()
                .items_center()
                .when(checking, |this| this.child(Spinner::new().small()))
                .child(check_button),
        )
        .when_some(result, |this, (text, error)| {
            this.child(
                div()
                    .max_w(gpui_kit::px(320.))
                    .text_xs()
                    .text_color(if error {
                        cx.theme().danger
                    } else {
                        cx.theme().muted_foreground
                    })
                    .child(text),
            )
        })
        .into_any_element();

    let last = match (&state.last_check, &state.last_result) {
        (Some(at), Some(result)) => format!("{} — {result}", short_time(at)),
        (Some(at), None) => short_time(at),
        _ => s::UPD_NEVER_CHECKED.to_owned(),
    };

    let mut rows = Vec::new();
    match mode {
        Mode::Policy => rows.push(note(s::UPD_POLICY, cx)),
        Mode::Unavailable => rows.push(note(s::UPD_UNAVAILABLE, cx)),
        Mode::Normal => {}
    }
    rows.push(super::setting_row(
        s::UPD_AUTO_CHECK,
        Some(s::UPD_AUTO_CHECK_DESC),
        switch,
        None,
        cx,
    ));
    rows.push(super::setting_row(
        s::DIAG_VERSION,
        None,
        div()
            .text_sm()
            .font_family(cx.theme().mono_font_family.clone())
            .child(format!("{} {}", s::APP_NAME, env!("CARGO_PKG_VERSION")))
            .into_any_element(),
        None,
        cx,
    ));
    rows.push(super::setting_row(
        s::UPD_LAST_CHECKED,
        None,
        div()
            .text_sm()
            .text_color(cx.theme().muted_foreground)
            .child(last)
            .into_any_element(),
        None,
        cx,
    ));
    if mode != Mode::Policy {
        rows.push(super::setting_row(
            s::UPD_CHECK_NOW,
            Some(s::UPD_CHECK_NOW_DESC),
            check_row,
            None,
            cx,
        ));
    }
    rows.push(super::setting_row(
        s::CMD_VIEW_RELEASE_NOTES,
        None,
        Button::new("upd-release-notes")
            .small()
            .ghost()
            .icon(IconName::ExternalLink)
            .label(s::CMD_VIEW_RELEASE_NOTES)
            .on_click(super::dispatch_here(&this.nav_focus, ViewReleaseNotes))
            .into_any_element(),
        None,
        cx,
    ));

    vec![
        GroupBox::new()
            .outline()
            .title(s::UPD_GROUP)
            .child(v_flex().gap_3().children(rows))
            .into_any_element(),
    ]
}

fn note(text: &'static str, cx: &Context<SettingsPage>) -> AnyElement {
    div()
        .text_sm()
        .text_color(cx.theme().warning)
        .child(text)
        .into_any_element()
}

/// `2026-10-03T08:15:42.123Z` → `2026-10-03 08:15 UTC`.
fn short_time(rfc3339: &str) -> String {
    match time::OffsetDateTime::parse(rfc3339, &time::format_description::well_known::Rfc3339) {
        Ok(t) => format!(
            "{:04}-{:02}-{:02} {:02}:{:02} UTC",
            t.year(),
            u8::from(t.month()),
            t.day(),
            t.hour(),
            t.minute()
        ),
        Err(_) => rfc3339.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn set_090_short_time() {
        assert_eq!(
            super::short_time("2026-10-03T08:15:42.123Z"),
            "2026-10-03 08:15 UTC"
        );
        assert_eq!(super::short_time("bogus"), "bogus");
    }
}
