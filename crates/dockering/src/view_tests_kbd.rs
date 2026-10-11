//! View tests for the keyboard flow polish (KBD-094…098, SHL-014, CDT-083). Real `HubHandle` +
//! `FakeEngine` (no Docker); test names carry requirement ids.

use dk_core::ContainerState;
use gpui_kit::{AppContext as _, Entity, Focusable, TestAppContext};

use crate::nav::{ContainerTab, Route};
use crate::pages::container_detail::ContainerDetailPage;
use crate::pages::images::ImagesPage;
use crate::pages::networks::NetworksPage;
use crate::pages::volumes::VolumesPage;
use crate::testing::{Harness, Setup, id_of, start};
use crate::view_tests_detail::open_detail;

fn containers_table_focused(h: &Harness, cx: &mut TestAppContext) -> bool {
    let page = h.page(cx).expect("containers page");
    cx.update_window(h.any_window(), |_, window, cx| {
        page.read(cx).table().focus_handle(cx).is_focused(window)
    })
    .unwrap()
}

fn search_focused(h: &Harness, cx: &mut TestAppContext) -> bool {
    let page = h.page(cx).expect("containers page");
    cx.update_window(h.any_window(), |_, window, cx| {
        page.read(cx)
            .search_input()
            .focus_handle(cx)
            .is_focused(window)
    })
    .unwrap()
}

fn route(h: &Harness, cx: &mut TestAppContext) -> Route {
    h.read(cx, |s, _, _| s.route().clone())
}

/// Types `query` into the focused page search field and presses `Enter` straight away, so the
/// 120 ms debounce can't have applied it yet.
fn search_and_enter(h: &Harness, cx: &mut TestAppContext, query: &str) {
    h.press(cx, "secondary-f");
    h.type_text(cx, query);
    h.press(cx, "enter");
}

// ── KBD-094 ──────────────────────────────────────────────────────────────────────────────

#[gpui_kit::test]
fn kbd_094_enter_in_search_focuses_first_match(cx: &mut TestAppContext) {
    let h = start(cx, Setup::default());
    h.wait_containers(cx);
    h.focus_table(cx);
    search_and_enter(&h, cx, "redis");
    assert!(containers_table_focused(&h, cx), "focus moved to the table");
    assert_eq!(
        h.cursor_key(cx),
        Some(id_of("redis")),
        "cursor on the match"
    );
    // A second Enter opens it (KBD-032).
    h.press(cx, "enter");
    assert_eq!(
        route(&h, cx),
        Route::ContainerDetail {
            id: id_of("redis"),
            tab: crate::nav::ContainerTab::Overview,
        }
    );
    h.shutdown();
}

#[gpui_kit::test]
fn kbd_094_single_enter_does_not_open_detail(cx: &mut TestAppContext) {
    let h = start(cx, Setup::default());
    h.wait_containers(cx);
    search_and_enter(&h, cx, "redis");
    assert_eq!(route(&h, cx), Route::Containers);
    h.shutdown();
}

#[gpui_kit::test]
fn kbd_094_enter_skips_group_header(cx: &mut TestAppContext) {
    let h = start(cx, Setup::default());
    let page = h.wait_containers(cx);
    search_and_enter(&h, cx, "myshop");
    assert!(containers_table_focused(&h, cx));
    let on_group = cx.read(|cx| {
        page.read(cx)
            .table()
            .read(cx)
            .model(cx)
            .cursor_row()
            .is_some_and(|r| r.is_group())
    });
    assert!(
        !on_group,
        "the cursor is on a container, not the group header"
    );
    h.shutdown();
}

#[gpui_kit::test]
fn kbd_094_enter_without_matches_stays_in_the_field(cx: &mut TestAppContext) {
    let h = start(cx, Setup::default());
    h.wait_containers(cx);
    search_and_enter(&h, cx, "no-such-container");
    assert!(search_focused(&h, cx), "focus stays in the search field");
    assert!(!containers_table_focused(&h, cx));
    h.shutdown();
}

#[gpui_kit::test]
fn kbd_094_enter_with_empty_query_keeps_the_cursor(cx: &mut TestAppContext) {
    let h = start(cx, Setup::default());
    h.wait_containers(cx);
    h.focus_table(cx);
    h.select_row(cx, &id_of("scratchpad"));
    h.press(cx, "secondary-f");
    h.press(cx, "enter");
    assert!(containers_table_focused(&h, cx));
    assert_eq!(h.cursor_key(cx), Some(id_of("scratchpad")));
    h.shutdown();
}

#[gpui_kit::test]
fn kbd_094_shift_enter_does_not_hand_off(cx: &mut TestAppContext) {
    let h = start(cx, Setup::default());
    h.wait_containers(cx);
    h.press(cx, "secondary-f");
    h.type_text(cx, "redis");
    h.press(cx, "shift-enter");
    assert!(search_focused(&h, cx));
    h.shutdown();
}

#[gpui_kit::test]
fn kbd_094_enter_hands_off_on_images_volumes_networks(cx: &mut TestAppContext) {
    let h = start(cx, Setup::default());
    h.wait_containers(cx);

    let images = h.goto::<ImagesPage>(cx, Route::Images);
    h.wait_until(cx, "image rows", |_, cx| {
        !images.read(cx).table().read(cx).model(cx).rows().is_empty()
    });
    search_and_enter(&h, cx, "postgres");
    h.draw(cx);
    let (focused, rows) = cx
        .update_window(h.any_window(), |_, window, cx| {
            let t = images.read(cx).table().clone();
            (
                t.focus_handle(cx).is_focused(window),
                t.read(cx).model(cx).rows().len(),
            )
        })
        .unwrap();
    assert!(focused, "images: table focused");
    assert_eq!(rows, 1, "images: the query applied at once");

    let volumes = h.goto::<VolumesPage>(cx, Route::Volumes);
    h.wait_until(cx, "volume rows", |_, cx| {
        !volumes
            .read(cx)
            .table()
            .read(cx)
            .model(cx)
            .rows()
            .is_empty()
    });
    search_and_enter(&h, cx, "pgdata");
    h.draw(cx);
    let (focused, cursor) = cx
        .update_window(h.any_window(), |_, window, cx| {
            let t = volumes.read(cx).table().clone();
            (
                t.focus_handle(cx).is_focused(window),
                t.read(cx).model(cx).cursor().cloned(),
            )
        })
        .unwrap();
    assert!(focused, "volumes: table focused");
    assert_eq!(cursor.as_deref(), Some("myshop_pgdata"));

    let networks = h.goto::<NetworksPage>(cx, Route::Networks);
    h.wait_until(cx, "network rows", |_, cx| {
        !networks
            .read(cx)
            .table()
            .read(cx)
            .model(cx)
            .rows()
            .is_empty()
    });
    search_and_enter(&h, cx, "bridge");
    h.draw(cx);
    let focused = cx
        .update_window(h.any_window(), |_, window, cx| {
            networks
                .read(cx)
                .table()
                .focus_handle(cx)
                .is_focused(window)
        })
        .unwrap();
    assert!(focused, "networks: table focused");
    h.shutdown();
}

#[gpui_kit::test]
fn kbd_094_enter_waits_for_background_build(cx: &mut TestAppContext) {
    let mut containers: Vec<_> = (0..2_100)
        .map(|i| dk_core::fake::fixtures::container(&format!("bulk-{i}"), ContainerState::Running))
        .collect();
    containers.push(dk_core::fake::fixtures::container(
        "needle",
        ContainerState::Running,
    ));
    let needle = containers.last().map(|c| c.id.clone()).unwrap();
    let h = start(
        cx,
        Setup {
            containers,
            ..Default::default()
        },
    );
    h.wait_until(cx, "bulk rows", |_, cx| {
        h.shell
            .read(cx)
            .containers_page()
            .is_some_and(|p| p.read(cx).table().read(cx).model(cx).rows().len() > 2_000)
    });
    search_and_enter(&h, cx, "needle");
    h.wait_until(cx, "hand-off after the background build", |window, cx| {
        let Some(p) = h.shell.read(cx).containers_page().cloned() else {
            return false;
        };
        let t = p.read(cx).table().clone();
        let one_row = t.read(cx).model(cx).rows().len() == 1;
        one_row
            && t.focus_handle(cx).is_focused(window)
            && t.read(cx).model(cx).cursor().map(|k| k.to_string()) == Some(needle.clone())
    });
    h.shutdown();
}

// ── SHL-014: list pages keep their state ─────────────────────────────────────────────────

fn back_key() -> &'static str {
    if cfg!(target_os = "macos") {
        "cmd-["
    } else {
        "alt-left"
    }
}

fn open_container_detail(h: &Harness, cx: &mut TestAppContext, id: &str) {
    let id = id.to_owned();
    h.update(cx, |_, window, cx| {
        window.dispatch_action(
            Box::new(crate::actions::Navigate {
                route: Route::ContainerDetail {
                    id,
                    tab: crate::nav::ContainerTab::Overview,
                },
            }),
            cx,
        )
    });
    h.wait_until(cx, "detail mounted", |_, cx| {
        matches!(
            h.shell.read(cx).page(),
            crate::shell::ShellPage::ContainerDetail(_)
        )
    });
}

#[gpui_kit::test]
fn shl_014_containers_state_survives_detail_round_trip(cx: &mut TestAppContext) {
    let h = start(cx, Setup::default());
    let page = h.wait_containers(cx);
    h.focus_table(cx);
    h.press(cx, "secondary-f");
    h.type_text(cx, "my");
    h.wait_until(cx, "search applied", |_, cx| {
        page.read(cx).table().read(cx).model(cx).rows().len() < 10
    });
    let rows_before = cx.read(|cx| page.read(cx).table().read(cx).model(cx).rows().len());
    h.focus_table(cx);
    h.select_row(cx, &id_of("myshop-db-1"));
    h.press(cx, "space");
    h.select_row(cx, &id_of("myshop-web-1"));

    // Leave and come back by different routes (D1).
    for (n, ret) in ["back", "sidebar", "alt-up"].into_iter().enumerate() {
        open_container_detail(&h, cx, &id_of("myshop-web-1"));
        assert_eq!(h.read(cx, |s, _, _| s.parked_pages()), 1, "list parked");
        match ret {
            "back" => h.press(cx, back_key()),
            "sidebar" => h.press(cx, "secondary-1"),
            _ => h.press(cx, "alt-up"),
        }
        h.wait_until(cx, "list back", |_, cx| {
            h.shell.read(cx).containers_page().is_some()
        });
        let same = h.read(cx, |s, _, _| s.containers_page() == Some(&page));
        assert!(same, "route {n}: the same list page comes back");
        let (value, rows, selected, cursor) = cx.read(|cx| {
            let p = page.read(cx);
            let m = p.table().read(cx).model(cx);
            (
                p.search_input().read(cx).value().to_string(),
                m.rows().len(),
                m.selected().len(),
                m.cursor().map(|k| k.to_string()),
            )
        });
        assert_eq!(value, "my", "route {n}: search text");
        assert_eq!(rows, rows_before, "route {n}: the query still applies");
        assert_eq!(selected, 1, "route {n}: checked rows");
        assert_eq!(cursor, Some(id_of("myshop-web-1")), "route {n}: cursor row");
        assert!(containers_table_focused(&h, cx), "route {n}: table focused");
        assert_eq!(h.read(cx, |s, _, _| s.parked_pages()), 0);
    }
    h.shutdown();
}

#[gpui_kit::test]
fn shl_014_scroll_position_survives(cx: &mut TestAppContext) {
    let containers: Vec<_> = (0..150)
        .map(|i| {
            dk_core::fake::fixtures::container(&format!("bulk-{i:03}"), ContainerState::Running)
        })
        .collect();
    let far = containers[120].id.clone();
    let h = start(
        cx,
        Setup {
            containers,
            ..Default::default()
        },
    );
    let page = h.wait_containers(cx);
    h.focus_table(cx);
    h.select_row(cx, &far);
    h.draw(cx);
    h.draw(cx);
    // The page the shell shows now, not a handle kept from before the round trip.
    let offset = |cx: &mut TestAppContext| {
        cx.read(|cx| {
            let page = h
                .shell
                .read(cx)
                .containers_page()
                .cloned()
                .expect("containers page");
            let t = page.read(cx).table().read(cx).table().clone();
            t.read(cx)
                .vertical_scroll_handle
                .0
                .borrow()
                .base_handle
                .offset()
                .y
                .as_f32()
        })
    };
    let before = offset(cx);
    assert!(before < -100.0, "the list is scrolled down, got {before}");
    open_container_detail(&h, cx, &far);
    h.press(cx, back_key());
    h.wait_until(cx, "list back", |_, cx| {
        h.shell.read(cx).containers_page().is_some()
    });
    h.draw(cx);
    h.draw(cx);
    assert_eq!(offset(cx), before, "scroll position restored");
    let same = h.read(cx, |s, _, _| s.containers_page() == Some(&page));
    assert!(same, "the same list page is shown");
    h.shutdown();
}

#[gpui_kit::test]
fn shl_014_images_and_volumes_keep_state_while_parked(cx: &mut TestAppContext) {
    let h = start(cx, Setup::default());
    h.wait_containers(cx);
    let images = h.goto::<ImagesPage>(cx, Route::Images);
    h.wait_until(cx, "image rows", |_, cx| {
        images.read(cx).table().read(cx).model(cx).rows().len() > 1
    });
    h.press(cx, "secondary-f");
    h.type_text(cx, "a");
    h.wait_until(cx, "images filtered", |_, cx| {
        images.read(cx).search_input().read(cx).value().as_ref() == "a"
    });
    let rows = cx.read(|cx| images.read(cx).table().read(cx).model(cx).rows().len());
    let volumes = h.goto::<VolumesPage>(cx, Route::Volumes);
    h.wait_until(cx, "volume rows", |_, cx| {
        !volumes
            .read(cx)
            .table()
            .read(cx)
            .model(cx)
            .rows()
            .is_empty()
    });
    // Containers (left earlier) and Images are both parked while Volumes shows.
    assert_eq!(h.read(cx, |s, _, _| s.parked_pages()), 2);
    h.draw(cx);
    h.press(cx, "secondary-2");
    h.wait_until(cx, "images back", |_, cx| {
        matches!(h.shell.read(cx).route(), Route::Images)
    });
    let same = h.dyn_page::<ImagesPage>(cx).is_some_and(|p| p == images);
    assert!(same, "the same Images page comes back");
    let (value, now) = cx.read(|cx| {
        (
            images.read(cx).search_input().read(cx).value().to_string(),
            images.read(cx).table().read(cx).model(cx).rows().len(),
        )
    });
    assert_eq!((value.as_str(), now), ("a", rows));
    h.shutdown();
}

#[gpui_kit::test]
fn shl_014_engine_switch_drops_parked_lists(cx: &mut TestAppContext) {
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
    search_and_enter(&h, cx, "redis");
    open_container_detail(&h, cx, &id_of("redis"));
    assert_eq!(h.read(cx, |s, _, _| s.parked_pages()), 1);
    h.press(cx, "secondary-k");
    h.type_text(cx, "other");
    h.press(cx, "enter");
    h.wait_until(cx, "switched", |_, cx| {
        h.shell
            .read(cx)
            .store()
            .is_some_and(|s| s.read(cx).engine_id().as_str() == "other")
    });
    h.wait_until(cx, "fresh list", |_, cx| {
        h.shell.read(cx).containers_page().is_some_and(|p| {
            p.read(cx).search_input().read(cx).value().is_empty()
                && !p.read(cx).table().read(cx).model(cx).rows().is_empty()
        })
    });
    assert_eq!(h.read(cx, |s, _, _| s.parked_pages()), 0);
    h.shutdown();
}

#[gpui_kit::test]
fn sta_006_parked_list_has_no_stats_subscriptions(cx: &mut TestAppContext) {
    let mut setup = Setup::default();
    setup.config.containers.show_cpu_mem_columns = true;
    let h = start(cx, setup);
    let page = h.wait_containers(cx);
    h.wait_until(cx, "list stats", |_, cx| {
        page.read(cx).stats_subscription_count() > 0
    });
    open_container_detail(&h, cx, &id_of("redis"));
    h.wait_until(cx, "stats stopped while parked", |_, cx| {
        page.read(cx).stats_subscription_count() == 0
    });
    h.press(cx, back_key());
    h.wait_until(cx, "stats resumed", |_, cx| {
        page.read(cx).stats_subscription_count() > 0
    });
    h.shutdown();
}

#[gpui_kit::test]
fn kbd_042_alt_up_clears_search_and_filter_hiding_the_row(cx: &mut TestAppContext) {
    let h = start(cx, Setup::default());
    let page = h.wait_containers(cx);
    search_and_enter(&h, cx, "redis");
    open_container_detail(&h, cx, &id_of("myshop-db-1"));
    h.press(cx, "alt-up");
    h.wait_until(cx, "search cleared, cursor revealed", |_, cx| {
        let Some(p) = h.shell.read(cx).containers_page().cloned() else {
            return false;
        };
        let p = p.read(cx);
        p.search_input().read(cx).value().is_empty()
            && p.table().read(cx).model(cx).cursor().map(|k| k.to_string())
                == Some(id_of("myshop-db-1"))
    });
    // A status filter that hides the row is reset too.
    h.focus_table(cx);
    h.update(cx, |_, window, cx| {
        window.dispatch_action(
            Box::new(crate::actions::SetFilter {
                filter: "stopped".into(),
            }),
            cx,
        )
    });
    h.wait_until(cx, "stopped filter", |_, cx| {
        page.read(cx).filter() == crate::pages::containers::StatusFilter::Stopped
    });
    open_container_detail(&h, cx, &id_of("redis"));
    h.press(cx, "alt-up");
    h.wait_until(cx, "filter cleared, cursor revealed", |_, cx| {
        let Some(p) = h.shell.read(cx).containers_page().cloned() else {
            return false;
        };
        let p = p.read(cx);
        p.filter() == crate::pages::containers::StatusFilter::All
            && p.table().read(cx).model(cx).cursor().map(|k| k.to_string()) == Some(id_of("redis"))
    });
    h.shutdown();
}

// ── KBD-098: Backspace goes back on detail pages ─────────────────────────────────────────

fn tab_bar(h: &Harness, page: &Entity<ContainerDetailPage>, cx: &mut TestAppContext) {
    let f = cx.read(|cx| page.read(cx).tab_bar_focus().clone());
    h.focus(cx, &f);
}

#[gpui_kit::test]
fn kbd_098_backspace_goes_back_on_detail(cx: &mut TestAppContext) {
    let h = start(cx, Setup::default());
    let page = open_detail(&h, cx, "redis", ContainerTab::Overview);
    tab_bar(&h, &page, cx);
    h.press(cx, "backspace");
    assert_eq!(route(&h, cx), Route::Containers, "from the tab bar");
    // The list comes back with the cursor where it was, and its table focused: a second
    // Backspace must not delete the row (KBD-030).
    assert!(containers_table_focused(&h, cx));
    h.engine.clear_calls();
    h.press(cx, "backspace");
    h.draw(cx);
    assert!(!h.has_dialog(cx), "Backspace is not a delete key on lists");
    assert!(h.engine.calls_to("remove_container").is_empty());
    // From a body panel too (rows panel).
    let page = open_detail(&h, cx, "redis", ContainerTab::Overview);
    let rows = cx.read(|cx| {
        page.read(cx)
            .tabs()
            .overview
            .clone()
            .map(|o| o.focus_handle(cx))
    });
    h.focus(cx, &rows.expect("overview tab"));
    h.press(cx, "backspace");
    assert_eq!(route(&h, cx), Route::Containers, "from the rows panel");
    h.shutdown();
}

#[gpui_kit::test]
fn kbd_098_backspace_is_inert_in_logs_search_and_inspect_editor(cx: &mut TestAppContext) {
    let h = start(cx, Setup::default());
    let page = open_detail(&h, cx, "redis", ContainerTab::Logs);
    let logs = cx.read(|cx| page.read(cx).tabs().logs.clone()).unwrap();
    tab_bar(&h, &page, cx);
    h.press(cx, "secondary-f");
    h.type_text(cx, "abc");
    h.press(cx, "backspace");
    let value = cx.read(|cx| logs.read(cx).search_input().read(cx).value().to_string());
    assert_eq!(value, "ab", "Backspace edits the search");
    assert!(matches!(route(&h, cx), Route::ContainerDetail { .. }));
    // Even with nothing left to delete the field keeps the key.
    h.press(cx, "backspace backspace backspace");
    assert!(matches!(route(&h, cx), Route::ContainerDetail { .. }));

    h.update(cx, |_, window, cx| {
        window.dispatch_action(
            Box::new(crate::actions::detail_tab::SelectTab {
                tab: ContainerTab::Inspect,
            }),
            cx,
        )
    });
    h.draw(cx);
    let inspect = cx.read(|cx| page.read(cx).tabs().inspect.clone()).unwrap();
    let editor = cx.read(|cx| inspect.focus_handle(cx));
    h.focus(cx, &editor);
    h.press(cx, "backspace");
    assert!(
        matches!(route(&h, cx), Route::ContainerDetail { .. }),
        "the read-only editor keeps Backspace"
    );
    h.shutdown();
}

#[gpui_kit::test]
fn kbd_098_backspace_goes_back_on_resource_details(cx: &mut TestAppContext) {
    use crate::nav::{ImageTab, NetworkTab, VolumeTab};
    let h = start(cx, Setup::default());
    h.wait_containers(cx);
    let images = h.goto::<ImagesPage>(cx, Route::Images);
    h.wait_until(cx, "image rows", |_, cx| {
        !images.read(cx).table().read(cx).model(cx).rows().is_empty()
    });
    let image_id = cx.read(|cx| images.read(cx).rows()[0].image_id.clone());
    for route in [
        Route::ImageDetail {
            id: image_id,
            tab: ImageTab::Overview,
        },
        Route::VolumeDetail {
            name: "myshop_pgdata".into(),
            tab: VolumeTab::Overview,
        },
        Route::NetworkDetail {
            id: "myshop_default".into(),
            tab: NetworkTab::Overview,
        },
    ] {
        let before = h.read(cx, |s, _, _| s.route().clone());
        h.update(cx, |_, window, cx| {
            window.dispatch_action(Box::new(crate::actions::Navigate { route }), cx)
        });
        h.draw(cx);
        h.draw(cx);
        assert!(h.read(cx, |s, _, _| s.route().is_detail()));
        h.press(cx, "backspace");
        assert_eq!(
            h.read(cx, |s, _, _| s.route().clone()),
            before,
            "Backspace goes back from a resource detail"
        );
    }
    h.shutdown();
}

#[gpui_kit::test]
fn kbd_098_backspace_goes_to_the_terminal_not_back(cx: &mut TestAppContext) {
    let h = start(cx, Setup::default());
    let page = open_detail(&h, cx, "redis", ContainerTab::Terminal);
    let tab = cx
        .read(|cx| page.read(cx).tabs().terminal.clone())
        .expect("terminal tab");
    h.wait_until(cx, "terminal focused", |window, cx| {
        tab.read(cx)
            .active_view()
            .is_some_and(|v| v.read(cx).focus_handle(cx).is_focused(window))
    });
    h.press(cx, "backspace");
    assert!(
        matches!(route(&h, cx), Route::ContainerDetail { .. }),
        "the terminal keeps Backspace for the shell"
    );
    h.shutdown();
}

// ── KBD-095 / 096 / 097: viewing a detail tab from the keyboard ──────────────────────────

fn leave_terminal_key() -> &'static str {
    if cfg!(target_os = "macos") {
        "cmd-shift-f6"
    } else {
        "ctrl-shift-f6"
    }
}

fn select_tab(h: &Harness, cx: &mut TestAppContext, tab: ContainerTab) {
    h.update(cx, |_, window, cx| {
        window.dispatch_action(Box::new(crate::actions::detail_tab::SelectTab { tab }), cx)
    });
    h.draw(cx);
}

fn bar_handle(
    page: &Entity<ContainerDetailPage>,
    cx: &mut TestAppContext,
) -> gpui_kit::FocusHandle {
    cx.read(|cx| page.read(cx).tab_bar_focus().clone())
}

/// `redis` on the Overview tab with enough env entries that the rows panel overflows.
fn tall_overview(h: &Harness, cx: &mut TestAppContext) -> Entity<ContainerDetailPage> {
    let summary = crate::demo::containers()
        .into_iter()
        .find(|c| c.name == "redis")
        .expect("redis");
    let mut d = dk_core::fake::fixtures::details_for(summary);
    d.env = (0..80)
        .map(|i| dk_core::EnvVar {
            key: format!("VAR_{i:02}"),
            value: format!("value-{i}"),
        })
        .collect();
    h.engine.set_container_details(d);
    open_detail(h, cx, "redis", ContainerTab::Overview)
}

#[gpui_kit::test]
fn kbd_095_down_enters_the_rows_tabs_and_esc_returns(cx: &mut TestAppContext) {
    let h = start(cx, Setup::default());
    let page = open_detail(&h, cx, "redis", ContainerTab::Overview);
    for tab in [
        ContainerTab::Overview,
        ContainerTab::Mounts,
        ContainerTab::Network,
    ] {
        select_tab(&h, cx, tab);
        let bar = bar_handle(&page, cx);
        h.focus(cx, &bar);
        h.press(cx, "down");
        let rows = cx.read(|cx| {
            let t = page.read(cx).tabs();
            match tab {
                ContainerTab::Overview => t.overview.as_ref().map(|t| t.focus_handle(cx)),
                ContainerTab::Mounts => t.mounts.as_ref().map(|t| t.focus_handle(cx)),
                _ => t.network.as_ref().map(|t| t.focus_handle(cx)),
            }
        });
        assert!(
            h.is_focused(cx, &rows.expect("tab entity")),
            "{tab:?}: ↓ focuses the rows panel"
        );
        h.press(cx, "escape");
        assert!(
            h.is_focused(cx, &bar),
            "{tab:?}: Esc returns to the tab bar"
        );
    }
    h.shutdown();
}

#[gpui_kit::test]
fn kbd_095_down_enters_logs_stats_inspect_and_terminal(cx: &mut TestAppContext) {
    let h = start(cx, Setup::default());
    let page = open_detail(&h, cx, "redis", ContainerTab::Logs);
    let bar = bar_handle(&page, cx);
    // Logs → the log lines.
    h.focus(cx, &bar);
    h.press(cx, "down");
    let lines = cx.read(|cx| {
        page.read(cx)
            .tabs()
            .logs
            .as_ref()
            .map(|l| l.read(cx).list_focus().clone())
    });
    assert!(h.is_focused(cx, &lines.unwrap()), "logs: the log lines");
    h.press(cx, "escape");
    assert!(h.is_focused(cx, &bar), "logs: Esc returns to the tab bar");
    // Stats → the time-window selector.
    select_tab(&h, cx, ContainerTab::Stats);
    h.focus(cx, &bar);
    h.press(cx, "down");
    let selector = cx.read(|cx| {
        page.read(cx)
            .tabs()
            .stats
            .as_ref()
            .map(|s| s.read(cx).window_focus().clone())
    });
    assert!(h.is_focused(cx, &selector.unwrap()), "stats: selector");
    h.press(cx, "escape");
    assert!(h.is_focused(cx, &bar), "stats: Esc returns to the tab bar");
    // Inspect → the read-only editor; Esc leaves it.
    select_tab(&h, cx, ContainerTab::Inspect);
    h.focus(cx, &bar);
    h.press(cx, "down");
    let editor = cx.read(|cx| {
        page.read(cx)
            .tabs()
            .inspect
            .as_ref()
            .map(|i| i.focus_handle(cx))
    });
    assert!(h.is_focused(cx, &editor.unwrap()), "inspect: the editor");
    h.press(cx, "escape");
    assert!(
        h.is_focused(cx, &bar),
        "inspect: Esc returns to the tab bar"
    );
    // Terminal → the active session; Mod+Shift+F6 brings focus back.
    select_tab(&h, cx, ContainerTab::Terminal);
    let tab = cx.read(|cx| page.read(cx).tabs().terminal.clone()).unwrap();
    h.wait_until(cx, "terminal session", |_, cx| {
        tab.read(cx).active_view().is_some()
    });
    h.focus(cx, &bar);
    h.press(cx, "down");
    let session = cx.read(|cx| tab.read(cx).active_view().map(|v| v.focus_handle(cx)));
    assert!(
        h.is_focused(cx, &session.unwrap()),
        "terminal: the active session"
    );
    h.press(cx, leave_terminal_key());
    assert!(h.is_focused(cx, &bar), "terminal: Mod+Shift+F6 returns");
    h.shutdown();
}

#[gpui_kit::test]
fn kbd_095_down_without_content_does_nothing(cx: &mut TestAppContext) {
    let h = start(cx, Setup::default());
    // A stopped container has no terminal session.
    let page = open_detail(&h, cx, "scratchpad", ContainerTab::Terminal);
    let bar = bar_handle(&page, cx);
    h.focus(cx, &bar);
    h.press(cx, "down");
    assert!(h.is_focused(cx, &bar), "focus stays on the tab bar");
    h.shutdown();
}

#[gpui_kit::test]
fn kbd_096_page_keys_scroll_the_rows_panel_from_the_bar(cx: &mut TestAppContext) {
    let h = start(cx, Setup::default());
    let page = tall_overview(&h, cx);
    h.draw(cx);
    h.draw(cx);
    let bar = bar_handle(&page, cx);
    h.focus(cx, &bar);
    let overview = cx.read(|cx| page.read(cx).tabs().overview.clone()).unwrap();
    let offset = |cx: &mut TestAppContext| cx.read(|cx| overview.read(cx).rows().scroll_offset_y());
    assert_eq!(offset(cx), 0.0);
    h.press(cx, "pagedown");
    h.draw(cx);
    let one = offset(cx);
    assert!(one < -100.0, "PgDn scrolled by a page, got {one}");
    h.press(cx, "pagedown");
    h.draw(cx);
    let two = offset(cx);
    assert!(two < one, "PgDn again scrolls further ({one} → {two})");
    h.press(cx, "pageup pageup pageup");
    h.draw(cx);
    assert_eq!(offset(cx), 0.0, "PgUp scrolls back to the top");
    h.shutdown();
}

#[gpui_kit::test]
fn kbd_097_up_on_the_first_row_returns_to_the_bar(cx: &mut TestAppContext) {
    let h = start(cx, Setup::default());
    let page = open_detail(&h, cx, "redis", ContainerTab::Overview);
    let bar = bar_handle(&page, cx);
    h.focus(cx, &bar);
    h.press(cx, "down");
    let overview = cx.read(|cx| page.read(cx).tabs().overview.clone()).unwrap();
    let rows = cx.read(|cx| overview.focus_handle(cx));
    assert!(h.is_focused(cx, &rows));
    h.press(cx, "down up");
    assert!(h.is_focused(cx, &rows), "↑ off a later row only moves");
    h.press(cx, "up");
    assert!(h.is_focused(cx, &bar), "↑ on the first row returns");
    h.shutdown();
}

#[gpui_kit::test]
fn kbd_096_page_up_on_the_bar_pauses_log_follow(cx: &mut TestAppContext) {
    let h = start(cx, Setup::default());
    let page = open_detail(&h, cx, "redis", ContainerTab::Logs);
    let logs = cx.read(|cx| page.read(cx).tabs().logs.clone()).unwrap();
    crate::view_tests_detail::wait_log_stream(&h, cx);
    for i in 0..200 {
        crate::view_tests_detail::push(
            &h,
            "redis",
            dk_core::LogStream::Stdout,
            &format!("line {i}"),
        );
    }
    crate::view_tests_detail::wait_lines(&h, cx, &logs, 200);
    h.draw(cx);
    h.draw(cx);
    let bar = bar_handle(&page, cx);
    h.focus(cx, &bar);
    assert!(cx.read(|cx| logs.read(cx).is_following()), "tailing");
    h.press(cx, "pageup");
    h.draw(cx);
    assert!(
        !cx.read(|cx| logs.read(cx).is_following()),
        "PgUp from the bar scrolled the lines and paused follow"
    );
    h.shutdown();
}

#[gpui_kit::test]
fn kbd_096_terminal_page_keys_scroll_scrollback_not_the_shell(cx: &mut TestAppContext) {
    let h = start(cx, Setup::default());
    let page = open_detail(&h, cx, "redis", ContainerTab::Terminal);
    let tab = cx.read(|cx| page.read(cx).tabs().terminal.clone()).unwrap();
    h.wait_until(cx, "terminal session", |_, cx| {
        tab.read(cx).active_view().is_some()
    });
    let view = cx.read(|cx| tab.read(cx).active_view().cloned()).unwrap();
    cx.update(|cx| view.update(cx, |v, cx| v.feed(&b"x\r\n".repeat(300), cx)));
    h.draw(cx);
    let bar = bar_handle(&page, cx);
    h.focus(cx, &bar);
    let before = cx.read(|cx| view.read(cx).model().display_offset());
    h.press(cx, "pageup");
    let after = cx.read(|cx| view.read(cx).model().display_offset());
    assert!(after > before, "scrollback scrolled ({before} → {after})");
    h.shutdown();
}

#[gpui_kit::test]
fn kbd_096_stats_and_inspect_scroll_from_the_bar(cx: &mut TestAppContext) {
    let h = start(cx, Setup::default());
    // Tall inspect JSON, so the editor has something to page through.
    let summary = crate::demo::containers()
        .into_iter()
        .find(|c| c.name == "redis")
        .expect("redis");
    let mut d = dk_core::fake::fixtures::details_for(summary);
    d.raw = serde_json::json!({ "Items": (0..120).collect::<Vec<_>>() });
    h.engine.set_container_details(d);
    // A short window, so the charts column and the editor overflow.
    cx.update_window(h.any_window(), |_, window, _| {
        window.resize(gpui_kit::size(gpui_kit::px(1100.), gpui_kit::px(420.)));
    })
    .unwrap();
    let page = open_detail(&h, cx, "redis", ContainerTab::Stats);
    h.draw(cx);
    h.draw(cx);
    let bar = bar_handle(&page, cx);
    h.focus(cx, &bar);
    let stats = cx.read(|cx| page.read(cx).tabs().stats.clone()).unwrap();
    let y = |cx: &mut TestAppContext| cx.read(|cx| stats.read(cx).scroll_offset_y());
    assert_eq!(y(cx), 0.0);
    h.press(cx, "pagedown");
    h.draw(cx);
    let down = y(cx);
    assert!(down < 0.0, "PgDn scrolled the charts column, got {down}");
    h.press(cx, "pageup");
    h.draw(cx);
    assert!(y(cx) > down, "PgUp scrolled back");

    select_tab(&h, cx, ContainerTab::Inspect);
    let inspect = cx.read(|cx| page.read(cx).tabs().inspect.clone()).unwrap();
    h.wait_until(cx, "json formatted", |_, cx| {
        !inspect.read(cx).view().read(cx).text().is_empty()
    });
    h.draw(cx);
    h.draw(cx);
    h.focus(cx, &bar);
    let editor = cx.read(|cx| inspect.read(cx).view().read(cx).editor().clone());
    let ey = |cx: &mut TestAppContext| cx.read(|cx| editor.read(cx).scroll_offset().y.as_f32());
    let before = ey(cx);
    h.press(cx, "pagedown");
    h.draw(cx);
    h.draw(cx);
    assert!(
        ey(cx) < before,
        "PgDn scrolled the editor ({before} → {})",
        ey(cx)
    );
    h.shutdown();
}

#[gpui_kit::test]
fn kbd_095_resource_detail_down_enters_used_by_and_esc_returns(cx: &mut TestAppContext) {
    use crate::nav::ImageTab;
    use crate::pages::image_detail::ImageDetailPage;
    let h = start(cx, Setup::default());
    // Enough containers on one image that the Used by list overflows a short window.
    let image = crate::demo::image_id("redis:7");
    let many: Vec<_> = (0..60)
        .map(|i| {
            let mut c = dk_core::fake::fixtures::container(
                &format!("user-{i:02}"),
                ContainerState::Running,
            );
            c.image_id = image.clone();
            c
        })
        .collect();
    h.engine.set_containers(many);
    cx.update_window(h.any_window(), |_, window, _| {
        window.resize(gpui_kit::size(gpui_kit::px(1100.), gpui_kit::px(420.)));
    })
    .unwrap();
    h.wait_containers(cx);
    let detail = h.goto::<ImageDetailPage>(
        cx,
        Route::ImageDetail {
            id: image,
            tab: ImageTab::UsedBy,
        },
    );
    h.wait_until(cx, "details", |_, cx| {
        detail.read(cx).details().data().is_some()
    });
    h.draw(cx);
    h.draw(cx);
    let bar = h
        .read(cx, |_, window, cx| window.focused(cx))
        .expect("focus");
    let list = cx.read(|cx| detail.read(cx).used_by_focus().clone());
    assert!(!h.is_focused(cx, &list));
    // ↓ enters the Used by list itself.
    h.press(cx, "down");
    assert!(h.is_focused(cx, &list), "↓ focuses the Used by list");
    // The list is a body panel: Backspace goes back from it too (KBD-098).
    // Esc returns to the tab bar.
    h.press(cx, "escape");
    assert_eq!(
        h.read(cx, |_, window, cx| window.focused(cx))
            .expect("focus"),
        bar,
        "Esc returns to the tab bar"
    );
    // PgDn / PgUp from the bar scroll the body without moving focus.
    h.press(cx, "pagedown");
    h.draw(cx);
    let down = cx.read(|cx| detail.read(cx).body_scroll_y());
    assert!(down < 0.0, "PgDn scrolled the page body, got {down}");
    h.press(cx, "pageup");
    h.draw(cx);
    assert!(cx.read(|cx| detail.read(cx).body_scroll_y()) > down);
    assert_eq!(
        h.read(cx, |_, window, cx| window.focused(cx))
            .expect("focus"),
        bar
    );
    // Backspace from inside the list.
    h.press(cx, "down");
    assert!(h.is_focused(cx, &list));
    h.press(cx, "backspace");
    assert_ne!(
        h.read(cx, |s, _, _| s.route().clone()),
        Route::ImageDetail {
            id: crate::demo::image_id("redis:7"),
            tab: ImageTab::UsedBy,
        },
        "Backspace goes back from the Used by list"
    );
    h.shutdown();
}

// ── review follow-ups ─────────────────────────────────────────────────────────────────────

#[gpui_kit::test]
fn shl_014_parked_volumes_make_no_disk_usage_calls(cx: &mut TestAppContext) {
    let h = start(cx, Setup::default());
    h.wait_containers(cx);
    let volumes = h.goto::<VolumesPage>(cx, Route::Volumes);
    h.wait_until(cx, "sizes known", |_, cx| {
        matches!(
            volumes.read(cx).usage(),
            crate::pages::volumes::UsageState::Known(_)
        )
    });
    h.update(cx, |_, window, cx| {
        window.dispatch_action(
            Box::new(crate::actions::Navigate {
                route: Route::Containers,
            }),
            cx,
        )
    });
    h.draw(cx);
    h.engine.clear_calls();
    // Volume changes while the page is parked must not trigger the slow call.
    let store = h.shell.read_with(cx, |s, _| s.store().cloned()).unwrap();
    cx.update(|cx| {
        store.update(cx, |s, cx| {
            s.refetch(crate::state::Collection::Volumes, cx);
            s.refetch(crate::state::Collection::Volumes, cx);
        })
    });
    h.wait_until(cx, "refetched", |_, _| {
        !h.engine.calls_to("list_volumes").is_empty()
    });
    h.draw(cx);
    assert!(
        h.engine.calls_to("disk_usage").is_empty(),
        "a parked Volumes page doesn't call disk_usage"
    );
    // Coming back refreshes the sizes once.
    h.press(cx, "secondary-3");
    h.wait_until(cx, "sizes refreshed", |_, _| {
        !h.engine.calls_to("disk_usage").is_empty()
    });
    h.shutdown();
}

#[gpui_kit::test]
fn kbd_094_enter_survives_a_refresh_during_the_background_build(cx: &mut TestAppContext) {
    let mut containers: Vec<_> = (0..2_100)
        .map(|i| dk_core::fake::fixtures::container(&format!("bulk-{i}"), ContainerState::Running))
        .collect();
    containers.push(dk_core::fake::fixtures::container(
        "needle",
        ContainerState::Running,
    ));
    let needle = containers.last().map(|c| c.id.clone()).unwrap();
    let h = start(
        cx,
        Setup {
            containers,
            ..Default::default()
        },
    );
    h.wait_until(cx, "bulk rows", |_, cx| {
        h.shell
            .read(cx)
            .containers_page()
            .is_some_and(|p| p.read(cx).table().read(cx).model(cx).rows().len() > 2_000)
    });
    h.press(cx, "secondary-f");
    h.type_text(cx, "needle");
    h.press(cx, "enter");
    // A store refresh lands before the build does: it supersedes the build.
    let store = h.shell.read_with(cx, |s, _| s.store().cloned()).unwrap();
    cx.update(|cx| {
        store.update(cx, |s, cx| {
            s.refetch(crate::state::Collection::Containers, cx)
        })
    });
    h.wait_until(cx, "hand-off still happens", |window, cx| {
        let Some(p) = h.shell.read(cx).containers_page().cloned() else {
            return false;
        };
        let t = p.read(cx).table().clone();
        t.read(cx).model(cx).rows().len() == 1
            && t.focus_handle(cx).is_focused(window)
            && t.read(cx).model(cx).cursor().map(|k| k.to_string()) == Some(needle.clone())
    });
    h.shutdown();
}

#[gpui_kit::test]
fn kbd_020_palette_fits_at_high_zoom(cx: &mut TestAppContext) {
    let mut setup = Setup::default();
    setup.config.general.ui_scale = 1.6;
    let h = start(cx, setup);
    h.wait_containers(cx);
    h.press(cx, "secondary-p");
    h.draw(cx);
    let mut visual = gpui_kit::VisualTestContext::from_window(h.any_window(), cx);
    let panel = visual.debug_bounds("overlay-panel").expect("panel");
    let body = visual.debug_bounds("overlay-body").expect("body");
    assert!(
        (panel.bottom() - body.bottom()).as_f32() >= 12.0,
        "the list keeps the popover's bottom padding at 160% zoom: body {:?} panel {:?}",
        body,
        panel
    );
    h.shutdown();
}
