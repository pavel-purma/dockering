//! Updater UI view tests (UPD-008, UPD-009, SET-090, KBD-076). Statuses are injected into the
//! `UpdateStore`; the hub side is covered by `dk-hub`'s `updates_tests`.

use dk_hub::{
    DisabledReason, OfferMode, OfferStage, UiState, UpdateOffer, UpdateSnapshot, UpdateState,
    UpdateStatus,
};
use gpui_kit::component::WindowExt;
use gpui_kit::{Entity, TestAppContext};

use crate::nav::SettingsSection;
use crate::state::{AppState, UpdateStore};
use crate::testing::{Harness, Setup, start};

fn store(cx: &mut TestAppContext) -> Entity<UpdateStore> {
    cx.read(UpdateStore::global)
        .expect("update store installed")
}

const NOTES: &str = "https://github.com/pavel-purma/dockering/releases/tag/v9.9.9";

fn snapshot(status: UpdateStatus, offer: Option<UpdateOffer>) -> UpdateSnapshot {
    UpdateSnapshot {
        status,
        offer,
        revision: 0,
    }
}

fn offer(
    version: &str,
    mode: OfferMode,
    stage: OfferStage,
    needs_elevation: bool,
) -> Option<UpdateOffer> {
    Some(UpdateOffer {
        version: version.into(),
        notes_url: NOTES.into(),
        mode,
        stage,
        needs_elevation,
        install_failed: false,
    })
}

/// A verified install offer (*Restart to update*).
fn ready(version: &str, needs_elevation: bool) -> Option<UpdateOffer> {
    offer(
        version,
        OfferMode::Install,
        OfferStage::Ready,
        needs_elevation,
    )
}

fn set(h: &Harness, cx: &mut TestAppContext, snapshot: UpdateSnapshot) {
    let store = store(cx);
    store.update(cx, |s, cx| s.set_snapshot_for_test(snapshot, cx));
    h.draw(cx);
}

/// Activity only, no offer.
fn set_status(h: &Harness, cx: &mut TestAppContext, status: UpdateStatus) {
    set(h, cx, snapshot(status, None));
}

fn open_updates(h: &Harness, cx: &mut TestAppContext) {
    h.update(cx, |_, window, cx| {
        window.dispatch_action(
            Box::new(crate::actions::Navigate {
                route: crate::nav::Route::Settings {
                    section: SettingsSection::Updates,
                },
            }),
            cx,
        )
    });
    h.draw(cx);
}

fn notifications(h: &Harness, cx: &mut TestAppContext) -> usize {
    h.update(cx, |_, window, cx| window.notifications(cx).len())
}

#[gpui_kit::test]
fn upd_008_status_bar_renders_each_state(cx: &mut TestAppContext) {
    let h = start(cx, Setup::default());
    h.wait_containers(cx);
    let disabled = |reason| UpdateStatus::Disabled { reason };
    let idle = UpdateStatus::Idle { last_check: None };
    for (snapshot, expected) in [
        (snapshot(disabled(DisabledReason::Unavailable), None), false),
        (snapshot(disabled(DisabledReason::Policy), None), false),
        (snapshot(idle.clone(), None), false),
        (snapshot(UpdateStatus::Checking, None), false),
        // A *Download* offer (portable, macOS, .deb, degraded) is a link to the release page.
        (
            snapshot(
                idle.clone(),
                offer("9.9.9", OfferMode::Download, OfferStage::Ready, false),
            ),
            true,
        ),
        (
            snapshot(
                UpdateStatus::Downloading {
                    version: "9.9.9".into(),
                    done: 50,
                    total: 100,
                },
                None,
            ),
            true,
        ),
        // The activity and the offer are separate: a check never hides the offer.
        (snapshot(UpdateStatus::Checking, ready("9.9.9", true)), true),
        (snapshot(idle.clone(), ready("9.9.9", true)), true),
        // An install offer whose payload isn't verified yet has no button (UPD-017).
        (
            snapshot(
                idle.clone(),
                offer("9.9.9", OfferMode::Install, OfferStage::Downloading, false),
            ),
            false,
        ),
        (
            snapshot(
                UpdateStatus::Error {
                    message: "offline".into(),
                },
                None,
            ),
            false,
        ),
    ] {
        set(&h, cx, snapshot.clone());
        let item = h.read(cx, |shell, _, cx| shell.has_update_item(cx));
        assert_eq!(item, expected, "{snapshot:?}");
    }
    h.shutdown();
}

#[gpui_kit::test]
fn upd_008_ready_notification_once_per_version(cx: &mut TestAppContext) {
    let h = start(cx, Setup::default());
    h.wait_containers(cx);
    let before = notifications(&h, cx);
    let ready = |v: &str| snapshot(UpdateStatus::Idle { last_check: None }, ready(v, false));
    set(&h, cx, ready("9.9.9"));
    assert_eq!(notifications(&h, cx), before + 1);
    set_status(&h, cx, UpdateStatus::Idle { last_check: None });
    set(&h, cx, ready("9.9.9"));
    assert_eq!(
        notifications(&h, cx),
        before + 1,
        "same version: no second toast"
    );
    set(&h, cx, ready("9.9.10"));
    assert_eq!(notifications(&h, cx), before + 2);
    h.shutdown();
}

#[gpui_kit::test]
fn upd_008_first_launch_after_update_shows_notice(cx: &mut TestAppContext) {
    let h = start(
        cx,
        Setup {
            ui_state: UiState {
                updates: UpdateState {
                    last_run_version: Some("0.0.1".into()),
                    ..Default::default()
                },
                ..Default::default()
            },
            ..Setup::default()
        },
    );
    h.wait_containers(cx);
    assert_eq!(notifications(&h, cx), 1);
    h.shutdown();
}

#[gpui_kit::test]
fn upd_008_fresh_profile_shows_no_updated_notice(cx: &mut TestAppContext) {
    let h = start(cx, Setup::default());
    h.wait_containers(cx);
    assert_eq!(notifications(&h, cx), 0);
    h.shutdown();
}

#[gpui_kit::test]
fn set_090_updates_section_states(cx: &mut TestAppContext) {
    let h = start(cx, Setup::default());
    h.wait_containers(cx);
    open_updates(&h, cx);
    for status in [
        UpdateStatus::Idle { last_check: None },
        UpdateStatus::Disabled {
            reason: DisabledReason::Unavailable,
        },
        UpdateStatus::Disabled {
            reason: DisabledReason::Policy,
        },
        UpdateStatus::Checking,
    ] {
        set_status(&h, cx, status);
    }
    set_status(
        &h,
        cx,
        UpdateStatus::Disabled {
            reason: DisabledReason::Setting,
        },
    );
    // The switch writes `[updates] check`.
    let page = h
        .read(cx, |s, _, _| match s.page() {
            crate::shell::ShellPage::Settings(p) => Some(p.clone()),
            _ => None,
        })
        .expect("settings page");
    page.update(cx, |p, cx| {
        p.set_bool(
            crate::pages::settings::controls::BoolKey::CheckUpdates,
            false,
            cx,
        )
    });
    assert!(!h.hub.config().get().updates.check);
    assert!(!cx.read(|cx| AppState::config(cx).updates.check));
    h.shutdown();
}

// With the `updater` feature this would hit the network; it only checks the no-updater build.
#[cfg(not(feature = "updater"))]
#[gpui_kit::test]
fn set_090_manual_check_without_updater_reports_disabled(cx: &mut TestAppContext) {
    use crate::state::ManualCheck;
    use dk_hub::UpdateCheck;
    // Test builds have no `updater` feature: the hub answers Disabled, never errors.
    let h = start(cx, Setup::default());
    h.wait_containers(cx);
    open_updates(&h, cx);
    h.update(cx, |_, window, cx| {
        window.dispatch_action(Box::new(crate::actions::CheckForUpdates), cx)
    });
    let store = store(cx);
    h.wait_until(cx, "manual check result", |_, cx| {
        matches!(store.read(cx).manual(), Some(ManualCheck::Done(_)))
    });
    assert_eq!(
        cx.read(|cx| store.read(cx).manual().cloned()),
        Some(ManualCheck::Done(UpdateCheck::Disabled))
    );
    h.shutdown();
}

#[gpui_kit::test]
fn set_090_policy_disables_controls(cx: &mut TestAppContext) {
    let h = start(cx, Setup::default());
    h.wait_containers(cx);
    open_updates(&h, cx);
    set_status(
        &h,
        cx,
        UpdateStatus::Disabled {
            reason: DisabledReason::Policy,
        },
    );
    // *Check now* is not rendered under a policy; it is once the policy is gone.
    let visible = |h: &Harness, cx: &mut TestAppContext| {
        h.draw(cx);
        let mut visual = gpui_kit::VisualTestContext::from_window(h.any_window(), cx);
        visual.debug_bounds("upd-check-now").is_some()
    };
    assert!(!visible(&h, cx));
    set_status(&h, cx, UpdateStatus::Idle { last_check: None });
    assert!(visible(&h, cx));
    h.shutdown();
}

#[test]
fn kbd_076_update_actions_in_palette() {
    use gpui_kit::Action as _;
    let names: Vec<&str> = crate::commands::COMMANDS
        .iter()
        .map(|c| (c.action)().name())
        .collect();
    for action in [
        crate::actions::CheckForUpdates.name(),
        crate::actions::RestartToUpdate.name(),
        crate::actions::ViewReleaseNotes.name(),
    ] {
        assert!(names.contains(&action), "{action} not in the palette");
    }
}
