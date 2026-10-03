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
fn con_010_compose_group_expanded_then_collapses(cx: &mut TestAppContext) {
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
    assert!(
        visible(cx),
        "groups start expanded: every container is visible"
    );
    h.focus_table(cx);
    h.select_row(cx, "compose:myshop");
    h.press(cx, "enter");
    assert!(!visible(cx), "Enter on a group row toggles it (KBD-032)");
    h.press(cx, "enter");
    assert!(visible(cx));
    h.shutdown();
}

/// A click anywhere on a row dispatches `OnRow { Open }` (CON-033): a group row toggles,
/// an item row opens its detail.
#[gpui_kit::test]
fn con_033_row_click_toggles_group_and_opens_item(cx: &mut TestAppContext) {
    use crate::actions::{OnRow, RowCommand};
    let h = start(cx, Setup::default());
    let page = h.wait_containers(cx);
    let expanded = |cx: &mut TestAppContext| {
        cx.read(|cx| {
            page.read(cx)
                .table()
                .read(cx)
                .model(cx)
                .is_expanded("compose:myshop")
        })
    };
    let click = |cx: &mut TestAppContext, row: &str| {
        let table = cx.read(|cx| page.read(cx).table().clone());
        let row: gpui_kit::SharedString = row.to_owned().into();
        cx.update_window(h.any_window(), |_, window, cx| {
            let f = gpui_kit::Focusable::focus_handle(table.read(cx), cx);
            f.dispatch_action(
                &OnRow {
                    row,
                    action: RowCommand::Open,
                },
                window,
                cx,
            );
        })
        .expect("window");
        cx.run_until_parked();
    };
    h.draw(cx);
    assert!(expanded(cx));
    click(cx, "compose:myshop");
    assert!(!expanded(cx), "click on a group row collapses it");
    h.draw(cx);
    click(cx, "compose:myshop");
    assert!(expanded(cx), "and expands it again");
    let redis = id_of("redis");
    h.draw(cx);
    click(cx, &redis);
    assert_eq!(
        h.read(cx, |s, _, _| s.route().clone()),
        Route::ContainerDetail {
            id: redis,
            tab: crate::nav::ContainerTab::Overview
        }
    );
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
    h.press(cx, "secondary-right");
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

/// SHL-005: the header's summary (`8 running · 4 stopped`) gives way to the selection
/// actions as soon as one row is checked, and comes back when the selection clears.
#[gpui_kit::test]
fn shl_005_selection_actions_replace_summary(cx: &mut TestAppContext) {
    let h = start(cx, Setup::default());
    h.wait_containers(cx);
    let rendered = |cx: &mut TestAppContext, selector: &'static str| {
        h.draw(cx);
        gpui_kit::VisualTestContext::from_window(h.any_window(), cx)
            .debug_bounds(selector)
            .is_some()
    };
    assert!(rendered(cx, "page-summary"));
    assert!(!rendered(cx, "selection-actions"));
    h.focus_table(cx);
    h.select_row(cx, &id_of("redis"));
    h.press(cx, "space");
    assert!(
        rendered(cx, "selection-actions"),
        "one checked row is enough"
    );
    assert!(!rendered(cx, "page-summary"));
    h.press(cx, "escape");
    assert!(rendered(cx, "page-summary"));
    assert!(!rendered(cx, "selection-actions"));
    h.shutdown();
}

/// SHL-005: a single checked row is what *Stop* in the selection actions acts on, not the
/// cursor row.
#[gpui_kit::test]
fn shl_005_bulk_stop_acts_on_one_checked_row(cx: &mut TestAppContext) {
    let h = start(cx, Setup::default());
    h.wait_containers(cx);
    let redis = id_of("redis");
    h.focus_table(cx);
    h.select_row(cx, &redis);
    h.press(cx, "space");
    h.select_row(cx, &id_of("billing-svc"));
    h.engine.clear_calls();
    h.update(cx, |_, window, cx| {
        window.dispatch_action(Box::new(crate::actions::list::BulkStop), cx)
    });
    h.wait_until(cx, "stop called", |_, _| {
        !h.engine.calls_to("stop").is_empty()
    });
    let stops = h.engine.calls_to("stop");
    assert_eq!(stops.len(), 1);
    assert_eq!(stops[0].arg, redis);
    h.shutdown();
}

/// SHL-005: a mouse click on a row checkbox checks it (without focusing the table first);
/// the select-all checkbox in the table header checks every visible row, and a second click
/// clears the selection.
#[gpui_kit::test]
fn shl_005_header_checkbox_selects_all(cx: &mut TestAppContext) {
    let h = start(cx, Setup::default());
    let page = h.wait_containers(cx);
    let selected = |cx: &mut TestAppContext| {
        cx.read(|cx| page.read(cx).table().read(cx).model(cx).selected().len())
    };
    h.draw(cx);
    let mut visual = gpui_kit::VisualTestContext::from_window(h.any_window(), cx);
    let row = visual
        .debug_bounds("row-check-1")
        .expect("row checkbox rendered");
    visual.simulate_click(row.center(), Default::default());
    cx.run_until_parked();
    assert_eq!(selected(cx), 1, "row checkbox click");
    visual.simulate_click(row.center(), Default::default());
    cx.run_until_parked();
    assert_eq!(selected(cx), 0, "second click unchecks");
    h.draw(cx);
    let mut visual = gpui_kit::VisualTestContext::from_window(h.any_window(), cx);
    let header = visual
        .debug_bounds("select-all")
        .expect("select-all checkbox rendered");
    visual.simulate_click(header.center(), Default::default());
    cx.run_until_parked();
    assert_eq!(selected(cx), 12, "every container");
    h.draw(cx);
    let mut visual = gpui_kit::VisualTestContext::from_window(h.any_window(), cx);
    visual.simulate_click(header.center(), Default::default());
    cx.run_until_parked();
    assert_eq!(selected(cx), 0);
    h.shutdown();
}

/// A click on the empty part of the page header (next to the filters) used to move focus
/// to the content region, above the page's handlers, so the filter buttons stopped working.
/// Focus now lands on the page and the filter still switches by mouse.
#[gpui_kit::test]
fn con_004_filter_clicks_work_after_clicking_header_background(cx: &mut TestAppContext) {
    let h = start(cx, Setup::default());
    let page = h.wait_containers(cx);
    h.draw(cx);
    let mut visual = gpui_kit::VisualTestContext::from_window(h.any_window(), cx);
    let header = visual.debug_bounds("page-header").expect("header rendered");
    let running = visual
        .debug_bounds("filter-running")
        .expect("filter rendered");
    // Empty header space: left of the filters, below the title.
    let blank = gpui_kit::point(running.origin.x - gpui_kit::px(40.), running.center().y);
    assert!(header.contains(&blank));
    visual.simulate_click(blank, Default::default());
    cx.run_until_parked();
    h.draw(cx);
    let mut visual = gpui_kit::VisualTestContext::from_window(h.any_window(), cx);
    visual.simulate_click(running.center(), Default::default());
    cx.run_until_parked();
    assert_eq!(
        cx.read(|cx| page.read(cx).filter()),
        crate::pages::containers::model::StatusFilter::Running
    );
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
    h.press(cx, "secondary-f");
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
    h.press(cx, "secondary-enter");
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
    h.press(cx, "secondary-k");
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
    h.press(cx, "secondary-f");
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
    h.press(cx, "secondary-shift-p");
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
    h.press(cx, "secondary-2");
    assert_eq!(h.read(cx, |s, _, _| s.route().clone()), Route::Images);
    h.press(cx, "secondary-3");
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

#[gpui_kit::test]
fn kbd_021_switcher_filter_arrows_enter(cx: &mut TestAppContext) {
    let other = dk_core::fake::FakeEngine::new("other");
    let h = start(
        cx,
        Setup {
            extra_engines: vec![other],
            ..Default::default()
        },
    );
    h.wait_containers(cx);
    h.wait_until(cx, "two engines listed", |_, cx| {
        h.shell.read(cx).engines().read(cx).engines().len() == 2
    });
    h.press(cx, "secondary-k");
    assert_eq!(h.read(cx, |s, _, _| s.overlay()), Overlay::Switcher);
    // Typing filters; Enter switches to the (only) match.
    h.type_text(cx, "other");
    h.press(cx, "enter");
    h.wait_until(cx, "switched to other", |_, cx| {
        h.shell
            .read(cx)
            .store()
            .is_some_and(|s| s.read(cx).engine_id().as_str() == "other")
    });
    assert_eq!(h.read(cx, |s, _, _| s.overlay()), Overlay::None);
    h.shutdown();
}

#[gpui_kit::test]
fn kbd_037_quick_find_jumps(cx: &mut TestAppContext) {
    let h = start(cx, Setup::default());
    h.wait_containers(cx);
    h.focus_table(cx);
    h.press(cx, "/");
    h.type_text(cx, "redis");
    cx.run_until_parked();
    assert_eq!(h.cursor_key(cx), Some(id_of("redis")));
    // Esc closes the find field and returns focus to the table.
    h.press(cx, "escape");
    let page = h.page(cx).unwrap();
    let (open, focused) = cx
        .update_window(h.any_window(), |_, window, cx| {
            let t = page.read(cx).table().clone();
            let t = t.read(cx);
            (t.find_open(), t.focus_handle(cx).is_focused(window))
        })
        .unwrap();
    assert!(!open && focused);
    h.shutdown();
}

#[gpui_kit::test]
fn kbd_036_shift_f10_opens_row_menu(cx: &mut TestAppContext) {
    let h = start(cx, Setup::default());
    let page = h.wait_containers(cx);
    h.focus_table(cx);
    h.select_row(cx, &id_of("scratchpad"));
    h.press(cx, "shift-f10");
    let open = cx.read(|cx| page.read(cx).table().read(cx).key_menu_open());
    assert!(open, "Shift+F10 opens the row menu");
    // Enter on the first item (Start) runs it on the cursor row.
    h.engine.clear_calls();
    h.press(cx, "down enter");
    h.wait_until(cx, "start from menu", |_, _| {
        !h.engine.calls_to("start").is_empty()
    });
    h.shutdown();
}

#[gpui_kit::test]
fn kbd_039_group_by_none_flattens(cx: &mut TestAppContext) {
    let h = start(cx, Setup::default());
    let page = h.wait_containers(cx);
    h.focus_table(cx);
    h.press(cx, "secondary-shift-g");
    // Pick "None" from the menu (second item).
    h.press(cx, "down down enter");
    h.wait_until(cx, "flat list", |_, cx| {
        page.read(cx)
            .table()
            .read(cx)
            .model(cx)
            .rows()
            .iter()
            .all(|r| !r.is_group())
    });
    assert_eq!(
        cx.read(|cx| page.read(cx).group_by().clone()),
        dk_core::grouping::GroupBy::None
    );
    h.shutdown();
}

#[gpui_kit::test]
fn con_013_stop_all_stops_running_members(cx: &mut TestAppContext) {
    let h = start(cx, Setup::default());
    h.wait_containers(cx);
    h.focus_table(cx);
    h.select_row(cx, "compose:myshop");
    h.engine.clear_calls();
    // S on a group row with running members = stop all (reverse dependency order).
    h.press(cx, "s");
    h.wait_until(cx, "3 stops", |_, _| h.engine.calls_to("stop").len() == 3);
    let stopped: HashSet<String> = calls(&h, "stop").into_iter().collect();
    let expect: HashSet<String> = ["myshop-web-1", "myshop-api-1", "myshop-db-1"]
        .iter()
        .map(|n| id_of(n))
        .collect();
    assert_eq!(stopped, expect);
    // Reverse of start order: web (depends on api) stops before db.
    let order = calls(&h, "stop");
    let pos = |n: &str| order.iter().position(|i| *i == id_of(n)).unwrap();
    assert!(pos("myshop-web-1") < pos("myshop-db-1"));
    h.wait_until(cx, "aggregate 0/4", |_, cx| {
        h.shell.read(cx).store().is_some_and(|s| {
            s.read(cx).containers.data().is_some_and(|d| {
                d.iter()
                    .filter(|c| c.name.starts_with("myshop-"))
                    .all(|c| !c.state.is_running())
            })
        })
    });
    h.shutdown();
}

#[gpui_kit::test]
fn con_013_delete_all_lists_members_and_needs_force(cx: &mut TestAppContext) {
    let h = start(cx, Setup::default());
    h.wait_containers(cx);
    h.focus_table(cx);
    h.select_row(cx, "compose:myshop");
    h.press(cx, "delete");
    let open = cx
        .update_window(h.any_window(), |_, window, cx| window.has_active_dialog(cx))
        .unwrap();
    assert!(open);
    // Without Force only the stopped member (the one-off migrate) is removed.
    h.engine.clear_calls();
    h.press(cx, "secondary-enter");
    h.wait_until(cx, "remove called once", |_, _| {
        h.engine.calls_to("remove_container").len() == 1
    });
    assert_eq!(
        calls(&h, "remove_container"),
        vec![id_of("myshop-migrate-run-1a2b")]
    );
    h.shutdown();
}

#[gpui_kit::test]
fn spec10_polling_fallback_without_events_capability(cx: &mut TestAppContext) {
    // 1 s polling keeps the test well inside the harness's 10 s wait, even under load.
    let mut setup = Setup::default();
    setup.config.containers.polling_interval_s = 1;
    let h = start(cx, setup);
    h.engine
        .set_capabilities(dk_core::Capabilities::all() - dk_core::Capabilities::EVENTS);
    h.update(cx, |_, window, cx| {
        window.dispatch_action(Box::new(crate::actions::Refresh), cx)
    });
    h.wait_until(cx, "polling mode", |_, cx| {
        h.shell.read(cx).store().is_some_and(|s| {
            s.read(cx)
                .info()
                .is_some_and(|i| !i.capabilities.contains(dk_core::Capabilities::EVENTS))
                && s.read(cx).live_mode() == crate::state::LiveMode::Polling
        })
    });
    // A change made behind our back shows up via polling.
    let mut cs = crate::demo::containers();
    cs.retain(|c| c.name != "redis");
    h.engine.set_containers(cs);
    h.wait_until(cx, "polled refresh drops redis", |_, cx| {
        h.shell.read(cx).store().is_some_and(|s| {
            s.read(cx)
                .containers
                .data()
                .is_some_and(|d| d.iter().all(|c| c.name != "redis"))
        })
    });
    h.shutdown();
}

#[gpui_kit::test]
fn con_030_engine_event_triggers_refetch(cx: &mut TestAppContext) {
    let h = start(cx, Setup::default());
    h.wait_containers(cx);
    h.wait_until(cx, "subscribed", |_, _| h.engine.open_streams().0 > 0);
    let mut cs = crate::demo::containers();
    cs.push(dk_core::fake::fixtures::container(
        "newcomer",
        ContainerState::Running,
    ));
    h.engine.set_containers(cs);
    h.engine.emit_event(dk_core::fake::fixtures::event(
        dk_core::ResourceKind::Container,
        "create",
        "x",
    ));
    h.wait_until(cx, "newcomer appears", |_, cx| {
        h.shell.read(cx).store().is_some_and(|s| {
            s.read(cx)
                .containers
                .data()
                .is_some_and(|d| d.iter().any(|c| c.name == "newcomer"))
        })
    });
    h.shutdown();
}

#[gpui_kit::test]
fn kbd_020_palette_closes_after_non_navigation_command(cx: &mut TestAppContext) {
    let h = start(cx, Setup::default());
    let page = h.wait_containers(cx);
    h.focus_table(cx);
    let expanded = |cx: &mut TestAppContext| {
        cx.read(|cx| {
            page.read(cx)
                .table()
                .read(cx)
                .model(cx)
                .is_expanded("compose:myshop")
        })
    };
    assert!(expanded(cx), "groups start expanded");
    h.press(cx, "secondary-shift-p");
    h.type_text(cx, "Collapse all groups");
    h.press(cx, "enter");
    h.wait_until(cx, "palette closed", |_, cx| {
        h.shell.read(cx).overlay() == Overlay::None
    });
    assert!(!expanded(cx), "palette ran the list command on the page");
    let table_focused = cx
        .update_window(h.any_window(), |_, window, cx| {
            page.read(cx).table().focus_handle(cx).is_focused(window)
        })
        .unwrap();
    assert!(table_focused, "focus returned to the invoker (KBD-007)");
    h.shutdown();
}

#[gpui_kit::test]
fn kbd_017_028_theme_toggle_and_sidebar(cx: &mut TestAppContext) {
    use gpui_kit::component::ActiveTheme;
    let h = start(cx, Setup::default());
    h.wait_containers(cx);
    h.focus_table(cx);
    let dark_before = cx.read(|cx| cx.theme().is_dark());
    h.press(cx, "secondary-shift-l");
    let dark_after = cx.read(|cx| cx.theme().is_dark());
    assert_ne!(dark_before, dark_after, "Mod+Shift+L toggles the theme");
    let saved = cx.read(|cx| crate::state::AppState::config(cx).general.theme);
    assert_eq!(saved, crate::theme::toggled(dark_before));
    h.press(cx, "secondary-b");
    assert!(h.read(cx, |s, _, _| s.sidebar_collapsed()));
    h.press(cx, "secondary-b");
    assert!(!h.read(cx, |s, _, _| s.sidebar_collapsed()));
    h.shutdown();
}

#[gpui_kit::test]
fn shl_024_zoom_in_out_reset_persists(cx: &mut TestAppContext) {
    let h = start(cx, Setup::default());
    h.wait_containers(cx);
    h.focus_table(cx);
    let scale =
        |cx: &mut TestAppContext| cx.read(|cx| crate::state::AppState::config(cx).general.ui_scale);
    h.press(cx, "secondary-=");
    assert!((scale(cx) - 1.1).abs() < 1e-4, "{}", scale(cx));
    h.press(cx, "secondary--");
    h.press(cx, "secondary--");
    assert!((scale(cx) - 0.9).abs() < 1e-4);
    h.press(cx, "secondary-0");
    assert!((scale(cx) - 1.0).abs() < 1e-4);
    h.shutdown();
}

#[gpui_kit::test]
fn kbd_026_f5_refetches_everything(cx: &mut TestAppContext) {
    let h = start(cx, Setup::default());
    h.wait_containers(cx);
    h.focus_table(cx);
    h.engine.clear_calls();
    h.press(cx, "f5");
    h.wait_until(cx, "all four lists refetched", |_, _| {
        [
            "list_containers",
            "list_images",
            "list_volumes",
            "list_networks",
        ]
        .iter()
        .all(|op| !h.engine.calls_to(op).is_empty())
    });
    h.shutdown();
}
