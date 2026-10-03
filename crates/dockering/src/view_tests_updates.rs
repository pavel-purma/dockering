//! Updater UI view tests (UPD-008, UPD-009, SET-090, KBD-076). Statuses are injected into the
//! `UpdateStore`; the hub side is covered by `dk-hub`'s `updates_tests`.

use dk_hub::UpdateStatus;
use gpui_kit::component::WindowExt;
use gpui_kit::{Entity, TestAppContext};

use crate::nav::SettingsSection;
use crate::state::{AppState, UpdateStore};
use crate::testing::{Harness, Setup, start};

fn store(cx: &mut TestAppContext) -> Entity<UpdateStore> {
    cx.read(UpdateStore::global)
        .expect("update store installed")
}

fn set(h: &Harness, cx: &mut TestAppContext, status: UpdateStatus) {
    let store = store(cx);
    store.update(cx, |s, cx| s.set_status_for_test(status, cx));
    h.draw(cx);
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
    for status in [
        UpdateStatus::Disabled { by_policy: false },
        UpdateStatus::Disabled { by_policy: true },
        UpdateStatus::Idle { last_check: None },
        UpdateStatus::Checking,
        UpdateStatus::Available {
            version: "9.9.9".into(),
            notes_url: "https://github.com/pavel-purma/dockering/releases/tag/v9.9.9".into(),
            notify_only: true,
        },
        UpdateStatus::Downloading {
            version: "9.9.9".into(),
            done: 50,
            total: 100,
        },
        UpdateStatus::Ready {
            version: "9.9.9".into(),
            notes_url: "https://github.com/pavel-purma/dockering/releases/tag/v9.9.9".into(),
            needs_elevation: true,
        },
        UpdateStatus::Error {
            message: "offline".into(),
        },
    ] {
        set(&h, cx, status.clone());
        let item = h.read(cx, |shell, _, cx| shell.has_update_item(cx));
        let expected = matches!(
            status,
            UpdateStatus::Available { .. }
                | UpdateStatus::Downloading { .. }
                | UpdateStatus::Ready { .. }
        );
        assert_eq!(item, expected, "{status:?}");
    }
    h.shutdown();
}

#[gpui_kit::test]
fn upd_008_ready_notification_once_per_version(cx: &mut TestAppContext) {
    let h = start(cx, Setup::default());
    h.wait_containers(cx);
    let before = notifications(&h, cx);
    let ready = |v: &str| UpdateStatus::Ready {
        version: v.into(),
        notes_url: String::new(),
        needs_elevation: false,
    };
    set(&h, cx, ready("9.9.9"));
    assert_eq!(notifications(&h, cx), before + 1);
    set(&h, cx, UpdateStatus::Idle { last_check: None });
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
fn set_090_updates_section_states(cx: &mut TestAppContext) {
    let h = start(cx, Setup::default());
    h.wait_containers(cx);
    open_updates(&h, cx);
    for status in [
        UpdateStatus::Idle { last_check: None },
        UpdateStatus::Disabled { by_policy: false },
        UpdateStatus::Disabled { by_policy: true },
        UpdateStatus::Checking,
    ] {
        set(&h, cx, status);
    }
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
    set(&h, cx, UpdateStatus::Disabled { by_policy: true });
    // *Check now* is not rendered under a policy; it is once the policy is gone.
    let visible = |h: &Harness, cx: &mut TestAppContext| {
        h.draw(cx);
        let mut visual = gpui_kit::VisualTestContext::from_window(h.any_window(), cx);
        visual.debug_bounds("upd-check-now").is_some()
    };
    assert!(!visible(&h, cx));
    set(&h, cx, UpdateStatus::Idle { last_check: None });
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
