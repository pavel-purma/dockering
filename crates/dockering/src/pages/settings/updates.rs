//! Settings › Updates (SET-090, UPD-009): the same section on every OS. The automatic-check
//! switch, the current version, the last check, *Check now* with an inline result, the retained
//! offer (*Restart to update* or *Download* and the reason) and release notes.
//!
//! The offer and the check activity are drawn independently: checking, a failed check and
//! *Check for updates automatically* being off never erase the offer (UPD-009). An
//! administrator policy (a Windows registry value) switches updates off: the switch is disabled
//! and *Check now* hidden; the other hard disables (no `updater` feature, the environment
//! switch, demo mode, no trust keys) disable the actions and say why. Nothing here reads the
//! operating system.

use dk_hub::{DisabledReason, OfferMode, OfferStage, UpdateCheck, UpdateOffer, UpdateStatus};
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{ActiveTheme, Disableable, IconName, Sizable, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::{AnyElement, Context, IntoElement, SharedString, div};

use super::SettingsPage;
use super::controls::BoolKey;
use crate::actions::{CheckForUpdates, RestartToUpdate, ViewReleaseNotes};
use crate::state::{AppState, ManualCheck, UpdateStore};
use crate::strings as s;

/// What the section can do, from the hub status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    /// Policy: everything disabled, with a note; *Check now* hidden.
    Policy,
    /// Not compiled in, `DOCKERING_DISABLE_UPDATES`, demo, or no trust keys: a note; *Check
    /// now* and the offer's actions disabled.
    Unavailable,
    /// Normal, including "automatic checks off" (*Check now* and the offer still work).
    Normal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CheckNow {
    Hidden,
    Disabled,
    Enabled,
}

/// The button of the offer row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OfferAction {
    /// *Restart to update* (runs `RestartToUpdate`; after an install that couldn't relaunch it
    /// only retries the start).
    Restart,
    /// *Download* (runs `ViewReleaseNotes`, which opens the release page).
    Download,
}

/// The retained offer as the section shows it (UPD-009).
#[derive(Debug, Clone, PartialEq, Eq)]
struct OfferRow {
    version: String,
    /// The button, or none when nothing can be done now (fetching, or a hard disable).
    action: Option<OfferAction>,
    /// The button is disabled with a spinner: *Restart to update* is running.
    busy: bool,
    /// Why, or what is going on: the fallback reason, the progress, the real hard-disable reason.
    detail: Option<&'static str>,
}

/// Everything the section decides, without a window (so it is tested row by row).
#[derive(Debug, Clone, PartialEq, Eq)]
struct Model {
    /// The note above the rows (policy, or a hard disable with its real reason).
    note: Option<&'static str>,
    switch_checked: bool,
    switch_disabled: bool,
    check_now: CheckNow,
    offer: Option<OfferRow>,
}

/// The section for `status`, the retained `offer` and the configured switch. The offer comes
/// from `offer` alone: the activity in `status` only matters for a hard disable.
fn model(status: &UpdateStatus, offer: Option<&UpdateOffer>, check_on: bool) -> Model {
    let mode = match status {
        UpdateStatus::Disabled {
            reason: DisabledReason::Policy,
        } => Mode::Policy,
        UpdateStatus::Disabled {
            reason: DisabledReason::Unavailable,
        } => Mode::Unavailable,
        _ => Mode::Normal,
    };
    let note = match mode {
        Mode::Policy => Some(s::UPD_POLICY),
        Mode::Unavailable => Some(s::UPD_UNAVAILABLE),
        Mode::Normal => None,
    };
    Model {
        note,
        switch_checked: check_on && mode != Mode::Policy,
        switch_disabled: mode == Mode::Policy,
        check_now: match mode {
            Mode::Policy => CheckNow::Hidden,
            Mode::Unavailable => CheckNow::Disabled,
            Mode::Normal => CheckNow::Enabled,
        },
        // A hard disable keeps the offer but suppresses its actions and says why.
        offer: offer.map(|o| offer_row(o, note)),
    }
}

fn offer_row(offer: &UpdateOffer, hard_disabled: Option<&'static str>) -> OfferRow {
    let row = |action, busy, detail| OfferRow {
        version: offer.version.clone(),
        action,
        busy,
        detail,
    };
    if let Some(reason) = hard_disabled {
        return row(None, false, Some(reason));
    }
    // This install can't update itself, or installing failed: the release page (UPD-006).
    if offer.mode == OfferMode::Download || offer.install_failed {
        let reason = if offer.install_failed {
            s::UPD_REASON_INSTALL_FAILED
        } else {
            s::UPD_REASON_NO_SELF_UPDATE
        };
        return row(Some(OfferAction::Download), false, Some(reason));
    }
    match offer.stage {
        OfferStage::Downloading => row(None, false, Some(s::UPD_OFFER_DOWNLOADING)),
        OfferStage::Ready => row(Some(OfferAction::Restart), false, None),
        OfferStage::Applying => row(
            Some(OfferAction::Restart),
            true,
            Some(s::UPD_OFFER_APPLYING),
        ),
        OfferStage::InstalledRestart => row(
            Some(OfferAction::Restart),
            false,
            Some(s::UPD_TOOLTIP_INSTALLED_RESTART),
        ),
    }
}

pub(super) fn blocks(this: &mut SettingsPage, cx: &mut Context<SettingsPage>) -> Vec<AnyElement> {
    let store = UpdateStore::global(cx);
    let status = store
        .as_ref()
        .map(|s| s.read(cx).status().clone())
        .unwrap_or(UpdateStatus::Disabled {
            reason: DisabledReason::Unavailable,
        });
    let offer = store.as_ref().and_then(|s| s.read(cx).offer().cloned());
    let manual = store.as_ref().and_then(|s| s.read(cx).manual().cloned());
    let check_on = BoolKey::CheckUpdates.get(AppState::config(cx));
    let model = model(&status, offer.as_ref(), check_on);
    let state = AppState::ui_state(cx).updates;

    let switch = Switch::new(BoolKey::CheckUpdates.id())
        .checked(model.switch_checked)
        .disabled(model.switch_disabled)
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
        .disabled(checking || model.check_now != CheckNow::Enabled)
        .tooltip_with_action(s::CMD_CHECK_FOR_UPDATES, &CheckForUpdates, None)
        .debug_selector(|| "upd-check-now".into())
        .on_click(super::dispatch_here(&this.focus, CheckForUpdates));

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
    if let Some(text) = model.note {
        rows.push(note(text, cx));
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
            .child(format!("{} {}", s::APP_NAME, dk_hub::app_version()))
            .into_any_element(),
        None,
        cx,
    ));
    // The retained offer, whatever the updater is doing (UPD-009).
    if let Some(row) = &model.offer {
        rows.push(offer_row_view(this, row, cx));
    }
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
    if model.check_now != CheckNow::Hidden {
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
            .on_click(super::dispatch_here(&this.focus, ViewReleaseNotes))
            .into_any_element(),
        None,
        cx,
    ));

    vec![crate::ui::section(s::UPD_GROUP, v_flex().gap_3().children(rows), cx).into_any_element()]
}

/// The offer row: the version and its button (*Restart to update* or *Download*), with the
/// reason or progress under the title. Buttons only dispatch actions (KBD-002, KBD-076).
fn offer_row_view(this: &SettingsPage, row: &OfferRow, cx: &Context<SettingsPage>) -> AnyElement {
    let control = h_flex()
        .gap_3()
        .items_center()
        .child(
            div()
                .debug_selector(|| "upd-offer-version".into())
                .text_sm()
                .font_family(cx.theme().mono_font_family.clone())
                .child(s::upd_offer_version(&row.version)),
        )
        .when_some(row.action, |this_row, action| match action {
            OfferAction::Restart => this_row.child(
                Button::new("upd-offer-restart")
                    .small()
                    .primary()
                    .icon(IconName::Redo)
                    .label(s::CMD_RESTART_TO_UPDATE)
                    .loading(row.busy)
                    .disabled(row.busy)
                    .tooltip_with_action(s::CMD_RESTART_TO_UPDATE, &RestartToUpdate, None)
                    .debug_selector(|| "upd-offer-restart".into())
                    .on_click(super::dispatch_here(&this.focus, RestartToUpdate)),
            ),
            OfferAction::Download => this_row.child(
                Button::new("upd-offer-download")
                    .small()
                    .outline()
                    .icon(IconName::ExternalLink)
                    .label(s::UPD_DOWNLOAD)
                    .tooltip_with_action(s::CMD_VIEW_RELEASE_NOTES, &ViewReleaseNotes, None)
                    .debug_selector(|| "upd-offer-download".into())
                    .on_click(super::dispatch_here(&this.focus, ViewReleaseNotes)),
            ),
        })
        .into_any_element();
    div()
        .debug_selector(|| "upd-offer-row".into())
        .child(super::setting_row(
            s::UPD_OFFER,
            row.detail,
            control,
            None,
            cx,
        ))
        .into_any_element()
}

fn note(text: &'static str, cx: &Context<SettingsPage>) -> AnyElement {
    div()
        .debug_selector(|| "upd-note".into())
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
    use super::*;

    fn offer(mode: OfferMode, stage: OfferStage) -> UpdateOffer {
        UpdateOffer {
            version: "9.9.9".into(),
            notes_url: "https://example.test/notes".into(),
            mode,
            stage,
            needs_elevation: false,
            install_failed: false,
        }
    }

    fn idle() -> UpdateStatus {
        UpdateStatus::Idle { last_check: None }
    }

    fn disabled(reason: DisabledReason) -> UpdateStatus {
        UpdateStatus::Disabled { reason }
    }

    #[test]
    fn set_090_short_time() {
        assert_eq!(
            super::short_time("2026-10-03T08:15:42.123Z"),
            "2026-10-03 08:15 UTC"
        );
        assert_eq!(super::short_time("bogus"), "bogus");
    }

    /// The offer row for every kind of offer: the action and the reason.
    #[test]
    fn set_090_model_offer_row_and_reason() {
        use OfferAction::{Download, Restart};
        let failed = UpdateOffer {
            install_failed: true,
            ..offer(OfferMode::Download, OfferStage::Ready)
        };
        let row = |o: &UpdateOffer| model(&idle(), Some(o), true).offer.expect("offer row");
        let cases: Vec<(UpdateOffer, Option<OfferAction>, bool, Option<&str>)> = vec![
            (
                offer(OfferMode::Install, OfferStage::Ready),
                Some(Restart),
                false,
                None,
            ),
            (
                offer(OfferMode::Install, OfferStage::InstalledRestart),
                Some(Restart),
                false,
                Some("Dockering was updated. Restart to finish"),
            ),
            (
                offer(OfferMode::Install, OfferStage::Applying),
                Some(Restart),
                true,
                Some("Installing the update…"),
            ),
            (
                offer(OfferMode::Install, OfferStage::Downloading),
                None,
                false,
                Some("Downloading the update…"),
            ),
            (
                offer(OfferMode::Download, OfferStage::Ready),
                Some(Download),
                false,
                Some("This installation can't update itself automatically."),
            ),
            (
                failed,
                Some(Download),
                false,
                Some("The update couldn't be installed automatically."),
            ),
        ];
        for (o, action, busy, detail) in cases {
            let got = row(&o);
            assert_eq!(got.version, "9.9.9");
            assert_eq!(
                (got.action, got.busy, got.detail),
                (action, busy, detail),
                "{o:?}"
            );
        }
        assert_eq!(model(&idle(), None, true).offer, None);
    }

    /// Checking and errors never replace the offer: the row is the same whatever the activity.
    #[test]
    fn set_090_model_activity_never_changes_the_offer_row() {
        let o = offer(OfferMode::Install, OfferStage::Ready);
        let expected = model(&idle(), Some(&o), true).offer;
        assert!(expected.is_some());
        for status in [
            UpdateStatus::Checking,
            UpdateStatus::Error {
                message: "offline".into(),
            },
            UpdateStatus::Downloading {
                version: "9.9.10".into(),
                done: 1,
                total: 2,
            },
            disabled(DisabledReason::Setting),
        ] {
            assert_eq!(model(&status, Some(&o), true).offer, expected, "{status:?}");
        }
    }

    #[test]
    fn set_090_model_hard_disable_suppresses_actions_with_the_real_reason() {
        let o = offer(OfferMode::Install, OfferStage::Ready);
        for (reason, text) in [
            (DisabledReason::Policy, s::UPD_POLICY),
            (DisabledReason::Unavailable, s::UPD_UNAVAILABLE),
        ] {
            assert!(reason.is_hard());
            let m = model(&disabled(reason), Some(&o), true);
            let row = m.offer.expect("the offer stays visible");
            assert_eq!(
                (row.action, row.busy, row.detail),
                (None, false, Some(text))
            );
            assert_eq!(m.note, Some(text));
        }
    }

    /// Only the setting is off: the switch shows it, and *Check now* and the offer still work.
    #[test]
    fn set_090_model_setting_off_keeps_check_now_and_offer() {
        assert!(!DisabledReason::Setting.is_hard());
        let o = offer(OfferMode::Install, OfferStage::Ready);
        let m = model(&disabled(DisabledReason::Setting), Some(&o), false);
        assert!(
            !m.switch_checked && !m.switch_disabled,
            "off, and still operable"
        );
        assert_eq!(m.check_now, CheckNow::Enabled);
        assert_eq!(m.note, None, "no note: it is the user's choice");
        let row = m.offer.expect("the offer stays");
        assert_eq!(
            (row.action, row.busy, row.detail),
            (Some(OfferAction::Restart), false, None)
        );
    }

    /// A policy disables the switch (shown off whatever `config.toml` says) and hides *Check now*;
    /// the note names it as an administrator policy of the Windows registry and no other system.
    #[test]
    fn set_090_model_policy_disables_the_switch_and_hides_check_now() {
        let m = model(&disabled(DisabledReason::Policy), None, true);
        assert!(m.switch_disabled && !m.switch_checked);
        assert_eq!(m.check_now, CheckNow::Hidden);
        assert_eq!(m.note, Some(s::UPD_POLICY));
        // The other hard disable keeps *Check now* in place, disabled, with its own note.
        let u = model(&disabled(DisabledReason::Unavailable), None, true);
        assert!(!u.switch_disabled);
        assert_eq!(u.check_now, CheckNow::Disabled);
        assert_eq!(u.note, Some(s::UPD_UNAVAILABLE));
        // Everything else has no note and a working *Check now*.
        for status in [
            idle(),
            UpdateStatus::Checking,
            disabled(DisabledReason::Setting),
        ] {
            let m = model(&status, None, true);
            assert_eq!(
                (m.note, m.check_now),
                (None, CheckNow::Enabled),
                "{status:?}"
            );
        }
    }
}
