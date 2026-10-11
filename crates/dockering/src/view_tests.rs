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

#[gpui_kit::test]
fn eng_136_metadata_without_flags_preserves_focus_page_and_updates_stats(cx: &mut TestAppContext) {
    use crate::state::EngineListEvent;
    use std::cell::RefCell;
    use std::rc::Rc;

    let mut setup = Setup::default();
    setup.config.containers.show_cpu_mem_columns = true;
    let h = start(cx, setup);
    let page = h.wait_containers(cx);
    h.wait_until(cx, "info and list stats", |_, cx| {
        page.read(cx).stats_subscription_count() > 0
            && h.shell.read(cx).store().unwrap().read(cx).info().is_some()
    });
    h.focus_table(cx);
    h.select_row(cx, &id_of("redis"));
    let (store, list, mut status, focus) = h.read(cx, |s, window, cx| {
        (
            s.store().unwrap().clone(),
            s.engines().clone(),
            s.engines().read(cx).active().unwrap().clone(),
            window.focused(cx).unwrap(),
        )
    });
    let caps = status.capabilities();
    let events = Rc::new(RefCell::new(Vec::new()));
    let captured = events.clone();
    let _subscription = cx.update(|cx| {
        cx.subscribe(&list, move |_, event: &EngineListEvent, _| {
            captured.borrow_mut().push(event.clone());
        })
    });
    let initial_subscriptions = cx.read(|cx| page.read(cx).stats_subscription_count());
    status.info.as_mut().unwrap().transport_note = Some("CLI fallback: pull_image".into());
    let note_only = status.info.clone().unwrap();
    cx.update(|cx| {
        list.update(cx, |s, cx| {
            s.apply_test_event(dk_hub::HubEvent::StatusChanged(status.clone()), cx)
        })
    });
    h.wait_until(cx, "note-only snapshot applied", |_, cx| {
        store.read(cx).info() == Some(&note_only)
    });
    assert_eq!(
        cx.read(|cx| page.read(cx).stats_subscription_count()),
        initial_subscriptions
    );
    assert!(h.is_focused(cx, &focus));
    assert!(h.read(cx, |s, _, _| s.store() == Some(&store)
        && s.containers_page() == Some(&page)));
    events.borrow_mut().clear();
    // An older info request returns the fake's original metadata. It must not undo
    // the authoritative status snapshot arriving while that request is in flight.
    h.engine.set_latency(std::time::Duration::from_millis(150));
    h.engine.clear_calls();
    cx.update(|cx| store.update(cx, |s, cx| s.fetch_info(cx)));
    h.wait_until(cx, "older info call started", |_, _| {
        !h.engine.calls_to("info").is_empty()
    });
    let note = "CLI fallback: stats";
    let info = status.info.as_mut().unwrap();
    info.transport = Some("com".into());
    info.transport_note = Some(note.into());
    info.list_stats_limit = 0;
    let expected = info.clone();
    cx.update(|cx| {
        list.update(cx, |s, cx| {
            s.apply_test_event(dk_hub::HubEvent::StatusChanged(status.clone()), cx)
        })
    });
    h.wait_until(cx, "metadata applied and stats cancelled", |_, cx| {
        store.read(cx).info() == Some(&expected)
            && page.read(cx).stats_subscription_count() == 0
            && !page.read(cx).table().read(cx).delegate(cx).show_stats
    });
    // Pump past the delayed call without relying on a UI-thread sleep.
    h.engine.set_latency(std::time::Duration::ZERO);
    let started = std::time::Instant::now();
    h.wait_until(cx, "older info call completed", |_, _| {
        started.elapsed() >= std::time::Duration::from_millis(200)
    });
    assert_eq!(cx.read(|cx| store.read(cx).info().cloned()), Some(expected));
    assert_eq!(cx.read(|cx| store.read(cx).capabilities()), caps);
    assert_eq!(h.cursor_key(cx), Some(id_of("redis")));
    assert!(h.is_focused(cx, &focus));
    assert!(h.read(cx, |s, _, _| s.store() == Some(&store)
        && s.containers_page() == Some(&page)));
    assert_eq!(
        &*events.borrow(),
        &[EngineListEvent::InfoChanged(status.id().clone())]
    );
    assert_eq!(
        h.engine.calls_to("info").len(),
        1,
        "snapshot application does not refetch info"
    );
    assert!(
        h.engine.calls_to("list_containers").is_empty(),
        "metadata is not a reconnect"
    );
    // The same local path carries a nonzero limit: subscription budget changes with
    // identical capability flags, not an EngineKind branch.
    status.info.as_mut().unwrap().list_stats_limit = 1;
    cx.update(|cx| {
        list.update(cx, |s, cx| {
            s.apply_test_event(dk_hub::HubEvent::StatusChanged(status), cx)
        })
    });
    h.wait_until(cx, "one list stats subscription", |_, cx| {
        page.read(cx).stats_subscription_count() == 1
            && page.read(cx).table().read(cx).delegate(cx).show_stats
    });
    assert!(h.is_focused(cx, &focus));
    h.shutdown();
}

/// ENG-108/110: the bar's left segment ends with the transport tag. A route note in the
/// snapshot adds nothing after it, so no chip is drawn there.
#[gpui_kit::test]
fn eng_110_status_bar_draws_no_route_note(cx: &mut TestAppContext) {
    let h = start(cx, Setup::default());
    h.wait_containers(cx);
    h.wait_until(cx, "info loaded", |_, cx| {
        h.shell.read(cx).store().unwrap().read(cx).info().is_some()
    });
    let (store, list, mut status) = h.read(cx, |s, _, cx| {
        (
            s.store().unwrap().clone(),
            s.engines().clone(),
            s.engines().read(cx).active().unwrap().clone(),
        )
    });
    let tail = |cx: &mut TestAppContext| {
        h.draw(cx);
        let mut visual = gpui_kit::VisualTestContext::from_window(h.any_window(), cx);
        let left = visual.debug_bounds("status-left").expect("left segment");
        let transport = visual
            .debug_bounds("status-transport")
            .expect("transport tag");
        assert!(visual.debug_bounds("status-engine").is_some());
        (left.right(), transport.right())
    };
    let (left_right, transport_right) = tail(cx);
    assert_eq!(left_right, transport_right, "nothing follows the transport");
    let info = status.info.as_mut().unwrap();
    info.kind = dk_core::EngineKind::Wslc;
    info.transport = Some("com".into());
    info.api_version = Some("v3_0".into());
    info.transport_note = Some("CLI fallback: pull_image, stats".into());
    let expected = info.clone();
    cx.update(|cx| {
        list.update(cx, |s, cx| {
            s.apply_test_event(dk_hub::HubEvent::StatusChanged(status.clone()), cx)
        })
    });
    h.wait_until(cx, "snapshot applied", |_, cx| {
        store.read(cx).info() == Some(&expected)
    });
    let (left_right, transport_right) = tail(cx);
    assert_eq!(left_right, transport_right, "the note is not drawn");
    h.shutdown();
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
    assert_eq!(selected(cx), 16, "every container");
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
    let all = visual.debug_bounds("filter-all").expect("filter rendered");
    // Empty header space: left of the filters, below the title.
    let blank = gpui_kit::point(all.origin.x - gpui_kit::px(40.), all.center().y);
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

/// Height of the overlay panel and of its content. The panel adds only its padding and border
/// (about 23 px at the test rem), so there's no empty band under the content (KBD-020); a
/// stretched panel is 560 px whatever it holds.
fn overlay_heights(h: &crate::testing::Harness, cx: &mut TestAppContext) -> (f32, f32) {
    h.draw(cx);
    let mut visual = gpui_kit::VisualTestContext::from_window(h.any_window(), cx);
    let panel = visual.debug_bounds("overlay-panel").expect("panel");
    let body = visual.debug_bounds("overlay-body").expect("body");
    (panel.size.height.as_f32(), body.size.height.as_f32())
}

#[gpui_kit::test]
fn kbd_020_palette_panel_hugs_its_content(cx: &mut TestAppContext) {
    let h = start(cx, Setup::default());
    h.wait_containers(cx);
    h.focus_table(cx);
    h.press(cx, "secondary-p");
    assert_eq!(h.read(cx, |s, _, _| s.overlay()), Overlay::Palette);
    let (panel, body) = overlay_heights(&h, cx);
    assert!(
        (10.0..32.0).contains(&(panel - body)),
        "full palette: panel {panel} vs body {body}"
    );
    assert!(panel <= 560.0, "the panel respects its cap, got {panel}");
    h.type_text(cx, "Collapse all");
    cx.run_until_parked();
    let (panel, body) = overlay_heights(&h, cx);
    assert!(
        (10.0..32.0).contains(&(panel - body)),
        "filtered palette: panel {panel} vs body {body}"
    );
    assert!(panel < 300.0, "a two-row palette is short, got {panel}");
    h.shutdown();
}

#[gpui_kit::test]
fn kbd_022_reference_panel_hugs_its_content(cx: &mut TestAppContext) {
    let h = start(cx, Setup::default());
    h.wait_containers(cx);
    h.press(cx, "f1");
    assert_eq!(h.read(cx, |s, _, _| s.overlay()), Overlay::Shortcuts);
    let (panel, body) = overlay_heights(&h, cx);
    assert!(
        (10.0..32.0).contains(&(panel - body)),
        "full reference: panel {panel} vs body {body}"
    );
    assert!(panel <= 560.0, "the panel respects its cap, got {panel}");
    h.type_text(cx, "Next row");
    cx.run_until_parked();
    let (panel, body) = overlay_heights(&h, cx);
    assert!(
        (10.0..32.0).contains(&(panel - body)),
        "filtered reference: panel {panel} vs body {body}"
    );
    assert!(panel < 300.0, "a filtered reference is short, got {panel}");
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
    // Pick "None" from the menu (third item, after Combined and Compose project).
    h.press(cx, "down down down enter");
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
        cx.read(|cx| page.read(cx).group_by()),
        dk_core::grouping::GroupBy::None
    );
    h.shutdown();
}

const PART_OF: &str = dk_core::grouping::DEFAULT_GROUP_LABEL_KEY;

fn row_keys(
    cx: &mut TestAppContext,
    page: &gpui_kit::Entity<crate::pages::containers::ContainersPage>,
) -> Vec<String> {
    cx.read(|cx| {
        page.read(cx)
            .table()
            .read(cx)
            .model(cx)
            .rows()
            .iter()
            .map(|r| r.key.to_string())
            .collect()
    })
}

fn hub_group_by(h: &crate::testing::Harness) -> dk_core::grouping::GroupBy {
    h.hub.config().get().containers.group_by
}

/// Opens the group-by menu and picks the `n`-th item with the keyboard (1 = first).
fn pick_group_by(h: &crate::testing::Harness, cx: &mut TestAppContext, n: usize) {
    h.press(cx, "secondary-shift-g");
    h.press(cx, &vec!["down"; n].join(" "));
    h.press(cx, "enter");
}

#[gpui_kit::test]
fn con_010_combined_default_groups_label_fallback(cx: &mut TestAppContext) {
    let h = start(cx, Setup::default());
    let page = h.wait_containers(cx);
    assert_eq!(
        cx.read(|cx| page.read(cx).group_by()),
        dk_core::grouping::GroupBy::Combined
    );
    let keys = row_keys(cx, &page);
    for group in [
        "compose:myshop",
        "compose:monitoring",
        &format!("label:{PART_OF}=webshop"),
        &format!("label:{PART_OF}=analytics"),
    ] {
        assert!(keys.contains(&group.to_owned()), "{group} in {keys:?}");
    }
    let members = |cx: &mut TestAppContext, group: &str| -> Vec<String> {
        cx.read(|cx| {
            page.read(cx)
                .table()
                .read(cx)
                .model(cx)
                .row_by_key(group)
                .and_then(|r| {
                    r.group()
                        .map(|g| g.members.iter().map(|c| c.name.clone()).collect())
                })
                .unwrap_or_default()
        })
    };
    // A container with a Compose project and the label stays in its project.
    assert!(members(cx, "compose:monitoring").contains(&"monitoring-loki-1".to_owned()));
    assert_eq!(
        members(cx, &format!("label:{PART_OF}=analytics")),
        ["etl-worker"]
    );
    // Neither a project nor the label (another key doesn't count): ungrouped, after the groups.
    let top: Vec<(bool, String)> = cx.read(|cx| {
        page.read(cx)
            .table()
            .read(cx)
            .model(cx)
            .rows()
            .iter()
            .filter(|r| r.depth == 0)
            .map(|r| (r.is_group(), r.key.to_string()))
            .collect()
    });
    assert!(top.contains(&(false, id_of("redis"))));
    assert!(top.contains(&(false, id_of("billing-svc"))));
    let first_single = top.iter().position(|(is_group, _)| !is_group).unwrap();
    assert!(top[first_single..].iter().all(|(g, _)| !g), "{top:?}");
    h.shutdown();
}

/// CON-014: the menu has the four modes in order and picking one applies at once. There is
/// no label-key prompt: choosing *Label* opens no input and focus returns to the table.
#[gpui_kit::test]
fn con_014_no_label_prompt_on_containers_page(cx: &mut TestAppContext) {
    use dk_core::grouping::GroupBy;
    let h = start(cx, Setup::default());
    let page = h.wait_containers(cx);
    h.focus_table(cx);
    let table = cx.read(|cx| page.read(cx).table().focus_handle(cx));
    // Item n of the menu is Combined, Compose project, None, Label.
    for (n, want) in [
        (4, GroupBy::Label),
        (3, GroupBy::None),
        (2, GroupBy::Compose),
        (1, GroupBy::Combined),
    ] {
        pick_group_by(&h, cx, n);
        assert_eq!(cx.read(|cx| page.read(cx).group_by()), want, "item {n}");
        assert_eq!(hub_group_by(&h), want);
        assert!(!cx.read(|cx| page.read(cx).menu_open()));
        assert!(h.is_focused(cx, &table), "focus returns to the table");
        let search = cx.read(|cx| page.read(cx).search_input().focus_handle(cx));
        assert!(!h.is_focused(cx, &search));
    }
    // There is no fifth item: Down wraps around to the first one.
    pick_group_by(&h, cx, 5);
    assert_eq!(cx.read(|cx| page.read(cx).group_by()), GroupBy::Combined);
    assert_eq!(
        h.hub.config().get().containers.group_label_key,
        PART_OF,
        "the page never changes the key"
    );
    h.shutdown();
}

/// CON-014: changing the key in Settings regroups a mounted Containers page.
#[gpui_kit::test]
fn con_014_settings_key_change_regroups_mounted_page(cx: &mut TestAppContext) {
    use crate::state::AppState;
    let h = start(cx, Setup::default());
    let page = h.wait_containers(cx);
    let webshop = format!("label:{PART_OF}=webshop");
    assert!(row_keys(cx, &page).contains(&webshop));
    h.update(cx, |_, _, cx| {
        AppState::update_config(cx, |c| c.containers.group_label_key = "app".into())
    });
    h.wait_until(cx, "regrouped by app", |_, cx| {
        let t = page.read(cx).table().read(cx);
        let m = t.model(cx);
        m.row_by_key("label:app=billing").is_some() && m.row_by_key(&webshop).is_none()
    });
    // Compose projects still win in Combined.
    assert!(row_keys(cx, &page).contains(&"compose:myshop".to_owned()));
    h.update(cx, |_, _, cx| {
        AppState::update_config(cx, |c| c.containers.group_label_key = PART_OF.into())
    });
    h.wait_until(cx, "regrouped back", |_, cx| {
        page.read(cx)
            .table()
            .read(cx)
            .model(cx)
            .row_by_key(&webshop)
            .is_some()
    });
    h.shutdown();
}

/// CON-014, NFR-003/005: above the background threshold a key change rebuilds on a worker,
/// and the page ends on the newest key.
#[gpui_kit::test]
fn con_014_key_change_above_threshold_keeps_latest_result(cx: &mut TestAppContext) {
    use crate::state::AppState;
    let containers = (0..crate::pages::containers::BACKGROUND_THRESHOLD + 50)
        .map(|i| {
            let mut c = dk_core::fake::fixtures::container(
                &format!("bulk-{i:05}"),
                ContainerState::Running,
            );
            c.labels.insert("app".into(), format!("a{}", i % 5));
            c.labels.insert("team".into(), format!("t{}", i % 3));
            c
        })
        .collect();
    let h = start(
        cx,
        Setup {
            containers,
            ..Default::default()
        },
    );
    let page = h.wait_containers(cx);
    h.update(cx, |_, _, cx| {
        AppState::update_config(cx, |c| {
            c.containers.group_by = dk_core::grouping::GroupBy::Label;
            c.containers.group_label_key = "app".into();
        });
    });
    h.update(cx, |_, _, cx| {
        AppState::update_config(cx, |c| c.containers.group_label_key = "team".into());
    });
    h.wait_until(cx, "grouped by team", |_, cx| {
        page.read(cx)
            .table()
            .read(cx)
            .model(cx)
            .row_by_key("label:team=t0")
            .is_some()
    });
    cx.run_until_parked();
    let keys = row_keys(cx, &page);
    assert!(
        keys.iter().all(|k| !k.starts_with("label:app=")),
        "a stale build was applied: {keys:?}"
    );
    h.shutdown();
}

/// CON-011: a label group's header has the "label" tag with `key=value` as its tooltip and
/// no working directory; a Compose group keeps the "compose" tag.
#[gpui_kit::test]
fn con_011_label_group_has_label_tag_and_no_working_dir(cx: &mut TestAppContext) {
    use crate::pages::containers::model::group_header;
    let mut containers = crate::demo::containers();
    // A labelled container that carries a working directory but no Compose project.
    if let Some(c) = containers.iter_mut().find(|c| c.name == "etl-worker") {
        c.labels.insert(
            dk_core::grouping::COMPOSE_WORKING_DIR_LABEL.into(),
            "/srv/etl".into(),
        );
    }
    let h = start(
        cx,
        Setup {
            containers,
            ..Default::default()
        },
    );
    let page = h.wait_containers(cx);
    let analytics = format!("label:{PART_OF}=analytics");
    let (label_group, compose_group) = cx.read(|cx| {
        let t = page.read(cx).table().read(cx);
        let m = t.model(cx);
        let g = |key: &str| m.row_by_key(key).and_then(|r| r.group().cloned()).unwrap();
        (g(&analytics), g("compose:monitoring"))
    });
    assert!(!label_group.group.is_compose);
    assert_eq!(label_group.group.working_dir, None);
    let header = group_header(&label_group.group);
    assert_eq!(header.tag, crate::strings::LABEL_TAG);
    assert_eq!(
        header.tooltip.as_deref(),
        Some(format!("{PART_OF}=analytics").as_str())
    );
    let header = group_header(&compose_group.group);
    assert_eq!(header.tag, crate::strings::COMPOSE_TAG);
    assert_eq!(header.tooltip.as_deref(), Some("/home/dev/proj"));
    // The rendered headers carry the matching tag: both kinds in Combined, one kind in the
    // pure modes.
    let rendered = |cx: &mut TestAppContext| {
        h.draw(cx);
        let mut visual = gpui_kit::VisualTestContext::from_window(h.any_window(), cx);
        (
            visual.debug_bounds("group-tag-compose").is_some(),
            visual.debug_bounds("group-tag-label").is_some(),
        )
    };
    assert_eq!(rendered(cx), (true, true));
    for (by, want) in [
        (dk_core::grouping::GroupBy::Compose, (true, false)),
        (dk_core::grouping::GroupBy::Label, (false, true)),
    ] {
        h.update(cx, |_, _, cx| {
            crate::state::AppState::update_config(cx, |c| c.containers.group_by = by)
        });
        h.wait_until(cx, "regrouped", |_, cx| page.read(cx).group_by() == by);
        assert_eq!(rendered(cx), want, "{by:?}");
    }
    h.shutdown();
}

/// CON-013: only a Compose group's *Delete all* dialog carries the compose-down note.
#[gpui_kit::test]
fn con_013_label_group_delete_dialog_has_no_compose_note(cx: &mut TestAppContext) {
    let h = start(cx, Setup::default());
    h.wait_containers(cx);
    h.focus_table(cx);
    let note_shown = |cx: &mut TestAppContext| {
        h.draw(cx);
        gpui_kit::VisualTestContext::from_window(h.any_window(), cx)
            .debug_bounds("confirm-note")
            .is_some()
    };
    h.select_row(cx, "compose:myshop");
    h.press(cx, "delete");
    assert!(h.has_dialog(cx));
    assert!(note_shown(cx), "a Compose group says networks are kept");
    h.press(cx, "escape");
    assert!(!h.has_dialog(cx));
    h.select_row(cx, &format!("label:{PART_OF}=webshop"));
    h.press(cx, "delete");
    assert!(h.has_dialog(cx));
    assert!(!note_shown(cx), "a label group has no compose-down note");
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

/// Pointer commands resolve where the user clicked, not wherever keyboard focus was left
/// (KBD-007). Opening the engine switcher with the mouse and dismissing it with a click on
/// the title bar used to leave focus in the title bar; header buttons dispatch their action
/// from the focused element, so the group-by menu and the row ⋮ menu stopped opening.
#[gpui_kit::test]
fn kbd_007_header_menus_open_after_switcher_dismissed_by_title_click(cx: &mut TestAppContext) {
    let h = start(cx, Setup::default());
    let page = h.wait_containers(cx);
    let click = |cx: &mut TestAppContext, selector: &'static str, dx: f32| {
        h.draw(cx);
        let mut visual = gpui_kit::VisualTestContext::from_window(h.any_window(), cx);
        let b = visual
            .debug_bounds(selector)
            .unwrap_or_else(|| panic!("{selector} rendered"));
        let at = gpui_kit::point(b.center().x + gpui_kit::px(dx), b.center().y);
        visual.simulate_click(at, Default::default());
        cx.run_until_parked();
    };
    let switcher_click_away = |cx: &mut TestAppContext| {
        click(cx, "engine-switcher-button", 0.);
        assert_eq!(h.read(cx, |s, _, _| s.overlay()), Overlay::Switcher);
        // Empty title-bar space left of the switcher button.
        click(cx, "engine-switcher-button", -260.);
        assert_eq!(h.read(cx, |s, _, _| s.overlay()), Overlay::None);
    };

    switcher_click_away(cx);
    click(cx, "group-by", 0.);
    assert!(
        cx.read(|cx| page.read(cx).menu_open()),
        "group-by menu opens after the switcher was dismissed"
    );
    h.press(cx, "escape");
    assert!(!cx.read(|cx| page.read(cx).menu_open()));

    switcher_click_away(cx);
    click(cx, "row-more-1", 0.);
    assert!(
        cx.read(|cx| page.read(cx).table().read(cx).key_menu_open()),
        "row ⋮ menu opens after the switcher was dismissed"
    );
    h.shutdown();
}
