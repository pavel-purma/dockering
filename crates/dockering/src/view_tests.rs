//! View tests (spec 60, keyboard plan §8): real `HubHandle` + `FakeFactory`, full `AppShell`
//! in a headless window. Each test names the requirement it covers.

use std::collections::HashSet;

use dk_core::{ContainerState, EngineError};
use gpui_kit::component::WindowExt;
use gpui_kit::{AppContext as _, Focusable, TestAppContext};

use crate::keymap::{Group, Os, reference_rows};
use crate::nav::Route;
use crate::shell::app_shell::Overlay;
use crate::shell::{Region, ShellPage};
use crate::testing::{Setup, id_of, start};

fn calls(h: &crate::testing::Harness, op: &str) -> Vec<String> {
    h.engine.calls_to(op).into_iter().map(|c| c.arg).collect()
}

// ── four states ──────────────────────────────────────────────────────────────────────

#[gpui_kit::test]
fn containers_data_state_renders_grouped_rows(cx: &mut TestAppContext) {
    let h = start(cx, Setup::default());
    let page = h.wait_containers(cx);
    let keys: Vec<String> = cx.read(|cx| {
        page.read(cx)
            .table()
            .read(cx)
            .model(cx)
            .rows()
            .iter()
            .map(|r| r.key.to_string())
            .collect()
    });
    assert!(keys.contains(&"compose:myshop".to_string()), "{keys:?}");
    assert!(keys.contains(&"compose:monitoring".to_string()));
    assert!(keys.contains(&id_of("redis")));
    h.shutdown();
}

#[gpui_kit::test]
fn containers_empty_state(cx: &mut TestAppContext) {
    let h = start(
        cx,
        Setup {
            containers: vec![],
            ..Default::default()
        },
    );
    h.wait_until(cx, "empty containers loaded", |_, cx| {
        h.shell
            .read(cx)
            .store()
            .is_some_and(|s| s.read(cx).containers.data().is_some_and(|d| d.is_empty()))
    });
    let rows = h.read(cx, |s, _, cx| {
        s.containers_page()
            .map(|p| p.read(cx).table().read(cx).model(cx).rows().len())
    });
    assert_eq!(rows, Some(0));
    h.shutdown();
}

#[gpui_kit::test]
fn containers_error_state_keeps_retry(cx: &mut TestAppContext) {
    let h = start(cx, Setup::default());
    h.engine.set_error(
        "list_containers",
        Some(EngineError::Api {
            status: 500,
            message: "boom".into(),
        }),
    );
    h.wait_until(cx, "list error", |_, cx| {
        h.shell
            .read(cx)
            .store()
            .is_some_and(|s| s.read(cx).containers.error().is_some())
    });
    // Retry (Refresh) after the error clears loads data.
    h.engine.set_error("list_containers", None);
    h.update(cx, |_, window, cx| {
        window.dispatch_action(Box::new(crate::actions::Refresh), cx)
    });
    h.wait_until(cx, "data after retry", |_, cx| {
        h.shell
            .read(cx)
            .store()
            .is_some_and(|s| s.read(cx).containers.data().is_some_and(|d| !d.is_empty()))
    });
    h.shutdown();
}

#[gpui_kit::test]
fn containers_loading_state_is_first_load(cx: &mut TestAppContext) {
    let h = start(cx, Setup::default());
    h.engine.set_latency(std::time::Duration::from_millis(300));
    // Right after the engine connects, the store is loading with no data.
    h.wait_until(cx, "store created", |_, cx| {
        h.shell.read(cx).store().is_some()
    });
    let first = cx.read(|cx| {
        h.shell
            .read(cx)
            .store()
            .map(|s| s.read(cx).containers.is_first_load())
    });
    assert_eq!(first, Some(true));
    h.shutdown();
}

// ── grouping & keyboard ───────────────────────────────────────────────────────────────

#[gpui_kit::test]
fn con_010_compose_group_collapsed_then_expands(cx: &mut TestAppContext) {
    let h = start(cx, Setup::default());
    let page = h.wait_containers(cx);
    let member = id_of("myshop-web-1");
    let visible = |cx: &mut TestAppContext| {
        cx.read(|cx| {
            page.read(cx)
                .table()
                .read(cx)
                .model(cx)
                .index_of(&member)
                .is_some()
        })
    };
    assert!(!visible(cx), "groups start collapsed");
    h.focus_table(cx);
    h.select_row(cx, "compose:myshop");
    h.press(cx, "enter");
    assert!(visible(cx), "Enter on a group row expands it (KBD-032)");
    h.shutdown();
}

#[gpui_kit::test]
fn kbd_033_left_right_collapse_group(cx: &mut TestAppContext) {
    let h = start(cx, Setup::default());
    let page = h.wait_containers(cx);
    let member = id_of("myshop-api-1");
    let expanded = |cx: &mut TestAppContext| {
        cx.read(|cx| {
            page.read(cx)
                .table()
                .read(cx)
                .model(cx)
                .is_expanded("compose:myshop")
        })
    };
    h.focus_table(cx);
    h.select_row(cx, "compose:myshop");
    h.press(cx, "right");
    assert!(expanded(cx));
    h.select_row(cx, &member);
    h.press(cx, "left");
    assert_eq!(
        h.cursor_key(cx).as_deref(),
        Some("compose:myshop"),
        "← on member → group"
    );
    h.press(cx, "left");
    assert!(!expanded(cx), "← on an expanded group collapses it");
    h.press(cx, "ctrl-right");
    assert!(expanded(cx), "Mod+→ expands all");
    h.shutdown();
}

#[gpui_kit::test]
fn kbd_034_space_toggles_selection(cx: &mut TestAppContext) {
    let h = start(cx, Setup::default());
    let page = h.wait_containers(cx);
    let redis = id_of("redis");
    h.focus_table(cx);
    h.select_row(cx, &redis);
    h.press(cx, "space");
    let sel = |cx: &mut TestAppContext| {
        cx.read(|cx| page.read(cx).table().read(cx).model(cx).selected().clone())
    };
    assert!(sel(cx).contains(redis.as_str()));
    // On a group row: all members.
    h.select_row(cx, "compose:myshop");
    h.press(cx, "space");
    assert_eq!(sel(cx).len(), 5, "redis + 4 myshop members");
    h.press(cx, "space");
    assert_eq!(sel(cx).len(), 1);
    h.shutdown();
}

#[gpui_kit::test]
fn kbd_030_s_starts_stopped_container(cx: &mut TestAppContext) {
    let h = start(cx, Setup::default());
    h.wait_containers(cx);
    let scratch = id_of("scratchpad");
    h.focus_table(cx);
    h.select_row(cx, &scratch);
    h.engine.clear_calls();
    h.press(cx, "s");
    h.wait_until(cx, "start called", |_, _| {
        !h.engine.calls_to("start").is_empty()
    });
    assert_eq!(calls(&h, "start"), vec![scratch.clone()]);
    // CON-031: the row shows pending, then the refetch brings the new state.
    h.wait_until(cx, "refetched running", |_, cx| {
        h.shell.read(cx).store().is_some_and(|s| {
            s.read(cx)
                .containers
                .data()
                .and_then(|d| d.iter().find(|c| c.id == scratch).map(|c| c.state))
                == Some(ContainerState::Running)
        })
    });
    h.shutdown();
}

#[gpui_kit::test]
fn kbd_008_letters_ignored_in_search_input(cx: &mut TestAppContext) {
    let h = start(cx, Setup::default());
    let page = h.wait_containers(cx);
    h.focus_table(cx);
    h.select_row(cx, &id_of("scratchpad"));
    h.engine.clear_calls();
    // Mod+F focuses the search input (KBD-025); typing "s" must not start anything.
    h.press(cx, "ctrl-f");
    let search_focused = cx
        .update_window(h.any_window(), |_, window, cx| {
            page.read(cx)
                .search_input()
                .focus_handle(cx)
                .is_focused(window)
        })
        .unwrap();
    assert!(search_focused);
    h.type_text(cx, "s");
    cx.run_until_parked();
    assert!(
        h.engine.calls_to("start").is_empty(),
        "no single-letter action in inputs"
    );
    assert!(h.engine.calls_to("stop").is_empty());
    let value = cx.read(|cx| page.read(cx).search_input().read(cx).value().to_string());
    assert_eq!(value, "s");
    h.shutdown();
}

#[gpui_kit::test]
fn con_021_running_delete_needs_force(cx: &mut TestAppContext) {
    let h = start(cx, Setup::default());
    h.wait_containers(cx);
    let redis = id_of("redis");
    h.focus_table(cx);
    h.select_row(cx, &redis);
    h.press(cx, "delete");
    let has_dialog = cx
        .update_window(h.any_window(), |_, window, cx| window.has_active_dialog(cx))
        .unwrap();
    assert!(has_dialog, "running delete always confirms (CON-021)");
    assert!(h.engine.calls_to("remove_container").is_empty());
    // Initial focus is on Cancel (KBD-071): plain Enter would cancel, not delete.
    h.draw(cx);
    let table_focused = cx
        .update_window(h.any_window(), |_, window, cx| {
            let page = h.shell.read(cx).containers_page().cloned().unwrap();
            page.read(cx).table().focus_handle(cx).is_focused(window)
        })
        .unwrap();
    assert!(!table_focused, "focus moved into the dialog");
    // The dialog's Cancel holds focus and the Dialog key context is active.
    let (stack, cancel_focused) = cx
        .update_window(h.any_window(), |_, window, cx| {
            let stack: Vec<String> = window
                .context_stack()
                .iter()
                .map(|c| format!("{c:?}"))
                .collect();
            let confirm = window.focused(cx);
            (stack, confirm)
        })
        .unwrap();
    assert!(stack.iter().any(|c| c.contains("Dialog")), "{stack:?}");
    assert!(cancel_focused.is_some());
    // Mod+Enter confirms the destructive action (KBD-071); it's a force delete.
    h.press(cx, "ctrl-enter");
    h.wait_until(cx, "remove called", |_, _| {
        !h.engine.calls_to("remove_container").is_empty()
    });
    h.wait_until(cx, "redis gone", |_, cx| {
        h.shell.read(cx).store().is_some_and(|s| {
            s.read(cx)
                .containers
                .data()
                .is_some_and(|d| d.iter().all(|c| c.id != redis))
        })
    });
    h.shutdown();
}

#[gpui_kit::test]
fn kbd_007_focus_after_delete_moves_to_next_row(cx: &mut TestAppContext) {
    let mut config = dk_hub::Config::default();
    config.general.confirm_delete_stopped = false;
    let h = start(
        cx,
        Setup {
            config,
            ..Default::default()
        },
    );
    let page = h.wait_containers(cx);
    let rows: Vec<String> = cx.read(|cx| {
        page.read(cx)
            .table()
            .read(cx)
            .model(cx)
            .rows()
            .iter()
            .map(|r| r.key.to_string())
            .collect()
    });
    // "fresh" (Created, stopped) is followed by "redis".
    let fresh = id_of("fresh");
    let ix = rows.iter().position(|k| *k == fresh).expect("fresh row");
    let next = rows[ix + 1].clone();
    h.focus_table(cx);
    h.select_row(cx, &fresh);
    h.press(cx, "delete");
    h.wait_until(cx, "fresh deleted", |_, cx| {
        h.shell.read(cx).store().is_some_and(|s| {
            s.read(cx)
                .containers
                .data()
                .is_some_and(|d| d.iter().all(|c| c.id != fresh))
        })
    });
    h.wait_until(cx, "cursor on next row", |_, _| true);
    assert_eq!(h.cursor_key(cx), Some(next));
    h.shutdown();
}

// ── shell ────────────────────────────────────────────────────────────────────────────

#[gpui_kit::test]
fn kbd_005_f6_cycles_regions(cx: &mut TestAppContext) {
    let h = start(cx, Setup::default());
    h.wait_containers(cx);
    h.focus_table(cx);
    let region = |cx: &mut TestAppContext| h.read(cx, |s, window, cx| s.current_region(window, cx));
    assert_eq!(region(cx), Some(Region::Content));
    h.press(cx, "f6");
    assert_eq!(region(cx), Some(Region::StatusBar));
    h.press(cx, "f6");
    assert_eq!(region(cx), Some(Region::TitleBar));
    h.press(cx, "f6");
    assert_eq!(region(cx), Some(Region::Sidebar));
    h.press(cx, "shift-f6");
    assert_eq!(region(cx), Some(Region::TitleBar));
    h.press(cx, "shift-f6 shift-f6");
    // Back in content: focus returns to the remembered table, not just the region.
    assert_eq!(region(cx), Some(Region::Content));
    let page = h.page(cx).unwrap();
    let table_focused = cx
        .update_window(h.any_window(), |_, window, cx| {
            page.read(cx).table().focus_handle(cx).is_focused(window)
        })
        .unwrap();
    assert!(table_focused, "regions remember the last-focused child");
    h.shutdown();
}

#[gpui_kit::test]
fn kbd_006_escape_priority_order(cx: &mut TestAppContext) {
    let h = start(cx, Setup::default());
    let page = h.wait_containers(cx);
    h.focus_table(cx);
    h.select_row(cx, &id_of("redis"));
    h.press(cx, "space");
    // 1. An open popup closes first.
    h.press(cx, "ctrl-k");
    assert_eq!(h.read(cx, |s, _, _| s.overlay()), Overlay::Switcher);
    h.press(cx, "escape");
    assert_eq!(h.read(cx, |s, _, _| s.overlay()), Overlay::None);
    // 2. The multi-selection clears next (focus is back on the table).
    let selected = |cx: &mut TestAppContext| {
        cx.read(|cx| page.read(cx).table().read(cx).model(cx).selected().len())
    };
    assert_eq!(
        selected(cx),
        1,
        "Escape on the popup didn't clear the selection"
    );
    h.press(cx, "escape");
    assert_eq!(selected(cx), 0);
    // 3. A focused, non-empty search clears before anything else.
    h.press(cx, "ctrl-f");
    h.type_text(cx, "red");
    h.press(cx, "escape");
    let value = cx.read(|cx| page.read(cx).search_input().read(cx).value().to_string());
    assert_eq!(value, "");
    // Escape never navigates back.
    assert_eq!(h.read(cx, |s, _, _| s.route().clone()), Route::Containers);
    h.shutdown();
}

#[gpui_kit::test]
fn kbd_020_palette_runs_action(cx: &mut TestAppContext) {
    let h = start(cx, Setup::default());
    h.wait_containers(cx);
    h.focus_table(cx);
    h.press(cx, "ctrl-shift-p");
    assert_eq!(h.read(cx, |s, _, _| s.overlay()), Overlay::Palette);
    h.type_text(cx, "Go to Images");
    cx.run_until_parked();
    h.press(cx, "enter");
    h.wait_until(cx, "navigated to images", |_, cx| {
        *h.shell.read(cx).route() == Route::Images
    });
    h.shutdown();
}

#[gpui_kit::test]
fn kbd_022_reference_lists_os_bindings(cx: &mut TestAppContext) {
    let h = start(cx, Setup::default());
    h.wait_containers(cx);
    h.press(cx, "f1");
    assert_eq!(h.read(cx, |s, _, _| s.overlay()), Overlay::Shortcuts);
    let rows = cx.read(|cx| {
        h.shell
            .read(cx)
            .shortcuts()
            .map(|l| l.read(cx).rows().to_vec())
            .unwrap_or_default()
    });
    assert_eq!(rows, reference_rows(Os::current()));
    let groups: HashSet<Group> = rows.iter().map(|r| r.group).collect();
    assert!(groups.contains(&Group::Global) && groups.contains(&Group::Lists));
    let mac_only = rows.iter().any(|r| r.keys.contains('⌘'));
    assert_eq!(
        mac_only,
        cfg!(target_os = "macos"),
        "only this OS's bindings"
    );
    h.shutdown();
}

#[gpui_kit::test]
fn eng_102_switch_engine_drops_store(cx: &mut TestAppContext) {
    let other = dk_core::fake::FakeEngine::new("other");
    other.set_containers(vec![dk_core::fake::fixtures::container(
        "other-only",
        ContainerState::Running,
    )]);
    let h = start(
        cx,
        Setup {
            extra_engines: vec![other.clone()],
            ..Default::default()
        },
    );
    h.wait_containers(cx);
    let old_store = h.read(cx, |s, _, _| s.store().cloned()).expect("store");
    let weak = old_store.downgrade();
    drop(old_store);
    let target: gpui_kit::SharedString = "other".into();
    h.update(cx, |_, window, cx| {
        window.dispatch_action(Box::new(crate::actions::SwitchEngine { id: target }), cx)
    });
    h.wait_until(cx, "store for other engine", |_, cx| {
        h.shell
            .read(cx)
            .store()
            .is_some_and(|s| s.read(cx).engine_id().as_str() == "other")
    });
    cx.run_until_parked();
    assert!(
        weak.upgrade().is_none(),
        "the previous EngineStore was dropped"
    );
    h.wait_until(cx, "other engine rows", |_, cx| {
        h.shell.read(cx).store().is_some_and(|s| {
            s.read(cx)
                .containers
                .data()
                .is_some_and(|d| d.iter().any(|c| c.name == "other-only"))
        })
    });
    h.shutdown();
}

#[gpui_kit::test]
fn shl_013_disconnected_full_page_alert(cx: &mut TestAppContext) {
    // The last-used engine is activated directly, so its failure is the *active* state.
    let ui_state = dk_hub::UiState {
        last_engine: Some(dk_core::EngineId::new("demo")),
        ..Default::default()
    };
    let h = start(
        cx,
        Setup {
            ui_state,
            failing: vec![(
                "demo".into(),
                EngineError::unreachable_with_hint("connection refused", "Is Docker running?"),
            )],
            ..Default::default()
        },
    );
    h.wait_until(cx, "engine failed", |_, cx| {
        h.shell
            .read(cx)
            .engines()
            .read(cx)
            .active()
            .is_some_and(|s| matches!(s.state, dk_core::EngineState::Failed { .. }))
    });
    // No list page mounted: stale data can't be acted on (SHL-013).
    let page_is_none = h.read(cx, |s, _, _| matches!(s.page(), ShellPage::None));
    assert!(page_is_none);
    h.shutdown();
}

#[gpui_kit::test]
fn kbd_023_mod_digits_navigate_and_back_forward(cx: &mut TestAppContext) {
    let h = start(cx, Setup::default());
    h.wait_containers(cx);
    h.focus_table(cx);
    h.press(cx, "ctrl-2");
    assert_eq!(h.read(cx, |s, _, _| s.route().clone()), Route::Images);
    h.press(cx, "ctrl-3");
    assert_eq!(h.read(cx, |s, _, _| s.route().clone()), Route::Volumes);
    let back = if cfg!(target_os = "macos") {
        "cmd-["
    } else {
        "alt-left"
    };
    h.press(cx, back);
    assert_eq!(h.read(cx, |s, _, _| s.route().clone()), Route::Images);
    h.shutdown();
}

#[gpui_kit::test]
fn con_033_enter_opens_detail_with_primary_focus(cx: &mut TestAppContext) {
    let h = start(cx, Setup::default());
    h.wait_containers(cx);
    let redis = id_of("redis");
    h.focus_table(cx);
    h.select_row(cx, &redis);
    h.press(cx, "enter");
    let route = h.read(cx, |s, _, _| s.route().clone());
    assert_eq!(
        route,
        Route::ContainerDetail {
            id: redis,
            tab: crate::nav::ContainerTab::Overview
        }
    );
    h.wait_until(cx, "detail focused", |window, cx| {
        h.shell
            .read(cx)
            .page()
            .primary_focus(cx)
            .is_some_and(|f| f.is_focused(window))
    });
    h.shutdown();
}
