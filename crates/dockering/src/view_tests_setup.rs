//! Start rows, the set-up dialog and its progress (ENG-146…162). Real `HubHandle` over the fake
//! factory: its setup offers and scripted progress stand in for a backend.

use std::sync::Arc;

use dk_core::fake::{FakeEngine, FakeSetupStep};
use dk_core::{
    EngineError, EngineFactory, EngineId, EngineState, SetupChoice, SetupOffer, SetupPhase,
    SetupProgress,
};
use dk_hub::Feed;
use gpui_kit::TestAppContext;

use crate::actions::{SetUpEngine, StartEngine, StartListedEngine};
use crate::commands::{self, CommandContext};
use crate::keymap::{DEFAULT_KEYMAP, Os};
use crate::strings as s;
use crate::testing::{Harness, Setup, start_with_factories};
use crate::ui::setup_dialog::{SetupDialog, Stage};
use crate::view_tests_start::{ID, StoppedFactory, prompt};

const OFFER: &str = "dk-offer";

fn choice(id: &str, label: &str, download: u64, disk: u64) -> SetupChoice {
    SetupChoice {
        id: id.into(),
        label: label.into(),
        description: format!("About {label}"),
        download_bytes: download,
        disk_bytes: disk,
    }
}

fn offer(choices: Vec<SetupChoice>) -> SetupOffer {
    SetupOffer {
        id: OFFER.into(),
        title: "Set up an engine here".into(),
        body: "A dedicated engine is created for you.".into(),
        action_label: "Create an engine".into(),
        choices,
    }
}

fn two() -> SetupOffer {
    offer(vec![
        choice("alpha", "Alpha", 60_000_000, 300_000_000),
        choice("beta", "Beta", 200_000_000, 1_200_000_000),
    ])
}

fn phase(phase: SetupPhase, fraction: Option<f32>, detail: &str) -> FakeSetupStep {
    FakeSetupStep::Progress(SetupProgress::Phase {
        phase,
        fraction,
        detail: detail.into(),
    })
}

/// A first-run page: the demo engine fails, one engine is stopped and startable, and the
/// factory offers `offer`.
fn landing(cx: &mut TestAppContext, offers: Vec<SetupOffer>) -> (Harness, Arc<StoppedFactory>) {
    let f = StoppedFactory::new(Some(prompt()));
    let h = start_with_factories(
        cx,
        Setup {
            failing: vec![("demo".into(), EngineError::unreachable("down"))],
            ..Default::default()
        },
        vec![f.clone() as Arc<dyn EngineFactory>],
    );
    h.factory.set_setup_offers(offers);
    h.wait_until(cx, "landing rows", |_, _| true);
    h.wait_until(cx, "stopped engine listed, nothing active", |_, cx| {
        let list = h.shell.read(cx).engines().read(cx);
        list.active().is_none()
            && list
                .get(&EngineId::new(ID))
                .is_some_and(|e| matches!(e.state, EngineState::Stopped))
    });
    (h, f)
}

fn rendered(h: &Harness, cx: &mut TestAppContext, selector: &'static str) -> bool {
    h.draw(cx);
    gpui_kit::VisualTestContext::from_window(h.any_window(), cx)
        .debug_bounds(selector)
        .is_some()
}

fn wait_rendered(h: &Harness, cx: &mut TestAppContext, selector: &'static str) {
    h.wait_until(cx, selector, |window, _| {
        window.refresh();
        true
    });
    for _ in 0..200 {
        if rendered(h, cx, selector) {
            return;
        }
        h.pump(cx, 20);
    }
    panic!("never rendered: {selector}");
}

fn open_dialog(h: &Harness, cx: &mut TestAppContext) -> gpui_kit::Entity<SetupDialog> {
    h.update(cx, |_, window, cx| {
        window.dispatch_action(Box::new(SetUpEngine { offer: "".into() }), cx)
    });
    h.wait_until(cx, "dialog open", |_, cx| {
        h.shell.read(cx).setup_dialog().is_some()
    });
    h.draw(cx);
    h.read(cx, |s, _, _| s.setup_dialog().cloned()).unwrap()
}

fn stage(d: &gpui_kit::Entity<SetupDialog>, cx: &mut TestAppContext) -> Stage {
    cx.read(|cx| d.read(cx).stage().clone())
}

fn selected(d: &gpui_kit::Entity<SetupDialog>, cx: &mut TestAppContext) -> Option<usize> {
    cx.read(|cx| d.read(cx).selected())
}

fn pick(h: &Harness, cx: &mut TestAppContext, d: &gpui_kit::Entity<SetupDialog>, ix: usize) {
    h.update(cx, |_, _, cx| d.update(cx, |d, cx| d.select(ix, cx)));
    h.draw(cx);
}

fn run(h: &Harness, cx: &mut TestAppContext, d: &gpui_kit::Entity<SetupDialog>) {
    h.update(cx, |_, window, cx| d.update(cx, |d, cx| d.run(window, cx)));
    h.draw(cx);
}

#[gpui_kit::test]
fn eng_158_first_run_lists_startable_and_offers(cx: &mut TestAppContext) {
    let (h, _f) = landing(cx, vec![two()]);
    wait_rendered(&h, cx, "landing-start-row-stopped-one");
    wait_rendered(&h, cx, "landing-setup-row-dk-offer");
    assert!(rendered(&h, cx, "landing-rows"));
    // The primary action takes initial focus (ENG-159).
    let focus = h.read(cx, |s, _, _| s.start_focus().clone());
    h.wait_until(cx, "primary focused", |window, _| focus.is_focused(window));
    h.shutdown();
}

#[gpui_kit::test]
fn eng_158_start_row_activates_and_starts(cx: &mut TestAppContext) {
    let (h, f) = landing(cx, vec![]);
    wait_rendered(&h, cx, "landing-start-row-stopped-one");
    assert!(!rendered(&h, cx, "landing-setup-row-dk-offer"));
    h.update(cx, |_, window, cx| {
        window.dispatch_action(Box::new(StartListedEngine { id: ID.into() }), cx)
    });
    h.wait_until(cx, "started and connected", |_, cx| {
        h.shell
            .read(cx)
            .engines()
            .read(cx)
            .active()
            .is_some_and(|e| e.id().as_str() == ID && e.state.is_usable())
    });
    assert_eq!(f.starts(), 1);
    h.shutdown();
}

#[gpui_kit::test]
fn eng_147_failed_start_shows_error_hint_retry_and_start(cx: &mut TestAppContext) {
    let (h, f) = landing(cx, vec![]);
    *f.start_error.lock().unwrap() = Some(EngineError::unreachable_with_hint(
        "it did not come up",
        "Finish the sign-in window",
    ));
    h.update(cx, |_, window, cx| {
        window.dispatch_action(Box::new(StartListedEngine { id: ID.into() }), cx)
    });
    h.wait_until(cx, "failed", |_, cx| {
        h.shell
            .read(cx)
            .engines()
            .read(cx)
            .active()
            .is_some_and(|e| matches!(e.state, EngineState::Failed { .. }))
    });
    wait_rendered(&h, cx, "engine-unreachable");
    assert!(
        rendered(&h, cx, "failed-start-button"),
        "Start is offered again"
    );
    // Start again: the engine comes up.
    *f.start_error.lock().unwrap() = None;
    h.update(cx, |_, window, cx| {
        window.dispatch_action(Box::new(StartEngine), cx)
    });
    h.wait_until(cx, "connected", |_, cx| {
        h.shell
            .read(cx)
            .engines()
            .read(cx)
            .active()
            .is_some_and(|e| e.state.is_usable())
    });
    assert_eq!(f.starts(), 2);
    h.shutdown();
}

#[gpui_kit::test]
fn eng_151_nothing_happens_before_consent(cx: &mut TestAppContext) {
    let (h, _f) = landing(cx, vec![two()]);
    let d = open_dialog(&h, cx);
    assert!(h.has_dialog(cx));
    assert_eq!(stage(&d, cx), Stage::Choose);
    assert!(h.factory.setups().is_empty(), "opening starts nothing");
    h.press(cx, "enter");
    assert!(
        h.factory.setups().is_empty(),
        "Enter without a pick starts nothing"
    );
    h.press(cx, "escape");
    assert!(!h.has_dialog(cx));
    assert!(h.factory.setups().is_empty());
    h.shutdown();
}

#[gpui_kit::test]
fn eng_162_dialog_opens_with_nothing_selected(cx: &mut TestAppContext) {
    let (h, _f) = landing(cx, vec![two()]);
    let d = open_dialog(&h, cx);
    assert_eq!(selected(&d, cx), None);
    assert!(
        cx.read(|cx| d.read(cx).consent_lines().is_none()),
        "no size figures yet"
    );
    assert!(rendered(&h, cx, "setup-hint"), "the muted hint is shown");
    assert!(!rendered(&h, cx, "setup-download-size"));
    assert!(!rendered(&h, cx, "setup-disk-size"));
    h.shutdown();
}

#[gpui_kit::test]
fn eng_162_set_up_disabled_until_a_choice_is_made(cx: &mut TestAppContext) {
    let (h, _f) = landing(cx, vec![two()]);
    let d = open_dialog(&h, cx);
    assert!(!cx.read(|cx| d.read(cx).can_run()));
    run(&h, cx, &d);
    assert_eq!(
        stage(&d, cx),
        Stage::Choose,
        "a disabled Set up does nothing"
    );
    assert!(h.factory.setups().is_empty());
    pick(&h, cx, &d, 0);
    assert!(cx.read(|cx| d.read(cx).can_run()));
    assert!(
        !rendered(&h, cx, "setup-hint"),
        "the hint goes once a choice is picked"
    );
    h.shutdown();
}

#[gpui_kit::test]
fn eng_162_single_choice_must_still_be_picked(cx: &mut TestAppContext) {
    let one = offer(vec![choice("only", "Only", 1_000_000, 2_000_000)]);
    let (h, _f) = landing(cx, vec![one]);
    let d = open_dialog(&h, cx);
    assert_eq!(selected(&d, cx), None);
    assert!(!cx.read(|cx| d.read(cx).can_run()));
    pick(&h, cx, &d, 0);
    assert!(cx.read(|cx| d.read(cx).can_run()));
    h.shutdown();
}

#[gpui_kit::test]
fn eng_162_no_choice_is_remembered_between_opens(cx: &mut TestAppContext) {
    let (h, _f) = landing(cx, vec![two()]);
    let d = open_dialog(&h, cx);
    pick(&h, cx, &d, 1);
    assert_eq!(selected(&d, cx), Some(1));
    h.press(cx, "escape");
    assert!(!h.has_dialog(cx));
    let d2 = open_dialog(&h, cx);
    assert_eq!(
        selected(&d2, cx),
        None,
        "the dialog opens unselected every time"
    );
    h.shutdown();
}

#[test]
fn eng_162_no_recommended_marker() {
    for (name, text) in [
        ("strings", include_str!("strings.rs")),
        ("setup dialog", include_str!("ui/setup_dialog.rs")),
        ("engine views", include_str!("shell/engine_views.rs")),
    ] {
        assert!(
            !text.to_lowercase().contains("recommend"),
            "{name} ranks a choice"
        );
    }
}

#[gpui_kit::test]
fn eng_162_consent_sizes_follow_selection(cx: &mut TestAppContext) {
    let (h, _f) = landing(cx, vec![two()]);
    let d = open_dialog(&h, cx);
    pick(&h, cx, &d, 0);
    let first = cx
        .read(|cx| d.read(cx).consent_lines())
        .expect("sizes after a pick");
    assert_eq!(
        first.0,
        s::setup_download_size(&dk_core::format::format_size(60_000_000))
    );
    assert_eq!(
        first.1,
        s::setup_disk_size(&dk_core::format::format_size(300_000_000))
    );
    assert!(rendered(&h, cx, "setup-download-size"));
    assert!(rendered(&h, cx, "setup-disk-size"));
    pick(&h, cx, &d, 1);
    let second = cx.read(|cx| d.read(cx).consent_lines()).unwrap();
    assert_eq!(
        second.0,
        s::setup_download_size(&dk_core::format::format_size(200_000_000))
    );
    assert_eq!(
        second.1,
        s::setup_disk_size(&dk_core::format::format_size(1_200_000_000))
    );
    h.shutdown();
}

#[gpui_kit::test]
fn eng_162_radio_keyboard_and_focus(cx: &mut TestAppContext) {
    let (h, _f) = landing(cx, vec![two()]);
    let focus = h.read(cx, |s, _, _| s.start_focus().clone());
    h.wait_until(cx, "page focused", |window, _| focus.is_focused(window));
    let d = open_dialog(&h, cx);
    h.wait_until(cx, "group focused", |window, cx| {
        d.read(cx).group_focus().is_focused(window)
    });
    assert_eq!(selected(&d, cx), None);
    h.press(cx, "space");
    assert_eq!(selected(&d, cx), None, "Space alone picks nothing");
    h.press(cx, "down");
    assert_eq!(selected(&d, cx), Some(0));
    h.press(cx, "down");
    assert_eq!(selected(&d, cx), Some(1));
    h.press(cx, "up");
    assert_eq!(selected(&d, cx), Some(0));
    // Tab reaches Set up, then Cancel.
    let (run_focus, cancel_focus) = cx.read(|cx| {
        (
            d.read(cx).setup_focus().clone(),
            d.read(cx).cancel_focus().clone(),
        )
    });
    h.press(cx, "tab");
    assert!(h.is_focused(cx, &run_focus), "Set up follows the group");
    h.press(cx, "tab");
    assert!(h.is_focused(cx, &cancel_focus), "Cancel follows Set up");
    // Esc cancels and focus returns to the page (KBD-002).
    h.press(cx, "escape");
    assert!(!h.has_dialog(cx));
    h.wait_until(cx, "focus back", |window, _| focus.is_focused(window));
    assert!(h.factory.setups().is_empty());
    h.shutdown();
}

#[gpui_kit::test]
fn eng_154_enter_on_enabled_set_up_runs_it(cx: &mut TestAppContext) {
    let (h, _f) = landing(cx, vec![two()]);
    h.factory.set_setup_script(vec![
        phase(SetupPhase::Checking, None, ""),
        FakeSetupStep::Pending,
    ]);
    let d = open_dialog(&h, cx);
    h.press(cx, "down");
    h.press(cx, "down");
    h.press(cx, "tab");
    h.press(cx, "enter");
    h.wait_until(cx, "running", |_, cx| {
        matches!(d.read(cx).stage(), Stage::Running { .. })
    });
    assert_eq!(
        h.factory.setups(),
        vec![(OFFER.to_owned(), "beta".to_owned())]
    );
    h.shutdown();
}

#[gpui_kit::test]
fn eng_154_progress_view_shows_phase_detail_and_bar(cx: &mut TestAppContext) {
    let (h, _f) = landing(cx, vec![two()]);
    h.factory.set_setup_script(vec![
        phase(SetupPhase::Downloading, Some(0.5), "12 of 24 MB"),
        FakeSetupStep::Pending,
    ]);
    let d = open_dialog(&h, cx);
    pick(&h, cx, &d, 0);
    run(&h, cx, &d);
    h.wait_until(cx, "downloading", |_, cx| {
        matches!(
            d.read(cx).stage(),
            Stage::Running {
                phase: SetupPhase::Downloading,
                ..
            }
        )
    });
    assert!(rendered(&h, cx, "setup-progress"));
    assert_eq!(
        s::setup_phase(SetupPhase::Downloading, Some(0.5)),
        "Downloading · 50%"
    );
    h.shutdown();
}

#[gpui_kit::test]
fn eng_154_cancel_drops_the_setup(cx: &mut TestAppContext) {
    let (h, _f) = landing(cx, vec![two()]);
    h.factory.set_setup_script(vec![
        phase(SetupPhase::Checking, None, ""),
        FakeSetupStep::Pending,
    ]);
    let d = open_dialog(&h, cx);
    pick(&h, cx, &d, 0);
    run(&h, cx, &d);
    h.wait_until(cx, "running", |_, cx| {
        matches!(d.read(cx).stage(), Stage::Running { .. })
    });
    assert_eq!(h.factory.setups().len(), 1);
    h.press(cx, "escape");
    assert!(!h.has_dialog(cx));
    h.wait_until(cx, "stream dropped", |_, _| h.factory.setups_dropped() == 1);
    // The hub frees the slot: a new setup is accepted, not a Conflict.
    let d2 = open_dialog(&h, cx);
    pick(&h, cx, &d2, 0);
    run(&h, cx, &d2);
    h.wait_until(cx, "second run", |_, cx| {
        matches!(d2.read(cx).stage(), Stage::Running { .. })
    });
    assert_eq!(h.factory.setups().len(), 2);
    h.shutdown();
}

#[gpui_kit::test]
fn eng_154_stale_setup_events_dropped(cx: &mut TestAppContext) {
    let (h, _f) = landing(cx, vec![two()]);
    h.factory
        .set_setup_script(vec![FakeSetupStep::Error(EngineError::unreachable(
            "no network",
        ))]);
    let d = open_dialog(&h, cx);
    pick(&h, cx, &d, 0);
    run(&h, cx, &d);
    h.wait_until(cx, "failed", |_, cx| {
        matches!(d.read(cx).stage(), Stage::Failed { .. })
    });
    let old = cx.read(|cx| d.read(cx).setup_id());
    // *Try again* returns to the unselected choice screen; a new run gets a new id.
    h.update(cx, |_, _, cx| d.update(cx, |d, cx| d.try_again(cx)));
    assert_eq!(stage(&d, cx), Stage::Choose);
    assert_eq!(selected(&d, cx), None);
    h.factory.set_setup_script(vec![
        phase(SetupPhase::Checking, None, ""),
        FakeSetupStep::Pending,
    ]);
    pick(&h, cx, &d, 1);
    run(&h, cx, &d);
    h.wait_until(cx, "running again", |_, cx| {
        matches!(d.read(cx).stage(), Stage::Running { .. })
    });
    let now = cx.read(|cx| d.read(cx).setup_id());
    assert!(now > old);
    fn late(
        h: &Harness,
        cx: &mut TestAppContext,
        d: &gpui_kit::Entity<SetupDialog>,
        id: u64,
        item: dk_core::EngineResult<Feed<SetupProgress>>,
    ) {
        h.update(cx, |_, window, cx| {
            d.update(cx, |d, cx| d.apply(id, item, window, cx))
        })
    }
    // Late events of the older run change nothing, not even a failure or a Done.
    late(
        &h,
        cx,
        &d,
        old,
        Ok(Feed::Item(SetupProgress::Phase {
            phase: SetupPhase::Importing,
            fraction: Some(0.9),
            detail: "late".into(),
        })),
    );
    late(
        &h,
        cx,
        &d,
        old,
        Err(EngineError::unreachable("late failure")),
    );
    assert!(matches!(
        stage(&d, cx),
        Stage::Running {
            phase: SetupPhase::Checking,
            ..
        }
    ));
    late(
        &h,
        cx,
        &d,
        now,
        Ok(Feed::Item(SetupProgress::Phase {
            phase: SetupPhase::Importing,
            fraction: None,
            detail: String::new(),
        })),
    );
    assert!(matches!(
        stage(&d, cx),
        Stage::Running {
            phase: SetupPhase::Importing,
            ..
        }
    ));
    h.shutdown();
}

#[gpui_kit::test]
fn eng_154_error_shows_message_and_hint_with_try_again(cx: &mut TestAppContext) {
    let (h, _f) = landing(cx, vec![two()]);
    h.factory.set_setup_script(vec![FakeSetupStep::Error(
        EngineError::unreachable_with_hint("download failed", "Check your connection"),
    )]);
    let d = open_dialog(&h, cx);
    pick(&h, cx, &d, 0);
    run(&h, cx, &d);
    h.wait_until(cx, "failed", |_, cx| {
        matches!(d.read(cx).stage(), Stage::Failed { .. })
    });
    match stage(&d, cx) {
        Stage::Failed {
            message,
            hint,
            info,
        } => {
            assert!(message.contains("download failed"));
            assert_eq!(hint.as_deref(), Some("Check your connection"));
            assert!(!info);
        }
        other => panic!("{other:?}"),
    }
    assert!(rendered(&h, cx, "setup-hint-text"));
    // Enter on the focused *Try again* returns to the choice screen.
    h.press(cx, "enter");
    assert_eq!(stage(&d, cx), Stage::Choose);
    assert_eq!(selected(&d, cx), None);
    h.shutdown();
}

#[gpui_kit::test]
fn eng_154_conflict_is_an_info_message(cx: &mut TestAppContext) {
    let (h, _f) = landing(cx, vec![two()]);
    h.factory.set_setup_script(vec![
        phase(SetupPhase::Checking, None, ""),
        FakeSetupStep::Pending,
    ]);
    let d = open_dialog(&h, cx);
    pick(&h, cx, &d, 0);
    // A setup is already running in the hub (a second window, a leftover).
    let _busy = h.hub.run_setup(OFFER, "alpha");
    run(&h, cx, &d);
    h.wait_until(cx, "info", |_, cx| {
        matches!(d.read(cx).stage(), Stage::Failed { info: true, .. })
    });
    h.shutdown();
}

#[gpui_kit::test]
fn eng_154_done_closes_activates_and_notifies(cx: &mut TestAppContext) {
    let (h, _f) = landing(cx, vec![two()]);
    let fresh = h.factory.add(FakeEngine::new("fresh"));
    h.factory.set_setup_script(vec![
        phase(SetupPhase::Starting, None, ""),
        FakeSetupStep::Progress(SetupProgress::Done(fresh)),
    ]);
    let d = open_dialog(&h, cx);
    pick(&h, cx, &d, 0);
    run(&h, cx, &d);
    h.wait_until(cx, "dialog closed", |_, cx| {
        h.shell.read(cx).setup_dialog().is_none()
    });
    assert!(!h.has_dialog(cx));
    h.wait_until(cx, "success toast", |_, cx| crate::ui::toast::count(cx) > 0);
    h.wait_until(cx, "engine active", |_, cx| {
        h.shell
            .read(cx)
            .engines()
            .read(cx)
            .active()
            .is_some_and(|e| e.id().as_str() == "fresh")
    });
    h.shutdown();
}

#[gpui_kit::test]
fn eng_159_palette_entries_and_focus(cx: &mut TestAppContext) {
    // Entries: no default chord, hidden without an offer, *Start engine* unchanged.
    let entry = commands::COMMANDS
        .iter()
        .find(|c| c.label == s::CMD_SET_UP_ENGINE)
        .expect("palette entry");
    assert_eq!((entry.action)().name(), "dk::SetUpEngine");
    for os in [Os::WINDOWS, Os::MACOS, Os::LINUX] {
        assert!(
            !DEFAULT_KEYMAP
                .iter()
                .filter(|b| b.os.contains(os))
                .any(|b| (b.action)().name() == "dk::SetUpEngine"),
            "no default chord"
        );
    }
    assert!(!entry.available(&CommandContext::default()));
    assert!(entry.available(&CommandContext {
        has_setup_offer: true,
        ..Default::default()
    }));
    assert!(
        commands::COMMANDS
            .iter()
            .any(|c| c.label == s::CMD_START_ENGINE)
    );

    // Through the palette: the dialog opens, and Esc gives focus back to the page.
    let (h, _f) = landing(cx, vec![two()]);
    let focus = h.read(cx, |s, _, _| s.start_focus().clone());
    h.wait_until(cx, "page focused", |window, _| focus.is_focused(window));
    h.wait_until(cx, "offers loaded", |_, cx| {
        crate::state::SetupStore::global(cx).is_some_and(|st| !st.read(cx).offers().is_empty())
    });
    h.press(cx, "secondary-shift-p");
    h.type_text(cx, "Set up Docker");
    h.draw(cx);
    h.press(cx, "enter");
    h.wait_until(cx, "dialog open", |_, cx| {
        h.shell.read(cx).setup_dialog().is_some()
    });
    h.press(cx, "escape");
    assert!(!h.has_dialog(cx));
    h.wait_until(cx, "focus back on the page", |window, _| {
        focus.is_focused(window)
    });
    h.shutdown();
}

#[gpui_kit::test]
fn kbd_075_settings_engines_lists_setup_offers(cx: &mut TestAppContext) {
    let (h, _f) = landing(cx, vec![two()]);
    h.update(cx, |_, window, cx| {
        window.dispatch_action(
            Box::new(crate::actions::Navigate {
                route: crate::nav::Route::Settings {
                    section: crate::nav::SettingsSection::Engines,
                },
            }),
            cx,
        )
    });
    h.wait_until(cx, "settings", |_, cx| {
        matches!(
            h.shell.read(cx).page(),
            crate::shell::ShellPage::Settings(_)
        )
    });
    wait_rendered(&h, cx, "settings-setup-button");
    h.shutdown();
}

#[test]
fn eng_161_strings_have_no_mechanism_words() {
    let mut all: Vec<String> = [
        s::START_SECTION,
        s::SETUP_SECTION,
        s::CMD_SET_UP_ENGINE,
        s::SETUP_CHOOSE_TITLE,
        s::SETUP_CHOOSE_HINT,
        s::SETUP_BUTTON,
        s::SETUP_TRY_AGAIN,
        s::SETUP_CLOSE,
        s::SETUP_PROGRESS,
        s::SETUP_FAILED_TITLE,
        s::SETUP_ALREADY_RUNNING,
        s::SETUP_ENDED,
    ]
    .iter()
    .map(|s| (*s).to_owned())
    .collect();
    for p in [
        SetupPhase::Checking,
        SetupPhase::Downloading,
        SetupPhase::Verifying,
        SetupPhase::Importing,
        SetupPhase::Starting,
    ] {
        all.push(s::setup_phase(p, Some(0.5)));
    }
    all.push(s::setup_done("Engine"));
    all.push(s::setup_download_size("1 MB"));
    all.push(s::setup_disk_size("1 MB"));
    let banned = ["com", "cli", "rootfs", "dockerd", "exe", "wsl.exe"];
    for text in &all {
        let lower = text.to_lowercase();
        assert!(
            !lower.contains("docker desktop start") && !lower.contains("wsl.exe"),
            "{text}"
        );
        for token in lower.split(|c: char| !c.is_alphanumeric()) {
            assert!(
                !banned.contains(&token),
                "mechanism word {token:?} in {text:?}"
            );
        }
    }
    // The new UI code carries no such literal either.
    for (name, src) in [
        ("setup dialog", include_str!("ui/setup_dialog.rs")),
        ("engine views", include_str!("shell/engine_views.rs")),
    ] {
        for line in src.lines().filter(|l| !l.trim_start().starts_with("//")) {
            let lower = line.to_lowercase();
            for word in ["wsl.exe", "rootfs", "dockerd", "docker desktop start"] {
                assert!(!lower.contains(word), "{name}: {line}");
            }
        }
    }
}

#[test]
fn eng_162_ui_hardcodes_no_image_names() {
    use std::fs; // nfr-001-allow: test only
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut stack = vec![dir];
    let mut checked = 0;
    while let Some(d) = stack.pop() {
        for e in fs::read_dir(&d).expect("src dir") {
            let path = e.expect("entry").path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            if !name.ends_with(".rs")
                || name.starts_with("view_tests")
                || name == "testing.rs"
                || name == "demo.rs"
            {
                continue;
            }
            let src = fs::read_to_string(&path).expect("source"); // nfr-001-allow: test only
            // Unit-test modules may use sample names.
            let code = src.split("#[cfg(test)]").next().unwrap_or("");
            let lower = code.to_lowercase();
            for word in ["ubuntu", "alpine"] {
                assert!(
                    !lower.contains(word),
                    "{} hard-codes an image name ({word}); labels come from the offer",
                    path.display()
                );
            }
            checked += 1;
        }
    }
    assert!(checked > 20, "walked the sources ({checked})");
}
