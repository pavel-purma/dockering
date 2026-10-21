//! Updater UI view tests (UPD-008, UPD-009, UPD-011, UPD-015, SET-090, KBD-076). Snapshots are
//! injected into the `UpdateStore`; the hub side is covered by `dk-hub`'s `updates_tests`. The
//! test hub has no updater, so *Restart to update* runs against a stand-in for the hub call
//! (`UpdateStore::stub_apply_for_test`); "the app quit" is observed as the window bounds being
//! saved, because `quit()` is a no-op on the test platform.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use dk_core::EngineError;
use dk_hub::{
    ALREADY_APPLYING, ApplyOutcome, DisabledReason, OfferMode, OfferStage, UiState, UpdateOffer,
    UpdateSnapshot, UpdateState, UpdateStatus,
};
use futures::FutureExt as _;
use futures::channel::oneshot;
use gpui_kit::{Bounds, Entity, Pixels, TestAppContext};

use crate::nav::SettingsSection;
use crate::shell::Region;
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

fn idle() -> UpdateStatus {
    UpdateStatus::Idle { last_check: None }
}

fn disabled(reason: DisabledReason) -> UpdateStatus {
    UpdateStatus::Disabled { reason }
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

/// An install offer in `stage`.
fn install(version: &str, stage: OfferStage) -> Option<UpdateOffer> {
    offer(version, OfferMode::Install, stage, false)
}

/// A *Download* offer (portable, macOS, `.deb`, read-only install).
fn download(version: &str) -> Option<UpdateOffer> {
    offer(version, OfferMode::Download, OfferStage::Ready, false)
}

/// An install that failed: the hub degrades it to *Download* for that version.
fn degraded(version: &str) -> Option<UpdateOffer> {
    download(version).map(|o| UpdateOffer {
        install_failed: true,
        ..o
    })
}

/// Feeds `snapshot` to the store through its production guard and draws. `false`: dropped as
/// stale.
fn set(h: &Harness, cx: &mut TestAppContext, snapshot: UpdateSnapshot) -> bool {
    let store = store(cx);
    let taken = store.update(cx, |s, cx| s.set_snapshot_for_test(snapshot, cx));
    h.draw(cx);
    taken
}

/// Activity only, no offer.
fn set_status(h: &Harness, cx: &mut TestAppContext, status: UpdateStatus) {
    set(h, cx, snapshot(status, None));
}

/// An offer while the updater is idle.
fn set_offer(h: &Harness, cx: &mut TestAppContext, offer: Option<UpdateOffer>) {
    set(h, cx, snapshot(idle(), offer));
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
    h.update(cx, |_, _, cx| crate::ui::toast::count(cx))
}

/// The painted bounds of the element with `selector`, after a draw.
fn bounds(h: &Harness, cx: &mut TestAppContext, selector: &'static str) -> Option<Bounds<Pixels>> {
    h.draw(cx);
    let mut visual = gpui_kit::VisualTestContext::from_window(h.any_window(), cx);
    visual.debug_bounds(selector)
}

fn shown(h: &Harness, cx: &mut TestAppContext, selector: &'static str) -> bool {
    bounds(h, cx, selector).is_some()
}

/// Clicks the centre of the element with `selector`, the way a pointer would (the frame it hits
/// is the one just drawn). A toast overlaps the title bar while it slides in, and its
/// animation follows real time, so a click on a title-bar or page control is preceded by
/// [`clear_toasts`], and a click on a toast's own button by [`settle`].
fn click(h: &Harness, cx: &mut TestAppContext, selector: &'static str) {
    let b = bounds(h, cx, selector).unwrap_or_else(|| panic!("{selector} is not rendered"));
    let mut visual = gpui_kit::VisualTestContext::from_window(h.any_window(), cx);
    visual.simulate_click(b.center(), gpui_kit::Modifiers::default());
    visual.run_until_parked();
}

/// Waits until the element with `selector` has stopped moving (a toast that slid in).
fn settle(h: &Harness, cx: &mut TestAppContext, selector: &'static str) {
    let mut last = None;
    let mut still = 0;
    for _ in 0..200 {
        h.pump(cx, 50);
        let now = bounds(h, cx, selector);
        still = if now.is_some() && now == last {
            still + 1
        } else {
            0
        };
        if still >= 5 {
            return;
        }
        last = now;
    }
    panic!("{selector} never settled");
}

/// Closes every toast and waits until none is left.
fn clear_toasts(h: &Harness, cx: &mut TestAppContext) {
    h.update(cx, |_, _, cx| crate::ui::toast::clear(cx));
    h.wait_until(cx, "toasts gone", |_, cx| {
        crate::ui::toast::mounted(cx) == 0
    });
}

type Outcome = Result<ApplyOutcome, EngineError>;

/// Runs `result` instead of the hub call; every call is counted.
fn stub_apply(cx: &mut TestAppContext, result: Outcome) -> Rc<Cell<usize>> {
    let calls = Rc::new(Cell::new(0));
    let counter = calls.clone();
    store(cx).update(cx, |s, _| {
        s.stub_apply_for_test(move || {
            counter.set(counter.get() + 1);
            let result = result.clone();
            async move { result }.boxed_local()
        })
    });
    calls
}

/// The first call waits until the returned sender answers; a later one gets what the hub says
/// to a second apply while one runs.
fn stub_apply_pending(cx: &mut TestAppContext) -> (Rc<Cell<usize>>, oneshot::Sender<Outcome>) {
    let calls = Rc::new(Cell::new(0));
    let counter = calls.clone();
    let (tx, rx) = oneshot::channel();
    let rx = Rc::new(RefCell::new(Some(rx)));
    store(cx).update(cx, |s, _| {
        s.stub_apply_for_test(move || {
            counter.set(counter.get() + 1);
            match rx.borrow_mut().take() {
                Some(rx) => {
                    async move { rx.await.unwrap_or(Err(EngineError::Cancelled)) }.boxed_local()
                }
                None => async { Err(EngineError::protocol(ALREADY_APPLYING)) }.boxed_local(),
            }
        })
    });
    (calls, tx)
}

/// *Restart to update* saved the window bounds, which is what it does right before it quits.
fn quit_requested(h: &Harness) -> bool {
    h.hub.config().ui_state().window.is_some()
}

fn button_busy(h: &Harness, cx: &mut TestAppContext) -> Option<bool> {
    h.read(cx, |shell, _, cx| shell.update_button(cx).map(|b| b.busy))
}

// ── UPD-008: status bar and notification ─────────────────────────────────────────────────

#[gpui_kit::test]
fn upd_008_status_bar_keeps_only_transient_items(cx: &mut TestAppContext) {
    let h = start(cx, Setup::default());
    h.wait_containers(cx);
    let downloading = |version: &str| UpdateStatus::Downloading {
        version: version.into(),
        done: 50,
        total: 100,
    };
    for (snapshot, expected) in [
        (snapshot(disabled(DisabledReason::Unavailable), None), false),
        (snapshot(disabled(DisabledReason::Policy), None), false),
        (snapshot(idle(), None), false),
        // No manual check: an automatic one shows nothing.
        (snapshot(UpdateStatus::Checking, None), false),
        // The offer's sign is the title-bar button: no *Ready*, *Available* or restart item
        // in the status bar, whatever the stage.
        (snapshot(idle(), ready("9.9.9", true)), false),
        (snapshot(idle(), download("9.9.9")), false),
        (snapshot(idle(), degraded("9.9.9")), false),
        (
            snapshot(idle(), install("9.9.9", OfferStage::Applying)),
            false,
        ),
        (
            snapshot(idle(), install("9.9.9", OfferStage::InstalledRestart)),
            false,
        ),
        (
            snapshot(UpdateStatus::Checking, ready("9.9.9", false)),
            false,
        ),
        // An install offer being fetched: the progress is transient and stays.
        (
            snapshot(
                downloading("9.9.9"),
                install("9.9.9", OfferStage::Downloading),
            ),
            true,
        ),
        (snapshot(downloading("9.9.9"), None), true),
        // A background error is logged, never shown (UPD-004).
        (
            snapshot(
                UpdateStatus::Error {
                    message: "offline".into(),
                },
                ready("9.9.9", false),
            ),
            false,
        ),
    ] {
        set(&h, cx, snapshot.clone());
        let item = h.read(cx, |shell, _, cx| shell.has_update_item(cx));
        assert_eq!(item, expected, "{snapshot:?}");
        for selector in ["status-restart-to-update", "status-update-available"] {
            assert!(!shown(&h, cx, selector), "{selector} in {snapshot:?}");
        }
    }
    // A manual check shows its progress and its error (inline, with the offer untouched).
    set(
        &h,
        cx,
        snapshot(UpdateStatus::Checking, ready("9.9.9", false)),
    );
    store(cx).update(cx, |s, cx| s.check_now(cx));
    assert!(
        h.read(cx, |shell, _, cx| shell.has_update_item(cx)),
        "Checking… during a manual check"
    );
    set(
        &h,
        cx,
        snapshot(
            UpdateStatus::Error {
                message: "offline".into(),
            },
            ready("9.9.9", false),
        ),
    );
    assert!(
        h.read(cx, |shell, _, cx| shell.has_update_item(cx)),
        "the error of a manual check"
    );
    assert!(
        shown(&h, cx, "title-update"),
        "the offer stays on the button"
    );
    h.shutdown();
}

#[gpui_kit::test]
fn upd_008_notification_once_per_version_for_ready_and_download(cx: &mut TestAppContext) {
    let h = start(cx, Setup::default());
    h.wait_containers(cx);
    let before = notifications(&h, cx);
    // Install, *Ready*: one toast with *Restart now*.
    set_offer(&h, cx, ready("9.9.9", false));
    assert_eq!(notifications(&h, cx), before + 1);
    assert!(shown(&h, cx, "upd-restart-now"), "*Restart now* action");
    assert!(!shown(&h, cx, "upd-download"));
    // The same version again, with the offer gone and back (a re-check): no second toast.
    set_offer(&h, cx, None);
    set_offer(&h, cx, ready("9.9.9", false));
    set(
        &h,
        cx,
        snapshot(UpdateStatus::Checking, ready("9.9.9", false)),
    );
    assert_eq!(notifications(&h, cx), before + 1, "same version: no toast");
    // A newer version is a new announcement.
    set_offer(&h, cx, ready("9.9.10", false));
    assert_eq!(notifications(&h, cx), before + 2);
    // An install offer still being fetched is hidden and never announces.
    set_offer(&h, cx, install("9.9.11", OfferStage::Downloading));
    assert_eq!(notifications(&h, cx), before + 2, "hidden stage: no toast");
    assert!(!shown(&h, cx, "title-update"));
    // The moment it is *Ready* it does, once.
    set_offer(&h, cx, ready("9.9.11", false));
    assert_eq!(notifications(&h, cx), before + 3);
    // A *Download* offer announces on discovery, with *Download*.
    set_offer(&h, cx, download("9.9.12"));
    assert_eq!(notifications(&h, cx), before + 4);
    assert!(shown(&h, cx, "upd-download"), "*Download* action");
    set_offer(&h, cx, download("9.9.12"));
    assert_eq!(notifications(&h, cx), before + 4, "same version: no toast");
    // A failed install degrades the announced version to *Download*: still the same version.
    set_offer(&h, cx, ready("9.9.13", false));
    assert_eq!(notifications(&h, cx), before + 5);
    set_offer(&h, cx, degraded("9.9.13"));
    assert_eq!(
        notifications(&h, cx),
        before + 5,
        "degraded: no second toast"
    );
    // The notified version is the hub's persisted fact.
    assert_eq!(
        h.hub
            .config()
            .ui_state()
            .updates
            .notified_version
            .as_deref(),
        Some("9.9.13")
    );
    h.shutdown();
}

#[gpui_kit::test]
fn upd_008_restored_offer_shows_button_without_second_toast(cx: &mut TestAppContext) {
    // The offer was announced in an earlier run (`notified_version` is in `state.json`).
    let h = start(
        cx,
        Setup {
            ui_state: UiState {
                updates: UpdateState {
                    notified_version: Some("9.9.9".into()),
                    ..Default::default()
                },
                ..Default::default()
            },
            ..Setup::default()
        },
    );
    h.wait_containers(cx);
    let before = notifications(&h, cx);
    // The restored offer arrives with the first snapshot, offline: no network, no activity.
    set(
        &h,
        cx,
        snapshot(
            UpdateStatus::Error {
                message: "offline".into(),
            },
            ready("9.9.9", false),
        ),
    );
    assert!(shown(&h, cx, "title-update"), "the button is there at once");
    assert_eq!(notifications(&h, cx), before, "and no second toast");
    // A restored *Download* offer of an announced version is the same.
    assert!(h.hub.mark_update_notified("9.9.8"));
    set_offer(&h, cx, download("9.9.8"));
    assert!(shown(&h, cx, "title-update"));
    assert_eq!(notifications(&h, cx), before);
    // Only a version nobody was told about is announced.
    set_offer(&h, cx, download("9.9.7"));
    assert_eq!(notifications(&h, cx), before + 1);
    h.shutdown();
}

#[gpui_kit::test]
fn upd_008_download_click_changes_no_state(cx: &mut TestAppContext) {
    let h = start(cx, Setup::default());
    h.wait_containers(cx);
    let calls = stub_apply(cx, Ok(ApplyOutcome::Restarting));
    set_offer(&h, cx, download("9.9.9"));
    assert!(shown(&h, cx, "upd-download"), "the toast has *Download*");
    let notified = h.hub.config().ui_state().updates.clone();
    let (rev, held) = {
        let store = store(cx);
        cx.read(|cx| (store.read(cx).revision(), store.read(cx).offer().cloned()))
    };
    settle(&h, cx, "upd-download");
    click(&h, cx, "upd-download");
    // It opened the release page …
    assert_eq!(cx.opened_url().as_deref(), Some(NOTES));
    // … and nothing else happened: no hub call, no quit, the same snapshot, the same persisted
    // facts, and the button is still there.
    assert_eq!(calls.get(), 0, "no apply for a *Download* offer");
    assert!(!quit_requested(&h));
    assert_eq!(h.hub.config().ui_state().updates, notified);
    let store = store(cx);
    assert_eq!(
        cx.read(|cx| (store.read(cx).revision(), store.read(cx).offer().cloned())),
        (rev, held)
    );
    assert!(shown(&h, cx, "title-update"));
    h.shutdown();
}

#[gpui_kit::test]
fn upd_008_updated_notice_follows_last_run_version(cx: &mut TestAppContext) {
    // The previous run was an older version, however this one got installed (the updater, an
    // installer run by hand, a package manager): one 'Updated to X.Y.Z' notice, and no offer is
    // needed for it.
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
    assert_eq!(notifications(&h, cx), 1, "the 'Updated to' notice");
    // The running version is recorded, so the next start has nothing to say.
    assert_eq!(
        h.hub
            .config()
            .ui_state()
            .updates
            .last_run_version
            .as_deref(),
        Some(dk_hub::app_version())
    );
    // Updater activity doesn't touch it, and the notice isn't announced again.
    set_status(&h, cx, UpdateStatus::Checking);
    set_status(&h, cx, idle());
    assert_eq!(notifications(&h, cx), 1);
    // A later offer is announced by its own rule and leaves the notice alone.
    set_offer(&h, cx, ready("9.9.9", false));
    assert_eq!(notifications(&h, cx), 2);
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
fn shl_003_toasts_sit_bottom_right_above_the_status_bar(cx: &mut TestAppContext) {
    let h = start(cx, Setup::default());
    h.wait_containers(cx);
    h.update(cx, |_, window, cx| {
        crate::ui::notify::info(window, cx, "Saved")
    });
    h.pump(cx, 400);
    let toasts = bounds(&h, cx, "toasts").expect("the stack is drawn");
    let status = bounds(&h, cx, "status-left").expect("the status bar is drawn");
    let window = h.update(cx, |_, window, _| window.viewport_size());
    assert!(
        toasts.bottom() <= status.top(),
        "above the status bar: {toasts:?} vs {status:?}"
    );
    assert!(toasts.bottom() > window.height * 0.7, "near the bottom");
    assert!(
        toasts.right() > window.width - gpui_kit::px(40.),
        "against the right edge"
    );
    assert!(
        toasts.left() > window.width * 0.5,
        "not across the whole window"
    );
    h.shutdown();
}

#[gpui_kit::test]
fn shl_003_toast_hides_by_itself(cx: &mut TestAppContext) {
    let h = start(cx, Setup::default());
    h.wait_containers(cx);
    h.update(cx, |_, window, cx| {
        crate::ui::notify::info(window, cx, "Saved")
    });
    assert_eq!(notifications(&h, cx), 1);
    // Move the test clock in big steps: the toast needs its enter, 5 s and exit, and drawing
    // the window each 20 ms step is too slow on CI runners to get there in time.
    for _ in 0..80 {
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(100));
        cx.run_until_parked();
    }
    assert_eq!(
        cx.read(crate::ui::toast::mounted),
        0,
        "the toast hid and left"
    );
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

// ── UPD-011: stale snapshots ─────────────────────────────────────────────────────────────

#[gpui_kit::test]
fn upd_011_stale_snapshot_dropped(cx: &mut TestAppContext) {
    let h = start(cx, Setup::default());
    h.wait_containers(cx);
    let at = |revision, status, offer| UpdateSnapshot {
        status,
        offer,
        revision,
    };
    assert!(set(&h, cx, at(5, idle(), ready("9.9.9", false))));
    assert!(shown(&h, cx, "title-update"));
    // An older snapshot (a replay racing a newer value) changes nothing, not even activity.
    assert!(!set(&h, cx, at(4, idle(), None)), "revision 4 < 5: dropped");
    assert!(!set(&h, cx, at(3, UpdateStatus::Checking, None)));
    assert!(
        shown(&h, cx, "title-update"),
        "the newer offer is still shown"
    );
    let store = store(cx);
    let (revision, offer, checking) = cx.read(|cx| {
        let s = store.read(cx);
        (
            s.revision(),
            s.offer().map(|o| o.version.clone()),
            matches!(s.status(), UpdateStatus::Checking),
        )
    });
    assert_eq!(
        (revision, offer.as_deref(), checking),
        (5, Some("9.9.9"), false)
    );
    // The same revision (the replay of what is held) and a newer one are taken.
    assert!(set(&h, cx, at(5, idle(), ready("9.9.9", false))));
    assert!(set(&h, cx, at(6, idle(), None)));
    assert!(
        !shown(&h, cx, "title-update"),
        "revision 6 withdrew the offer"
    );
    h.shutdown();
}

// ── UPD-015: the title-bar button ────────────────────────────────────────────────────────

#[gpui_kit::test]
fn upd_015_button_follows_offer_stage(cx: &mut TestAppContext) {
    let h = start(cx, Setup::default());
    h.wait_containers(cx);
    assert!(!shown(&h, cx, "title-update"), "no offer: no button");
    // Every row of the stage table (plan §6.2): (offer, shown, tooltip).
    let rows: Vec<(Option<UpdateOffer>, bool, Option<&str>)> = vec![
        (None, false, None),
        (install("9.9.9", OfferStage::Downloading), false, None),
        (
            ready("9.9.9", false),
            true,
            Some("Restart to update (9.9.9)"),
        ),
        (
            ready("9.9.9", true),
            true,
            Some("Restart to update (9.9.9, admin)"),
        ),
        (install("9.9.9", OfferStage::Applying), true, None),
        (
            install("9.9.9", OfferStage::InstalledRestart),
            true,
            Some("Dockering was updated. Restart to finish"),
        ),
        (
            download("9.9.9"),
            true,
            Some("Dockering 9.9.9 is available"),
        ),
        (
            degraded("9.9.9"),
            true,
            Some("Dockering 9.9.9 is available"),
        ),
        (
            offer("9.9.9", OfferMode::Download, OfferStage::Downloading, false),
            false,
            None,
        ),
    ];
    for (o, visible, tooltip) in rows {
        set_offer(&h, cx, o.clone());
        assert_eq!(shown(&h, cx, "title-update"), visible, "{o:?}");
        let spec = h.read(cx, |shell, _, cx| shell.update_button(cx));
        assert_eq!(spec.is_some(), visible, "{o:?}");
        assert_eq!(
            spec.and_then(|b| b.tooltip).as_deref(),
            tooltip,
            "tooltip of {o:?}"
        );
    }
    h.shutdown();
}

#[gpui_kit::test]
fn upd_015_button_survives_recheck_and_failed_check(cx: &mut TestAppContext) {
    let h = start(cx, Setup::default());
    h.wait_containers(cx);
    for held in [
        ready("9.9.9", false),
        download("9.9.9"),
        install("9.9.9", OfferStage::InstalledRestart),
    ] {
        // The activity and the offer are separate: nothing a check does hides the offer.
        for status in [
            UpdateStatus::Checking,
            UpdateStatus::Error {
                message: "offline".into(),
            },
            UpdateStatus::Downloading {
                version: "9.9.10".into(),
                done: 10,
                total: 100,
            },
            idle(),
        ] {
            set(&h, cx, snapshot(status.clone(), held.clone()));
            assert!(shown(&h, cx, "title-update"), "{status:?} with {held:?}");
        }
        // A manual check in flight and its result leave it too.
        store(cx).update(cx, |s, cx| s.check_now(cx));
        h.draw(cx);
        assert!(shown(&h, cx, "title-update"), "after *Check now*: {held:?}");
    }
    h.shutdown();
}

#[gpui_kit::test]
fn upd_015_button_survives_checks_off_and_offline_start(cx: &mut TestAppContext) {
    let mut config = dk_hub::Config::default();
    config.updates.check = false;
    let h = start(
        cx,
        Setup {
            config,
            ..Setup::default()
        },
    );
    h.wait_containers(cx);
    // *Check for updates automatically* is off, and the offer was restored from disk at start:
    // the offer is still actionable (UPD-005), so the button is there.
    for held in [ready("9.9.9", false), download("9.9.9")] {
        set(
            &h,
            cx,
            snapshot(disabled(DisabledReason::Setting), held.clone()),
        );
        assert!(shown(&h, cx, "title-update"), "checks off: {held:?}");
        // An offline start: the check failed, the restored offer stays.
        set(
            &h,
            cx,
            snapshot(
                UpdateStatus::Error {
                    message: "offline".into(),
                },
                held.clone(),
            ),
        );
        assert!(shown(&h, cx, "title-update"), "offline: {held:?}");
    }
    // A hard disable (policy, the environment switch, demo, no feature, no trust keys) hides it
    // and keeps the stored offer: the hub leaves it out of the snapshot, and the UI doesn't act
    // on one it was given anyway.
    for reason in [DisabledReason::Policy, DisabledReason::Unavailable] {
        assert!(reason.is_hard());
        set(&h, cx, snapshot(disabled(reason), ready("9.9.9", false)));
        assert!(
            !shown(&h, cx, "title-update"),
            "{reason:?} hides the button"
        );
        assert_eq!(
            h.read(cx, |_, _, cx| UpdateStore::global(cx)
                .and_then(|s| s.read(cx).ready_version().map(str::to_owned))),
            None,
            "{reason:?}: nothing to restart"
        );
    }
    h.shutdown();
}

#[gpui_kit::test]
fn upd_015_ready_click_restarts(cx: &mut TestAppContext) {
    let h = start(cx, Setup::default());
    h.wait_containers(cx);
    let calls = stub_apply(cx, Ok(ApplyOutcome::Restarting));
    set_offer(&h, cx, ready("9.9.9", false));
    clear_toasts(&h, cx);
    let toasts = notifications(&h, cx);
    assert!(!quit_requested(&h));
    click(&h, cx, "title-update");
    h.draw(cx);
    assert_eq!(calls.get(), 1, "one hub apply call");
    // The new process was started: the shell saved its bounds and quit.
    assert!(quit_requested(&h), "Restarting saves the window and quits");
    assert_eq!(notifications(&h, cx), toasts, "no error toast");
    // *Restart now* in the toast is the same action.
    h.shutdown();
}

#[gpui_kit::test]
fn upd_015_restart_now_in_the_toast_restarts(cx: &mut TestAppContext) {
    let h = start(cx, Setup::default());
    h.wait_containers(cx);
    let calls = stub_apply(cx, Ok(ApplyOutcome::Restarting));
    set_offer(&h, cx, ready("9.9.9", false));
    settle(&h, cx, "upd-restart-now");
    click(&h, cx, "upd-restart-now");
    h.draw(cx);
    assert_eq!(calls.get(), 1);
    assert!(quit_requested(&h));
    // The toast's body opens the release notes; the action button's click must not also do it.
    assert_eq!(cx.opened_url(), None, "no browser for *Restart now*");
    h.shutdown();
}

#[gpui_kit::test]
fn upd_015_applying_is_disabled(cx: &mut TestAppContext) {
    let h = start(cx, Setup::default());
    h.wait_containers(cx);
    let (calls, answer) = stub_apply_pending(cx);
    // The hub says an apply runs: the button is there, disabled, and a click does nothing.
    set_offer(&h, cx, install("9.9.9", OfferStage::Applying));
    assert!(shown(&h, cx, "title-update"));
    assert_eq!(button_busy(&h, cx), Some(true), "disabled with a spinner");
    clear_toasts(&h, cx);
    let toasts = notifications(&h, cx);
    click(&h, cx, "title-update");
    h.update(cx, |_, window, cx| {
        window.dispatch_action(Box::new(crate::actions::RestartToUpdate), cx)
    });
    h.draw(cx);
    assert_eq!(calls.get(), 0, "no apply while one runs");
    assert_eq!(notifications(&h, cx), toasts, "ignored silently");
    // Our own click: the button turns busy at once, a second request is ignored silently, and
    // the hub's refusal ('already being applied') is ignored too.
    set_offer(&h, cx, ready("9.9.9", false));
    assert_eq!(button_busy(&h, cx), Some(false));
    clear_toasts(&h, cx);
    let toasts = notifications(&h, cx);
    click(&h, cx, "title-update");
    assert_eq!(calls.get(), 1);
    assert_eq!(button_busy(&h, cx), Some(true), "busy at the click");
    h.update(cx, |_, window, cx| {
        window.dispatch_action(Box::new(crate::actions::RestartToUpdate), cx)
    });
    h.draw(cx);
    assert_eq!(calls.get(), 1, "a second RestartToUpdate is ignored");
    assert_eq!(notifications(&h, cx), toasts);
    answer
        .send(Err(EngineError::protocol(ALREADY_APPLYING)))
        .expect("the apply task is waiting");
    h.draw(cx);
    assert_eq!(
        notifications(&h, cx),
        toasts,
        "'already being applied' is silent"
    );
    assert!(!quit_requested(&h));
    assert_eq!(button_busy(&h, cx), Some(false), "usable again");
    h.shutdown();
}

#[gpui_kit::test]
fn upd_015_a_failed_start_can_be_retried(cx: &mut TestAppContext) {
    let h = start(cx, Setup::default());
    h.wait_containers(cx);
    let calls = stub_apply(cx, Err(EngineError::protocol("the installer didn't start")));
    set_offer(&h, cx, ready("9.9.9", false));
    clear_toasts(&h, cx);
    let toasts = notifications(&h, cx);
    click(&h, cx, "title-update");
    h.draw(cx);
    assert_eq!(calls.get(), 1);
    assert_eq!(
        notifications(&h, cx),
        toasts + 1,
        "'Couldn't start the update'"
    );
    assert!(!quit_requested(&h), "the app and the offer stay");
    // The offer is still *Ready*, the button is usable and the retry reaches the hub.
    assert_eq!(button_busy(&h, cx), Some(false));
    clear_toasts(&h, cx);
    click(&h, cx, "title-update");
    h.draw(cx);
    assert_eq!(calls.get(), 2);
    h.shutdown();
}

#[gpui_kit::test]
fn upd_015_installed_restart_click_retries_start(cx: &mut TestAppContext) {
    let h = start(cx, Setup::default());
    h.wait_containers(cx);
    let calls = stub_apply(cx, Ok(ApplyOutcome::InstalledRestartManually));
    set_offer(&h, cx, install("9.9.9", OfferStage::InstalledRestart));
    assert!(shown(&h, cx, "title-update"));
    let toasts = notifications(&h, cx);
    click(&h, cx, "title-update");
    h.draw(cx);
    // The same call as *Ready*: the hub only retries the start. Still not started: the info
    // notice, no quit, and the button is there for another try.
    assert_eq!(calls.get(), 1);
    assert_eq!(
        notifications(&h, cx),
        toasts + 1,
        "the manual-restart notice"
    );
    assert!(!quit_requested(&h), "the app keeps running");
    assert!(shown(&h, cx, "title-update"));
    assert_eq!(button_busy(&h, cx), Some(false));
    // Now the new process starts: the shell quits.
    let calls = stub_apply(cx, Ok(ApplyOutcome::Restarting));
    clear_toasts(&h, cx);
    click(&h, cx, "title-update");
    h.draw(cx);
    assert_eq!(calls.get(), 1);
    assert!(quit_requested(&h));
    h.shutdown();
}

#[gpui_kit::test]
fn upd_015_download_click_opens_release_page(cx: &mut TestAppContext) {
    let h = start(cx, Setup::default());
    h.wait_containers(cx);
    let calls = stub_apply(cx, Ok(ApplyOutcome::Restarting));
    for (held, url) in [
        (download("9.9.9"), NOTES),
        (
            degraded("9.9.9").map(|o| UpdateOffer {
                notes_url: "https://example.test/notes/9.9.9".into(),
                ..o
            }),
            "https://example.test/notes/9.9.9",
        ),
    ] {
        set_offer(&h, cx, held.clone());
        clear_toasts(&h, cx);
        click(&h, cx, "title-update");
        assert_eq!(cx.opened_url().as_deref(), Some(url), "{held:?}");
    }
    // Opening the page is all it does.
    assert_eq!(calls.get(), 0);
    assert!(!quit_requested(&h));
    h.shutdown();
}

#[gpui_kit::test]
fn upd_015_same_first_button_on_every_os(cx: &mut TestAppContext) {
    use crate::shell::update_button::BUTTON_ID;
    use gpui_kit::AssetSource as _;
    let h = start(cx, Setup::default());
    h.wait_containers(cx);
    set_offer(&h, cx, ready("9.9.9", false));
    // The button is the first child of the action group: it starts at the group's left edge and
    // sits left of *Refresh*, on the same row.
    let group = bounds(&h, cx, "title-actions").expect("the actions group");
    let update = bounds(&h, cx, "title-update").expect("the update button");
    let refresh = bounds(&h, cx, "title-refresh").expect("the Refresh button");
    assert_eq!(BUTTON_ID, "title-update");
    assert_eq!(
        update.left(),
        group.left(),
        "first child, nothing before it"
    );
    assert!(update.right() <= refresh.left(), "left of Refresh");
    assert!((update.center().y - refresh.center().y).abs() < gpui_kit::px(1.));
    // With no offer the group starts at *Refresh* and nothing else moved.
    set_offer(&h, cx, None);
    let group = bounds(&h, cx, "title-actions").expect("the actions group");
    let refresh = bounds(&h, cx, "title-refresh").expect("the Refresh button");
    assert_eq!(group.left(), refresh.left());
    // The icon is Lucide's `CircleArrowUp`; the app's asset source (its own `icon_assets!` list
    // first) can load it.
    let path = crate::assets::Lucide::CircleArrowUp.path();
    assert!(
        matches!(crate::assets::AppAssets.load(&path), Ok(Some(_))),
        "{path} loads from the app's assets"
    );
    // No platform branch in the code that builds the button or the Settings section: it is the
    // same code on Windows, macOS and Linux (the overflow button beside it, absent on macOS,
    // SHL-021, is built elsewhere and doesn't move or change this one).
    for (name, source) in [
        (
            "shell/update_button.rs",
            include_str!("shell/update_button.rs"),
        ),
        (
            "pages/settings/updates.rs",
            include_str!("pages/settings/updates.rs"),
        ),
    ] {
        for needle in [
            "target_os",
            "cfg!(",
            "cfg(windows",
            "cfg(unix",
            "cfg(target",
        ] {
            assert!(
                !source.contains(needle),
                "{name} branches on the OS: {needle}"
            );
        }
    }
    h.shutdown();
}

#[gpui_kit::test]
fn upd_015_accessible_name(cx: &mut TestAppContext) {
    let h = start(cx, Setup::default());
    h.wait_containers(cx);
    for held in [
        ready("9.9.9", false),
        download("9.9.9"),
        install("9.9.9", OfferStage::InstalledRestart),
        install("9.9.9", OfferStage::Applying),
    ] {
        set_offer(&h, cx, held.clone());
        let name = h
            .read(cx, |shell, _, cx| shell.update_button(cx))
            .map(|b| b.name());
        assert_eq!(
            name.as_deref(),
            Some("Update available: Dockering 9.9.9"),
            "{held:?}"
        );
    }
    // The name is what `Button::accessibility_label` gets (the button has no text of its own).
    // The platform accessibility tree is only built for an assistive technology, so a test
    // window has none to read: the end-to-end test (REL-085) reads the name through AT-SPI.
    assert!(include_str!("shell/update_button.rs").contains(".accessibility_label(spec.name())"));
    h.shutdown();
}

#[gpui_kit::test]
fn upd_015_focus_returns_to_title_bar(cx: &mut TestAppContext) {
    let h = start(cx, Setup::default());
    h.wait_containers(cx);
    let title = h.read(cx, |s, _, _| s.title_focus().clone());
    let button = h.read(cx, |s, _, _| s.update_focus().clone());
    for gone in [
        // withdrawn, expired, or applied (a new offer of nothing)
        snapshot(idle(), None),
        // a hard disable
        snapshot(disabled(DisabledReason::Policy), ready("9.9.9", false)),
        // replaced by one that isn't actionable yet
        snapshot(idle(), install("9.9.10", OfferStage::Downloading)),
    ] {
        set_offer(&h, cx, ready("9.9.9", false));
        h.focus(cx, &button);
        assert!(h.is_focused(cx, &button), "the button holds focus");
        set(&h, cx, gone.clone());
        assert!(!shown(&h, cx, "title-update"), "{gone:?}");
        // Focus is on the title bar, not nowhere and not dragged into the page.
        assert!(
            h.is_focused(cx, &title),
            "focus returns to the title bar: {gone:?}"
        );
        assert_eq!(
            h.read(cx, |s, window, cx| s.current_region(window, cx)),
            Some(Region::TitleBar)
        );
    }
    // Focus somewhere else stays where it is when the button goes.
    set_offer(&h, cx, ready("9.9.9", false));
    h.focus_table(cx);
    set_offer(&h, cx, None);
    assert!(!h.is_focused(cx, &title));
    assert_eq!(
        h.read(cx, |s, window, cx| s.current_region(window, cx)),
        Some(Region::Content)
    );
    h.shutdown();
}

// ── KBD-076: keyboard ────────────────────────────────────────────────────────────────────

#[gpui_kit::test]
fn kbd_076_title_update_button_in_tab_order_and_f6(cx: &mut TestAppContext) {
    let h = start(cx, Setup::default());
    h.wait_containers(cx);
    let (switcher, button) = h.read(cx, |s, _, _| {
        (s.switcher_button_focus().clone(), s.update_focus().clone())
    });
    let region = |cx: &mut TestAppContext| h.read(cx, |s, window, cx| s.current_region(window, cx));
    set_offer(&h, cx, ready("9.9.9", false));
    // Tab order: the engine switcher, then the update button (the first of the action group),
    // then *Refresh* and the rest.
    h.focus(cx, &switcher);
    h.press(cx, "tab");
    assert!(
        h.is_focused(cx, &button),
        "Tab from the switcher reaches the button"
    );
    assert_eq!(
        region(cx),
        Some(Region::TitleBar),
        "the button is in the title region"
    );
    h.press(cx, "tab");
    assert!(!h.is_focused(cx, &button));
    assert_eq!(region(cx), Some(Region::TitleBar));
    h.press(cx, "shift-tab");
    assert!(h.is_focused(cx, &button), "and Shift+Tab comes back");
    // F6 regions: away and back, the title region remembers its button.
    h.press(cx, "f6");
    assert_ne!(region(cx), Some(Region::TitleBar));
    h.press(cx, "shift-f6");
    assert_eq!(region(cx), Some(Region::TitleBar));
    assert!(
        h.is_focused(cx, &button),
        "F6 restores the last-focused child"
    );
    // Enter on the focused button is a click: it runs RestartToUpdate; Space does too.
    let calls = stub_apply(cx, Err(EngineError::protocol("not now")));
    h.press(cx, "enter");
    assert_eq!(calls.get(), 1, "Enter runs RestartToUpdate");
    h.press(cx, "space");
    assert_eq!(calls.get(), 2, "Space runs RestartToUpdate");
    // A *Download* offer: the same key runs ViewReleaseNotes.
    set_offer(&h, cx, download("9.9.9"));
    h.focus(cx, &button);
    h.press(cx, "enter");
    assert_eq!(cx.opened_url().as_deref(), Some(NOTES));
    // While *Applying* the key does nothing.
    set_offer(&h, cx, install("9.9.9", OfferStage::Applying));
    h.focus(cx, &button);
    h.press(cx, "enter");
    assert_eq!(calls.get(), 2);
    h.shutdown();
}

#[gpui_kit::test]
fn kbd_076_restart_to_update_palette_entry_follows_the_stage(cx: &mut TestAppContext) {
    // *Restart to Update* is offered for a *Ready* or *Installed, restart* offer only.
    let h = start(cx, Setup::default());
    h.wait_containers(cx);
    let restartable = |cx: &mut TestAppContext| {
        cx.read(|cx| {
            UpdateStore::global(cx)
                .and_then(|u| u.read(cx).restartable_version().map(str::to_owned))
        })
    };
    for (held, expected) in [
        (ready("9.9.9", false), Some("9.9.9")),
        (
            install("9.9.9", OfferStage::InstalledRestart),
            Some("9.9.9"),
        ),
        (install("9.9.9", OfferStage::Applying), None),
        (install("9.9.9", OfferStage::Downloading), None),
        (download("9.9.9"), None),
        (degraded("9.9.9"), None),
        (None, None),
    ] {
        set_offer(&h, cx, held.clone());
        assert_eq!(restartable(cx).as_deref(), expected, "{held:?}");
    }
    h.shutdown();
}

// ── SET-090: Settings → Updates ──────────────────────────────────────────────────────────

/// The Tab stops from `start`, in order (until the sequence repeats).
fn tab_stops(
    h: &Harness,
    cx: &mut TestAppContext,
    start: &gpui_kit::FocusHandle,
) -> Vec<gpui_kit::FocusHandle> {
    h.focus(cx, start);
    let mut seen: Vec<gpui_kit::FocusHandle> = Vec::new();
    for _ in 0..40 {
        h.press(cx, "tab");
        let focused = h.read(cx, |_, window, cx| window.focused(cx));
        match focused {
            Some(f) if !seen.contains(&f) => seen.push(f),
            _ => break,
        }
    }
    seen
}

#[gpui_kit::test]
fn kbd_076_settings_offer_button_focus_falls_back_to_the_settings_nav(cx: &mut TestAppContext) {
    let h = start(cx, Setup::default());
    h.wait_containers(cx);
    open_updates(&h, cx);
    let page = h
        .read(cx, |s, _, _| match s.page() {
            crate::shell::ShellPage::Settings(p) => Some(p.clone()),
            _ => None,
        })
        .expect("settings page");
    let root = cx.read(|cx| page.read(cx).focus().clone());
    set_offer(&h, cx, None);
    let without = tab_stops(&h, cx, &root);
    set_offer(&h, cx, ready("9.9.9", false));
    clear_toasts(&h, cx);
    let with = tab_stops(&h, cx, &root);
    // The offer adds two Tab stops: the title-bar button and, in the page, *Restart to update*.
    // Both are buttons, with no chord of their own, reached with Tab.
    let title = h.read(cx, |s, _, _| s.title_focus().clone());
    let added: Vec<_> = h.read(cx, |_, window, _| {
        with.iter()
            .filter(|f| !without.contains(f) && !title.contains(f, window))
            .cloned()
            .collect()
    });
    assert_eq!(
        added.len(),
        1,
        "the offer's button in the page is one more Tab stop"
    );
    // It disappears (applied, withdrawn, expired) while it holds focus: focus goes to the
    // Settings owner, which on Settings routes is the section nav (SET-080, KBD-007), and is
    // never lost.
    h.focus(cx, &added[0]);
    assert!(h.is_focused(cx, &added[0]));
    set_offer(&h, cx, None);
    h.draw(cx);
    let (region, has_focus) = h.read(cx, |s, window, cx| {
        (s.current_region(window, cx), window.focused(cx).is_some())
    });
    assert!(has_focus, "focus is never lost");
    assert_eq!(region, Some(Region::Sidebar), "the Settings section nav");
    h.shutdown();
}

#[gpui_kit::test]
fn set_090_offer_row_and_reason(cx: &mut TestAppContext) {
    let h = start(cx, Setup::default());
    h.wait_containers(cx);
    open_updates(&h, cx);
    assert!(!shown(&h, cx, "upd-offer-row"), "no offer: no row");
    // (offer, restart button, download button): the same on every OS.
    let rows: Vec<(Option<UpdateOffer>, bool, bool)> = vec![
        (ready("9.9.9", false), true, false),
        (install("9.9.9", OfferStage::InstalledRestart), true, false),
        (install("9.9.9", OfferStage::Applying), true, false),
        (install("9.9.9", OfferStage::Downloading), false, false),
        (download("9.9.9"), false, true),
        (degraded("9.9.9"), false, true),
    ];
    for (held, restart, download) in rows {
        set_offer(&h, cx, held.clone());
        assert!(shown(&h, cx, "upd-offer-row"), "{held:?}");
        assert!(shown(&h, cx, "upd-offer-version"), "the offered version");
        assert_eq!(shown(&h, cx, "upd-offer-restart"), restart, "{held:?}");
        assert_eq!(shown(&h, cx, "upd-offer-download"), download, "{held:?}");
        assert!(!shown(&h, cx, "upd-note"), "no note without a hard disable");
    }
    // A hard disable keeps the row, suppresses its actions and says why.
    for reason in [DisabledReason::Policy, DisabledReason::Unavailable] {
        set(&h, cx, snapshot(disabled(reason), ready("9.9.9", false)));
        assert!(shown(&h, cx, "upd-offer-row"), "{reason:?}");
        assert!(!shown(&h, cx, "upd-offer-restart"), "{reason:?}");
        assert!(!shown(&h, cx, "upd-offer-download"), "{reason:?}");
        assert!(shown(&h, cx, "upd-note"), "{reason:?} names its reason");
    }
    // The buttons only dispatch actions: *Restart to update* reaches the hub call, *Download*
    // opens the release page.
    let calls = stub_apply(cx, Err(EngineError::protocol("not now")));
    set_offer(&h, cx, ready("9.9.9", false));
    clear_toasts(&h, cx);
    click(&h, cx, "upd-offer-restart");
    h.draw(cx);
    assert_eq!(calls.get(), 1);
    set_offer(&h, cx, download("9.9.9"));
    clear_toasts(&h, cx);
    click(&h, cx, "upd-offer-download");
    assert_eq!(cx.opened_url().as_deref(), Some(NOTES));
    assert_eq!(calls.get(), 1);
    h.shutdown();
}

#[gpui_kit::test]
fn set_090_checking_does_not_erase_offer(cx: &mut TestAppContext) {
    let h = start(cx, Setup::default());
    h.wait_containers(cx);
    open_updates(&h, cx);
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
    ] {
        set(&h, cx, snapshot(status.clone(), ready("9.9.9", false)));
        assert!(shown(&h, cx, "upd-offer-row"), "{status:?}");
        assert!(shown(&h, cx, "upd-offer-restart"), "{status:?}");
    }
    // *Check now* in flight (its inline progress) and finished (its inline result).
    set(&h, cx, snapshot(UpdateStatus::Checking, download("9.9.9")));
    let updates = store(cx);
    updates.update(cx, |s, cx| s.check_now(cx));
    assert!(shown(&h, cx, "upd-offer-download"), "while the check runs");
    h.wait_until(cx, "manual check done", |_, cx| {
        matches!(
            updates.read(cx).manual(),
            Some(crate::state::ManualCheck::Done(_))
        )
    });
    assert!(shown(&h, cx, "upd-offer-download"), "after the check");
    h.shutdown();
}

#[gpui_kit::test]
fn set_090_setting_off_keeps_check_now_and_offer(cx: &mut TestAppContext) {
    let mut config = dk_hub::Config::default();
    config.updates.check = false;
    let h = start(
        cx,
        Setup {
            config,
            ..Setup::default()
        },
    );
    h.wait_containers(cx);
    open_updates(&h, cx);
    set(
        &h,
        cx,
        snapshot(disabled(DisabledReason::Setting), ready("9.9.9", false)),
    );
    // The switch is off, yet *Check now* is there and works, and the offer is actionable.
    assert!(!cx.read(|cx| AppState::config(cx).updates.check));
    assert!(shown(&h, cx, "upd-check-now"));
    assert!(shown(&h, cx, "upd-offer-restart"));
    assert!(
        !shown(&h, cx, "upd-note"),
        "no note: this is the user's choice"
    );
    clear_toasts(&h, cx);
    click(&h, cx, "upd-check-now");
    let updates = store(cx);
    h.wait_until(cx, "manual check ran", |_, cx| {
        updates.read(cx).manual().is_some()
    });
    // And the title-bar button follows.
    assert!(shown(&h, cx, "title-update"));
    h.shutdown();
}

#[gpui_kit::test]
fn set_090_policy_note_is_windows_only(cx: &mut TestAppContext) {
    let h = start(cx, Setup::default());
    h.wait_containers(cx);
    open_updates(&h, cx);
    set_status(&h, cx, disabled(DisabledReason::Policy));
    assert!(shown(&h, cx, "upd-note"), "the policy names itself");
    assert!(!shown(&h, cx, "upd-check-now"), "*Check now* is hidden");
    // The note says the policy is a Windows registry value and claims nothing for other systems.
    let note = crate::strings::UPD_POLICY;
    assert!(note.contains("administrator policy"), "{note}");
    assert!(note.contains("Windows"), "{note}");
    for other in ["macOS", "Mac ", "Linux", "Unix", "plist", "/etc"] {
        assert!(!note.contains(other), "{note} claims a policy on {other}");
    }
    // Only a policy says it: the setting, a plain idle state and the other hard disable don't.
    set_status(&h, cx, disabled(DisabledReason::Setting));
    assert!(!shown(&h, cx, "upd-note"));
    assert!(shown(&h, cx, "upd-check-now"));
    set_status(&h, cx, idle());
    assert!(!shown(&h, cx, "upd-note"));
    set_status(&h, cx, disabled(DisabledReason::Unavailable));
    assert!(shown(&h, cx, "upd-note"));
    assert_ne!(crate::strings::UPD_UNAVAILABLE, note);
    assert!(!crate::strings::UPD_UNAVAILABLE.contains("policy"));
    h.shutdown();
}
