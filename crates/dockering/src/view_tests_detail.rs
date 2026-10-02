//! Container detail view tests (CDT-*, LOG-*, TRM-*, STA-*, KBD-040…070). Real `HubHandle`
//! + `FakeEngine` (no Docker); test names carry requirement ids.

use dk_core::{ContainerState, EngineError, ResourceKind};
use gpui_kit::{Entity, TestAppContext};

use crate::nav::{ContainerTab, Route};
use crate::pages::container_detail::ContainerDetailPage;
use crate::shell::ShellPage;
use crate::testing::{Harness, Setup, id_of, start};

/// The mounted container detail page.
pub fn detail_page(h: &Harness, cx: &mut TestAppContext) -> Option<Entity<ContainerDetailPage>> {
    h.read(cx, |s, _, _| match s.page() {
        ShellPage::ContainerDetail(p) => Some(p.clone()),
        _ => None,
    })
}

/// Navigates to `name`'s detail on `tab` and waits until inspect data arrived.
pub fn open_detail(
    h: &Harness,
    cx: &mut TestAppContext,
    name: &str,
    tab: ContainerTab,
) -> Entity<ContainerDetailPage> {
    h.wait_containers(cx);
    let id = id_of(name);
    h.update(cx, |_, window, cx| {
        window.dispatch_action(
            Box::new(crate::actions::Navigate {
                route: Route::ContainerDetail { id, tab },
            }),
            cx,
        )
    });
    h.wait_until(cx, "detail data", |_, cx| match h.shell.read(cx).page() {
        ShellPage::ContainerDetail(p) => p
            .read(cx)
            .detail_state()
            .is_some_and(|s| s.read(cx).details.data().is_some()),
        _ => false,
    });
    detail_page(h, cx).expect("detail page")
}

fn route(h: &Harness, cx: &mut TestAppContext) -> Route {
    h.read(cx, |s, _, _| s.route().clone())
}

fn focus_tab_bar(h: &Harness, page: &Entity<ContainerDetailPage>, cx: &mut TestAppContext) {
    let f = cx.read(|cx| page.read(cx).tab_bar_focus().clone());
    h.focus(cx, &f);
}

// ── frame (CDT-001…003, CDT-080/081, KBD-040…042) ────────────────────────────────────────

#[gpui_kit::test]
fn cdt_002_tab_in_route_and_back_forward_keep_tab(cx: &mut TestAppContext) {
    let h = start(cx, Setup::default());
    let page = open_detail(&h, cx, "redis", ContainerTab::Overview);
    let id = id_of("redis");
    // Navigation focuses the tab bar (KBD-007 primary focus).
    h.wait_until(cx, "tab bar focused", |window, cx| {
        page.read(cx).tab_bar_focus().is_focused(window)
    });
    // Ctrl+Tab switches tabs everywhere; the tab is part of the route (replace, no push).
    h.press(cx, "ctrl-tab");
    assert_eq!(
        route(&h, cx),
        Route::ContainerDetail {
            id: id.clone(),
            tab: ContainerTab::Logs
        }
    );
    // Arrows move between tabs while the tab bar is focused (KBD-040).
    h.press(cx, "right");
    assert_eq!(cx.read(|cx| page.read(cx).tab()), ContainerTab::Terminal);
    h.press(cx, "left left");
    assert_eq!(cx.read(|cx| page.read(cx).tab()), ContainerTab::Overview);
    h.press(cx, "ctrl-shift-tab");
    assert_eq!(cx.read(|cx| page.read(cx).tab()), ContainerTab::Inspect);
    // The same page entity serves every tab (CDT-081).
    let same = detail_page(&h, cx).is_some_and(|p| p == page);
    assert!(same, "tab switches keep the page entity");
    // Back leaves the detail page (tab switches don't add history) and Forward restores
    // the tab that was showing.
    h.press(cx, "alt-left");
    assert_eq!(route(&h, cx), Route::Containers);
    h.press(cx, "alt-right");
    assert_eq!(
        route(&h, cx),
        Route::ContainerDetail {
            id,
            tab: ContainerTab::Inspect
        }
    );
    let tab = detail_page(&h, cx).map(|p| cx.read(|cx| p.read(cx).tab()));
    assert_eq!(tab, Some(ContainerTab::Inspect));
    h.shutdown();
}

#[gpui_kit::test]
fn cdt_081_tab_state_survives_switching(cx: &mut TestAppContext) {
    let h = start(cx, Setup::default());
    let page = open_detail(&h, cx, "redis", ContainerTab::Overview);
    let overview = cx
        .read(|cx| page.read(cx).tabs().overview.clone())
        .expect("overview tab entity");
    // Move the row cursor, switch away and back: the cursor is where it was.
    let f = cx.read(|cx| overview.read(cx).rows().focus.clone());
    h.focus(cx, &f);
    h.press(cx, "down down");
    let cursor = cx.read(|cx| overview.read(cx).rows().cursor().cloned());
    assert_eq!(cursor.as_deref(), Some("image"));
    focus_tab_bar(&h, &page, cx);
    h.press(cx, "right");
    h.press(cx, "left");
    let again = cx.read(|cx| page.read(cx).tabs().overview.clone()).unwrap();
    assert_eq!(again, overview, "same tab entity");
    let cursor = cx.read(|cx| overview.read(cx).rows().cursor().cloned());
    assert_eq!(cursor.as_deref(), Some("image"));
    h.shutdown();
}

#[gpui_kit::test]
fn cdt_003_refreshes_on_engine_event(cx: &mut TestAppContext) {
    let h = start(cx, Setup::default());
    let _page = open_detail(&h, cx, "scratchpad", ContainerTab::Overview);
    let id = id_of("scratchpad");
    h.wait_until(cx, "subscribed", |_, _| h.engine.open_streams().0 > 0);
    h.engine.clear_calls();
    let mut cs = crate::demo::containers();
    if let Some(c) = cs.iter_mut().find(|c| c.id == id) {
        c.state = ContainerState::Running;
        c.status_text = "Up 1 second".into();
    }
    h.engine.set_containers(cs);
    h.engine.emit_event(dk_core::fake::fixtures::event(
        ResourceKind::Container,
        "start",
        &id,
    ));
    h.wait_until(cx, "inspect refetched", |_, cx| detail_running(&h, cx));
    assert!(!h.engine.calls_to("inspect_container").is_empty());
    h.shutdown();
}

fn detail_running(h: &Harness, cx: &gpui_kit::App) -> bool {
    match h.shell.read(cx).page() {
        ShellPage::ContainerDetail(p) => p.read(cx).detail_state().is_some_and(|s| {
            s.read(cx)
                .details
                .data()
                .is_some_and(|d| d.summary.state == ContainerState::Running)
        }),
        _ => false,
    }
}

#[gpui_kit::test]
fn cdt_041_header_letters_start_and_stop(cx: &mut TestAppContext) {
    let h = start(cx, Setup::default());
    let page = open_detail(&h, cx, "scratchpad", ContainerTab::Overview);
    focus_tab_bar(&h, &page, cx);
    h.engine.clear_calls();
    h.press(cx, "s");
    h.wait_until(cx, "start called", |_, _| {
        !h.engine.calls_to("start").is_empty()
    });
    assert_eq!(
        h.engine
            .calls_to("start")
            .into_iter()
            .map(|c| c.arg)
            .collect::<Vec<_>>(),
        vec![id_of("scratchpad")]
    );
    h.wait_until(cx, "running after refresh", |_, cx| detail_running(&h, cx));
    h.press(cx, "s");
    h.wait_until(cx, "stop called", |_, _| {
        !h.engine.calls_to("stop").is_empty()
    });
    h.shutdown();
}

#[gpui_kit::test]
fn cdt_001_delete_running_confirms_with_force(cx: &mut TestAppContext) {
    let h = start(cx, Setup::default());
    let page = open_detail(&h, cx, "redis", ContainerTab::Overview);
    focus_tab_bar(&h, &page, cx);
    h.press(cx, "delete");
    assert!(h.has_dialog(cx), "destructive actions confirm (SHL-002)");
    h.draw(cx);
    h.press(cx, "ctrl-enter");
    h.wait_until(cx, "removed + back to list", |_, cx| {
        *h.shell.read(cx).route() == Route::Containers
    });
    assert_eq!(h.engine.calls_to("remove_container").len(), 1);
    h.shutdown();
}

#[gpui_kit::test]
fn cdt_080_removed_container_banner_and_read_only(cx: &mut TestAppContext) {
    let h = start(cx, Setup::default());
    let page = open_detail(&h, cx, "scratchpad", ContainerTab::Overview);
    let id = id_of("scratchpad");
    h.wait_until(cx, "subscribed", |_, _| h.engine.open_streams().0 > 0);
    // Removed behind our back (another client): destroy event.
    let mut cs = crate::demo::containers();
    cs.retain(|c| c.id != id);
    h.engine.set_containers(cs);
    h.engine.emit_event(dk_core::fake::fixtures::event(
        ResourceKind::Container,
        "destroy",
        &id,
    ));
    h.wait_until(cx, "removed", |_, cx| page.read(cx).is_removed(cx));
    // Data stays visible; actions are disabled (S does nothing).
    let has_data = cx.read(|cx| {
        page.read(cx)
            .detail_state()
            .is_some_and(|s| s.read(cx).details.data().is_some())
    });
    assert!(has_data, "last data stays visible read-only");
    focus_tab_bar(&h, &page, cx);
    h.engine.clear_calls();
    h.press(cx, "s");
    h.draw(cx);
    assert!(h.engine.calls_to("start").is_empty(), "actions disabled");
    h.press(cx, "delete");
    assert!(!h.has_dialog(cx));
    h.shutdown();
}

#[gpui_kit::test]
fn cdt_080_removed_detected_from_list_refetch(cx: &mut TestAppContext) {
    // Engines without events: the list refetch (polling) reveals the removal.
    let h = start(cx, Setup::default());
    let page = open_detail(&h, cx, "scratchpad", ContainerTab::Overview);
    let id = id_of("scratchpad");
    let mut cs = crate::demo::containers();
    cs.retain(|c| c.id != id);
    h.engine.set_containers(cs);
    h.update(cx, |_, window, cx| {
        window.dispatch_action(Box::new(crate::actions::Refresh), cx)
    });
    h.wait_until(cx, "removed", |_, cx| page.read(cx).is_removed(cx));
    h.shutdown();
}

#[gpui_kit::test]
fn cdt_states_loading_and_error_with_retry(cx: &mut TestAppContext) {
    let h = start(cx, Setup::default());
    h.wait_containers(cx);
    h.engine.set_error(
        "inspect_container",
        Some(EngineError::Api {
            status: 500,
            message: "boom".into(),
        }),
    );
    let id = id_of("redis");
    h.update(cx, |_, window, cx| {
        window.dispatch_action(
            Box::new(crate::actions::Navigate {
                route: Route::ContainerDetail {
                    id,
                    tab: ContainerTab::Overview,
                },
            }),
            cx,
        )
    });
    // Loading first (no data), then the error state.
    let page = {
        h.wait_until(cx, "page", |_, cx| {
            matches!(h.shell.read(cx).page(), ShellPage::ContainerDetail(_))
        });
        detail_page(&h, cx).unwrap()
    };
    h.wait_until(cx, "error", |_, cx| {
        page.read(cx)
            .detail_state()
            .is_some_and(|s| s.read(cx).details.error().is_some())
    });
    // Retry (F5) after the error clears loads data.
    h.engine.set_error("inspect_container", None);
    focus_tab_bar(&h, &page, cx);
    h.press(cx, "f5");
    h.wait_until(cx, "data", |_, cx| {
        page.read(cx)
            .detail_state()
            .is_some_and(|s| s.read(cx).details.data().is_some())
    });
    h.shutdown();
}

#[gpui_kit::test]
fn kbd_042_alt_up_focuses_row_in_parent_list(cx: &mut TestAppContext) {
    let h = start(cx, Setup::default());
    // A compose member: its (collapsed) group expands so the row can take the cursor.
    let page = open_detail(&h, cx, "myshop-db-1", ContainerTab::Overview);
    focus_tab_bar(&h, &page, cx);
    h.press(cx, "alt-up");
    assert_eq!(route(&h, cx), Route::Containers);
    h.wait_until(cx, "cursor on row", |_, _| true);
    assert_eq!(h.cursor_key(cx), Some(id_of("myshop-db-1")));
    h.shutdown();
}

// ── Overview / Mounts / Network / Inspect (CDT-010…040, KBD-043/044) ─────────────────────

fn details_with(
    name: &str,
    f: impl FnOnce(&mut dk_core::ContainerDetails),
) -> dk_core::ContainerDetails {
    let summary = crate::demo::containers()
        .into_iter()
        .find(|c| c.name == name)
        .expect("demo container");
    let mut d = dk_core::fake::fixtures::details_for(summary);
    f(&mut d);
    d
}

#[gpui_kit::test]
fn cdt_010_overview_masks_secrets_and_reveal_works(cx: &mut TestAppContext) {
    let h = start(cx, Setup::default());
    let page = open_detail(&h, cx, "redis", ContainerTab::Overview);
    let overview = cx.read(|cx| page.read(cx).tabs().overview.clone()).unwrap();
    // fixtures: PATH (plain) + DB_PASSWORD (sensitive).
    let shown =
        |cx: &mut TestAppContext, key: &str| cx.read(|cx| overview.read(cx).env_display(key, cx));
    assert_eq!(shown(cx, "PATH").as_deref(), Some("/usr/bin"));
    assert_eq!(
        shown(cx, "DB_PASSWORD").as_deref(),
        Some(crate::strings::MASKED_VALUE)
    );
    // Keyboard: focus the panel, move to the secret row, Space reveals it.
    let f = cx.read(|cx| overview.read(cx).rows().focus.clone());
    h.focus(cx, &f);
    overview.update(cx, |o, _| o.rows_mut().set_cursor("env:DB_PASSWORD"));
    h.press(cx, "space");
    assert_eq!(shown(cx, "DB_PASSWORD").as_deref(), Some("secret"));
    h.press(cx, "space");
    assert_eq!(
        shown(cx, "DB_PASSWORD").as_deref(),
        Some(crate::strings::MASKED_VALUE)
    );
    // Mod+C copies the focused value (KBD-044), even while masked (explicit action).
    h.press(cx, "ctrl-c");
    let copied = cx.read_from_clipboard().and_then(|c| c.text());
    assert_eq!(copied.as_deref(), Some("secret"));
    // Single letters still work on the non-input panel (KBD-041): C copies the id.
    h.press(cx, "c");
    let copied = cx.read_from_clipboard().and_then(|c| c.text());
    assert_eq!(copied, Some(id_of("redis")));
    h.shutdown();
}

#[gpui_kit::test]
fn kbd_044_rows_arrows_and_enter_follows_image_link(cx: &mut TestAppContext) {
    let h = start(cx, Setup::default());
    let page = open_detail(&h, cx, "redis", ContainerTab::Overview);
    let overview = cx.read(|cx| page.read(cx).tabs().overview.clone()).unwrap();
    let f = cx.read(|cx| overview.read(cx).rows().focus.clone());
    h.focus(cx, &f);
    h.press(cx, "end");
    let last = cx.read(|cx| overview.read(cx).rows().cursor().cloned());
    assert_eq!(last.as_deref(), Some("res:pids"));
    h.press(cx, "home down down");
    h.press(cx, "enter");
    let image_id = crate::demo::containers()
        .into_iter()
        .find(|c| c.name == "redis")
        .unwrap()
        .image_id;
    h.wait_until(cx, "image detail", |_, cx| {
        matches!(h.shell.read(cx).route(), Route::ImageDetail { id, .. } if *id == image_id)
    });
    h.shutdown();
}

#[gpui_kit::test]
fn cdt_020_mounts_volume_link_opens_volume_detail(cx: &mut TestAppContext) {
    let h = start(cx, Setup::default());
    h.engine.set_container_details(details_with("redis", |d| {
        d.mounts = vec![
            dk_core::MountDetail {
                kind: dk_core::MountKind::Bind,
                source: "/home/dev/conf".into(),
                destination: "/etc/redis".into(),
                mode: "ro".into(),
                rw: false,
                propagation: None,
                volume_name: None,
            },
            dk_core::MountDetail {
                kind: dk_core::MountKind::Volume,
                source: "/var/lib/docker/volumes/scratch/_data".into(),
                destination: "/data".into(),
                mode: String::new(),
                rw: true,
                propagation: None,
                volume_name: Some("scratch".into()),
            },
        ];
    }));
    let page = open_detail(&h, cx, "redis", ContainerTab::Mounts);
    let mounts = cx.read(|cx| page.read(cx).tabs().mounts.clone()).unwrap();
    h.draw(cx);
    let keys = cx.read(|cx| mounts.read(cx).rows().keys().to_vec());
    assert_eq!(keys.len(), 2);
    let f = cx.read(|cx| mounts.read(cx).rows().focus.clone());
    h.focus(cx, &f);
    h.press(cx, "down enter");
    h.wait_until(cx, "volume detail", |_, cx| {
        matches!(h.shell.read(cx).route(), Route::VolumeDetail { name, .. } if name == "scratch")
    });
    h.shutdown();
}

#[gpui_kit::test]
fn cdt_030_network_link_and_port_link(cx: &mut TestAppContext) {
    let h = start(cx, Setup::default());
    let page = open_detail(&h, cx, "myshop-web-1", ContainerTab::Network);
    let net = cx.read(|cx| page.read(cx).tabs().network.clone()).unwrap();
    h.draw(cx);
    let keys = cx.read(|cx| net.read(cx).rows().keys().to_vec());
    assert!(keys.iter().any(|k| k == "port:0"), "{keys:?}");
    assert!(keys.iter().any(|k| k == "net:bridge"), "{keys:?}");
    let f = cx.read(|cx| net.read(cx).rows().focus.clone());
    h.focus(cx, &f);
    // Row 0 is the port 8080:80 (Mod+C copies its URL), row 1 the bridge network.
    h.press(cx, "home ctrl-c");
    let copied = cx.read_from_clipboard().and_then(|c| c.text());
    assert_eq!(copied.as_deref(), Some("http://localhost:8080"));
    h.press(cx, "down enter");
    h.wait_until(cx, "network detail", |_, cx| {
        matches!(h.shell.read(cx).route(), Route::NetworkDetail { id, .. } if id == "bridge")
    });
    h.shutdown();
}

#[gpui_kit::test]
fn cdt_040_inspect_shows_pretty_json_unmasked(cx: &mut TestAppContext) {
    let h = start(cx, Setup::default());
    let page = open_detail(&h, cx, "redis", ContainerTab::Inspect);
    let inspect = cx.read(|cx| page.read(cx).tabs().inspect.clone()).unwrap();
    h.wait_until(cx, "json formatted", |_, cx| {
        !inspect.read(cx).view().read(cx).text().is_empty()
    });
    let text = cx.read(|cx| inspect.read(cx).view().read(cx).text().to_string());
    assert!(text.contains("\n  \"Id\""), "pretty-printed: {text}");
    assert!(text.contains("DB_PASSWORD=secret"), "not masked (CDT-040)");
    // Mod+F from the page opens the in-editor search (KBD-025).
    focus_tab_bar(&h, &page, cx);
    h.press(cx, "ctrl-f");
    let open = cx.read(|cx| {
        inspect
            .read(cx)
            .view()
            .read(cx)
            .editor()
            .read(cx)
            .search_session()
            .open
    });
    assert!(open);
    h.shutdown();
}
