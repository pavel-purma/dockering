//! Container detail view tests (CDT-*, LOG-*, TRM-*, STA-*, KBD-040…070). Real `HubHandle`
//! + `FakeEngine` (no Docker); test names carry requirement ids.

use dk_core::{ContainerState, EngineError, ResourceKind};
use gpui_kit::{AppContext as _, Entity, TestAppContext};

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
    h.wait_until(cx, "containers loaded", |_, cx| {
        h.shell
            .read(cx)
            .store()
            .is_some_and(|s| s.read(cx).containers.data().is_some_and(|d| !d.is_empty()))
    });
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

// ── Logs (LOG-001…009, KBD-050…053) ───────────────────────────────────────────────────────

use crate::pages::container_detail::logs::LogsView;

fn logs_view(page: &Entity<ContainerDetailPage>, cx: &mut TestAppContext) -> Entity<LogsView> {
    cx.read(|cx| page.read(cx).tabs().logs.clone())
        .expect("logs tab entity")
}

fn push(h: &Harness, name: &str, stream: dk_core::LogStream, text: &str) {
    h.engine.push_log(
        &id_of(name),
        dk_core::fake::fixtures::log_line(stream, text),
    );
}

fn wait_lines(h: &Harness, cx: &mut TestAppContext, logs: &Entity<LogsView>, n: usize) {
    h.wait_until(cx, "log lines", |_, cx| logs.read(cx).lines().len() >= n);
}

fn texts(logs: &Entity<LogsView>, cx: &mut TestAppContext) -> Vec<String> {
    cx.read(|cx| {
        logs.read(cx)
            .lines()
            .iter()
            .map(|l| l.text.to_string())
            .collect()
    })
}

fn wait_log_stream(h: &Harness, cx: &mut TestAppContext) {
    h.wait_until(cx, "log stream subscribed", |_, _| {
        h.engine.open_streams().1 > 0
    });
}

#[gpui_kit::test]
fn log_001_initial_tail_then_follow(cx: &mut TestAppContext) {
    let h = start(cx, Setup::default());
    let redis = id_of("redis");
    h.engine.set_logs(
        &redis,
        (0..5)
            .map(|i| {
                dk_core::fake::fixtures::log_line(dk_core::LogStream::Stdout, &format!("old {i}"))
            })
            .collect(),
    );
    let page = open_detail(&h, cx, "redis", ContainerTab::Logs);
    let logs = logs_view(&page, cx);
    wait_lines(&h, cx, &logs, 5);
    wait_log_stream(&h, cx);
    push(&h, "redis", dk_core::LogStream::Stdout, "live");
    wait_lines(&h, cx, &logs, 6);
    assert_eq!(texts(&logs, cx).last().map(String::as_str), Some("live"));
    assert_eq!(h.engine.calls_to("logs").len(), 1, "one subscription");
    h.shutdown();
}

#[gpui_kit::test]
fn log_007_batching_many_pushes_few_notifies(cx: &mut TestAppContext) {
    let h = start(cx, Setup::default());
    let page = open_detail(&h, cx, "redis", ContainerTab::Logs);
    let logs = logs_view(&page, cx);
    wait_log_stream(&h, cx);
    for i in 0..1200 {
        push(
            &h,
            "redis",
            dk_core::LogStream::Stdout,
            &format!("line {i}"),
        );
    }
    wait_lines(&h, cx, &logs, 1200);
    let flushes = cx.read(|cx| logs.read(cx).flushes());
    assert!(flushes <= 30, "1200 lines in {flushes} batches (LOG-007)");
    let t = texts(&logs, cx);
    assert_eq!(t.first().map(String::as_str), Some("line 0"));
    assert_eq!(t.last().map(String::as_str), Some("line 1199"));
    h.shutdown();
}

#[gpui_kit::test]
fn log_002_ansi_colours_and_stderr(cx: &mut TestAppContext) {
    let h = start(cx, Setup::default());
    let page = open_detail(&h, cx, "redis", ContainerTab::Logs);
    let logs = logs_view(&page, cx);
    wait_log_stream(&h, cx);
    push(
        &h,
        "redis",
        dk_core::LogStream::Stdout,
        "\x1b[1;32mOK\x1b[0m done \x1b]0;title\x07",
    );
    push(&h, "redis", dk_core::LogStream::Stderr, "warning!");
    wait_lines(&h, cx, &logs, 2);
    let (text, spans, stream) = cx.read(|cx| {
        let l = logs.read(cx).lines();
        (l[0].text.to_string(), l[0].spans.clone(), l[1].stream)
    });
    assert_eq!(text, "OK done ", "SGR kept, OSC stripped");
    assert_eq!(spans.len(), 1);
    assert_eq!(spans[0].0, 0..2);
    assert!(spans[0].1.bold);
    assert_eq!(spans[0].1.fg, Some(dk_core::ansi::AnsiColor::Indexed(2)));
    assert_eq!(stream, dk_core::LogStream::Stderr, "stderr is tinted");
    h.shutdown();
}

#[gpui_kit::test]
fn log_005_ring_cap_with_dropped_indicator(cx: &mut TestAppContext) {
    let h = start(cx, Setup::default());
    let page = open_detail(&h, cx, "redis", ContainerTab::Logs);
    let logs = logs_view(&page, cx);
    logs.update(cx, |l, _| l.set_max_lines(100));
    wait_log_stream(&h, cx);
    for i in 0..250 {
        push(&h, "redis", dk_core::LogStream::Stdout, &format!("n{i}"));
    }
    h.wait_until(cx, "all appended", |_, cx| {
        logs.read(cx)
            .lines()
            .back()
            .is_some_and(|l| l.text.as_ref() == "n249")
    });
    let (len, dropped, first) = cx.read(|cx| {
        let l = logs.read(cx);
        (l.lines().len(), l.dropped(), l.lines()[0].text.to_string())
    });
    assert_eq!(len, 100);
    assert_eq!(dropped, 150);
    assert_eq!(first, "n150");
    h.shutdown();
}

#[gpui_kit::test]
fn log_003_follow_pill(cx: &mut TestAppContext) {
    let h = start(cx, Setup::default());
    let page = open_detail(&h, cx, "redis", ContainerTab::Logs);
    let logs = logs_view(&page, cx);
    wait_log_stream(&h, cx);
    for i in 0..200 {
        push(&h, "redis", dk_core::LogStream::Stdout, &format!("a{i}"));
    }
    wait_lines(&h, cx, &logs, 200);
    assert!(
        cx.read(|cx| logs.read(cx).is_following()),
        "tail is on by default"
    );
    // Scrolling up pauses following (keyboard: focus the list, PgUp).
    let f = cx.read(|cx| logs.read(cx).list_focus().clone());
    h.focus(cx, &f);
    h.press(cx, "pageup");
    assert!(!cx.read(|cx| logs.read(cx).is_following()));
    for i in 0..3 {
        push(&h, "redis", dk_core::LogStream::Stdout, &format!("b{i}"));
    }
    wait_lines(&h, cx, &logs, 203);
    assert_eq!(
        cx.read(|cx| logs.read(cx).unseen()),
        3,
        "Jump to bottom (3 new)"
    );
    // End jumps to the bottom and re-enables follow (KBD-051).
    h.press(cx, "end");
    assert!(cx.read(|cx| logs.read(cx).is_following()));
    assert_eq!(cx.read(|cx| logs.read(cx).unseen()), 0);
    h.shutdown();
}

#[gpui_kit::test]
fn log_004_search_next_prev(cx: &mut TestAppContext) {
    let h = start(cx, Setup::default());
    let page = open_detail(&h, cx, "redis", ContainerTab::Logs);
    let logs = logs_view(&page, cx);
    wait_log_stream(&h, cx);
    for t in ["alpha", "beta", "Alpha two", "gamma"] {
        push(&h, "redis", dk_core::LogStream::Stdout, t);
    }
    wait_lines(&h, cx, &logs, 4);
    focus_tab_bar(&h, &page, cx);
    h.press(cx, "ctrl-f");
    let search_focused = cx
        .update_window(h.any_window(), |_, window, cx| {
            gpui_kit::Focusable::focus_handle(logs.read(cx).search_input(), cx).is_focused(window)
        })
        .unwrap();
    assert!(search_focused, "Mod+F focuses the logs search (KBD-050)");
    h.type_text(cx, "alpha");
    h.draw(cx);
    let state = |cx: &mut TestAppContext| {
        cx.read(|cx| {
            let l = logs.read(cx);
            (l.matches().to_vec(), l.current_match())
        })
    };
    assert_eq!(
        state(cx),
        (vec![0, 2], Some(1)),
        "case-insensitive, newest first"
    );
    h.press(cx, "enter");
    assert_eq!(state(cx).1, Some(0), "Enter = next (wraps)");
    h.press(cx, "shift-enter");
    assert_eq!(state(cx).1, Some(1), "Shift+Enter = previous");
    h.press(cx, "f3");
    assert_eq!(state(cx).1, Some(0), "F3 = next");
    // Typing in the search never triggers single-letter actions (KBD-008).
    h.engine.clear_calls();
    h.type_text(cx, "s");
    h.draw(cx);
    assert!(h.engine.calls_to("stop").is_empty());
    h.shutdown();
}

#[gpui_kit::test]
fn log_004_toggles_clear_and_copy_all(cx: &mut TestAppContext) {
    let h = start(cx, Setup::default());
    let page = open_detail(&h, cx, "redis", ContainerTab::Logs);
    let logs = logs_view(&page, cx);
    wait_log_stream(&h, cx);
    push(&h, "redis", dk_core::LogStream::Stdout, "one");
    push(&h, "redis", dk_core::LogStream::Stdout, "two");
    wait_lines(&h, cx, &logs, 2);
    let f = cx.read(|cx| logs.read(cx).list_focus().clone());
    h.focus(cx, &f);
    let (ts0, wrap0) = cx.read(|cx| (logs.read(cx).timestamps(), logs.read(cx).wrap()));
    h.press(cx, "alt-t alt-w");
    let (ts1, wrap1) = cx.read(|cx| (logs.read(cx).timestamps(), logs.read(cx).wrap()));
    assert_eq!((ts1, wrap1), (!ts0, !wrap0), "Alt+T / Alt+W (KBD-052)");
    if ts1 {
        h.press(cx, "alt-t");
    }
    h.press(cx, "ctrl-shift-c");
    let copied = cx
        .read_from_clipboard()
        .and_then(|c| c.text())
        .unwrap_or_default();
    assert_eq!(copied, "one\ntwo\n");
    h.press(cx, "ctrl-shift-k");
    assert!(
        cx.read(|cx| logs.read(cx).lines().is_empty()),
        "clear is client-side"
    );
    assert_eq!(h.engine.calls_to("logs").len(), 1, "no re-fetch");
    h.shutdown();
}

#[gpui_kit::test]
fn log_004_save_writes_through_hub(cx: &mut TestAppContext) {
    let h = start(cx, Setup::default());
    let page = open_detail(&h, cx, "redis", ContainerTab::Logs);
    let logs = logs_view(&page, cx);
    wait_log_stream(&h, cx);
    push(&h, "redis", dk_core::LogStream::Stdout, "saved line");
    wait_lines(&h, cx, &logs, 1);
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("redis.log");
    let f = cx.read(|cx| logs.read(cx).list_focus().clone());
    h.focus(cx, &f);
    h.press(cx, "ctrl-s");
    assert!(cx.did_prompt_for_new_path(), "native save dialog");
    let target = path.clone();
    cx.simulate_new_path_selection(move |_| Some(target));
    h.wait_until(cx, "file written", |_, _| path.exists());
    let text = std::fs::read_to_string(&path).expect("saved file"); // nfr-001-allow: test only
    assert!(text.contains("saved line"));
    h.shutdown();
}

#[gpui_kit::test]
fn log_006_exit_footer_then_resume_on_start(cx: &mut TestAppContext) {
    let h = start(cx, Setup::default());
    let page = open_detail(&h, cx, "redis", ContainerTab::Logs);
    let logs = logs_view(&page, cx);
    wait_log_stream(&h, cx);
    push(&h, "redis", dk_core::LogStream::Stdout, "before stop");
    wait_lines(&h, cx, &logs, 1);
    // Stop from the header (S): the engine ends the log stream.
    focus_tab_bar(&h, &page, cx);
    h.press(cx, "s");
    h.wait_until(cx, "exit footer", |_, cx| {
        logs.read(cx).exit_footer(cx).as_deref() == Some("Container exited (code 0)")
    });
    // Start again: following resumes with `since = last ts` (no duplicates).
    h.press(cx, "s");
    h.wait_until(cx, "resubscribed", |_, _| {
        h.engine.calls_to("logs").len() == 2 && h.engine.open_streams().1 > 0
    });
    push(&h, "redis", dk_core::LogStream::Stdout, "after start");
    wait_lines(&h, cx, &logs, 2);
    assert_eq!(texts(&logs, cx), vec!["before stop", "after start"]);
    assert!(cx.read(|cx| logs.read(cx).exit_footer(cx)).is_none());
    h.shutdown();
}

// ── Terminal (TRM-001…012, KBD-060…063) ───────────────────────────────────────────────────

use crate::pages::container_detail::terminal::TerminalTab;

fn terminal_tab(
    page: &Entity<ContainerDetailPage>,
    cx: &mut TestAppContext,
) -> Entity<TerminalTab> {
    cx.read(|cx| page.read(cx).tabs().terminal.clone())
        .expect("terminal tab entity")
}

fn wait_sessions(h: &Harness, cx: &mut TestAppContext, tab: &Entity<TerminalTab>, n: usize) {
    h.wait_until(cx, "terminal sessions connected", |_, cx| {
        let t = tab.read(cx);
        t.sessions().len() == n && t.sessions().iter().all(|s| s.hub_id.is_some())
    });
}

fn grid(tab: &Entity<TerminalTab>, cx: &mut TestAppContext) -> String {
    cx.read(|cx| {
        tab.read(cx)
            .active_view()
            .map(|v| v.read(cx).grid_text())
            .unwrap_or_default()
    })
}

fn leave_terminal(h: &Harness, cx: &mut TestAppContext) {
    h.draw(cx);
    let leave = if cfg!(target_os = "macos") {
        "cmd-shift-f6"
    } else {
        "ctrl-shift-f6"
    };
    h.press(cx, leave);
}

fn terminal_focused(h: &Harness, tab: &Entity<TerminalTab>, cx: &mut TestAppContext) -> bool {
    cx.update_window(h.any_window(), |_, window, cx| {
        tab.read(cx)
            .active_view()
            .is_some_and(|v| gpui_kit::Focusable::focus_handle(v.read(cx), cx).is_focused(window))
    })
    .unwrap()
}

#[gpui_kit::test]
fn trm_001_not_running_shows_start(cx: &mut TestAppContext) {
    let h = start(cx, Setup::default());
    let page = open_detail(&h, cx, "scratchpad", ContainerTab::Terminal);
    let tab = terminal_tab(&page, cx);
    h.draw(cx);
    assert!(cx.read(|cx| tab.read(cx).sessions().is_empty()));
    assert_eq!(h.engine.exec_count(), 0, "no exec on a stopped container");
    // Start (from the header / the tab's Start button) → a session opens.
    focus_tab_bar(&h, &page, cx);
    h.press(cx, "s");
    wait_sessions(&h, cx, &tab, 1);
    assert_eq!(h.engine.exec_count(), 1);
    h.shutdown();
}

#[gpui_kit::test]
fn trm_001_opens_session_and_typing_echoes(cx: &mut TestAppContext) {
    let h = start(cx, Setup::default());
    let page = open_detail(&h, cx, "redis", ContainerTab::Terminal);
    let tab = terminal_tab(&page, cx);
    wait_sessions(&h, cx, &tab, 1);
    h.wait_until(cx, "prompt", |_, cx| {
        tab.read(cx)
            .active_view()
            .is_some_and(|v| v.read(cx).grid_text().contains("fake$"))
    });
    h.wait_until(cx, "terminal focused", |window, cx| {
        tab.read(cx)
            .active_view()
            .is_some_and(|v| gpui_kit::Focusable::focus_handle(v.read(cx), cx).is_focused(window))
    });
    // The loopback FakeTerminal echoes input (TRM-001/003).
    h.type_text(cx, "echo hi");
    h.wait_until(cx, "echo", |_, cx| {
        tab.read(cx)
            .active_view()
            .is_some_and(|v| v.read(cx).grid_text().contains("echo hi"))
    });
    // KBD-060: Tab and single letters go to the shell, not the app.
    h.engine.clear_calls();
    h.type_text(cx, "s");
    h.press(cx, "tab");
    h.draw(cx);
    assert!(
        h.engine.calls_to("stop").is_empty(),
        "S typed into the shell"
    );
    assert!(terminal_focused(&h, &tab, cx), "Tab stays in the terminal");
    assert!(grid(&tab, cx).contains("echo his"));
    h.shutdown();
}

#[gpui_kit::test]
fn kbd_061_leave_terminal_focuses_tab_bar(cx: &mut TestAppContext) {
    let h = start(cx, Setup::default());
    let page = open_detail(&h, cx, "redis", ContainerTab::Terminal);
    let tab = terminal_tab(&page, cx);
    wait_sessions(&h, cx, &tab, 1);
    h.wait_until(cx, "terminal focused", |window, cx| {
        tab.read(cx)
            .active_view()
            .is_some_and(|v| gpui_kit::Focusable::focus_handle(v.read(cx), cx).is_focused(window))
    });
    let leave = if cfg!(target_os = "macos") {
        "cmd-shift-f6"
    } else {
        "ctrl-shift-f6"
    };
    h.press(cx, leave);
    let bar = cx.read(|cx| page.read(cx).tab_bar_focus().clone());
    assert!(h.is_focused(cx, &bar), "Mod+Shift+F6 → detail tab bar");
    // Ctrl+Tab from the terminal switches detail tabs (KBD-061).
    let v = cx.read(|cx| tab.read(cx).active_view().cloned()).unwrap();
    let f = cx.read(|cx| gpui_kit::Focusable::focus_handle(v.read(cx), cx));
    h.focus(cx, &f);
    h.press(cx, "ctrl-tab");
    assert_eq!(cx.read(|cx| page.read(cx).tab()), ContainerTab::Stats);
    h.shutdown();
}

#[gpui_kit::test]
fn trm_007_sub_tabs_new_switch_close(cx: &mut TestAppContext) {
    let h = start(cx, Setup::default());
    let page = open_detail(&h, cx, "redis", ContainerTab::Terminal);
    let tab = terminal_tab(&page, cx);
    wait_sessions(&h, cx, &tab, 1);
    h.wait_until(cx, "terminal focused", |_, cx| {
        tab.read(cx).active_view().is_some()
    });
    h.draw(cx);
    let mod_shift = if cfg!(target_os = "macos") {
        "cmd-shift"
    } else {
        "ctrl-shift"
    };
    h.press(cx, &format!("{mod_shift}-t"));
    wait_sessions(&h, cx, &tab, 2);
    assert_eq!(cx.read(|cx| tab.read(cx).active()), 1);
    assert_eq!(h.engine.exec_count(), 2);
    h.draw(cx);
    h.press(cx, "ctrl-pageup");
    assert_eq!(cx.read(|cx| tab.read(cx).active()), 0);
    h.draw(cx);
    h.press(cx, "ctrl-pagedown");
    assert_eq!(cx.read(|cx| tab.read(cx).active()), 1);
    h.draw(cx);
    h.press(cx, &format!("{mod_shift}-w"));
    h.wait_until(cx, "one session", |_, cx| {
        tab.read(cx).sessions().len() == 1
    });
    h.shutdown();
}

#[gpui_kit::test]
fn trm_006_exit_then_reconnect_in_same_sub_tab(cx: &mut TestAppContext) {
    let h = start(cx, Setup::default());
    let page = open_detail(&h, cx, "redis", ContainerTab::Terminal);
    let tab = terminal_tab(&page, cx);
    wait_sessions(&h, cx, &tab, 1);
    h.wait_until(cx, "focused", |window, cx| {
        tab.read(cx)
            .active_view()
            .is_some_and(|v| gpui_kit::Focusable::focus_handle(v.read(cx), cx).is_focused(window))
    });
    // `exit` ends the FakeTerminal (exit code 0); it must arrive as one write.
    let v = cx.read(|cx| tab.read(cx).active_view().cloned()).unwrap();
    v.update(cx, |v, cx| {
        v.paste_text(
            "exit
", cx,
        )
    });
    h.wait_until(cx, "exited", |_, cx| {
        tab.read(cx)
            .active_view()
            .is_some_and(|v| v.read(cx).is_exited())
    });
    assert!(grid(&tab, cx).contains("[process exited with code 0]"));
    h.press(cx, "enter");
    h.wait_until(cx, "reconnected", |_, cx| {
        tab.read(cx)
            .active_view()
            .is_some_and(|v| !v.read(cx).is_exited() && v.read(cx).grid_text().contains("fake$"))
    });
    assert_eq!(
        cx.read(|cx| tab.read(cx).sessions().len()),
        1,
        "same sub-tab"
    );
    assert_eq!(h.engine.exec_count(), 2);
    h.shutdown();
}

#[gpui_kit::test]
fn trm_004_shell_and_user_in_exec_request(cx: &mut TestAppContext) {
    use crate::pages::container_detail::terminal::Shell;
    let mut config = dk_hub::Config::default();
    config.terminal.default_shell = "bash".into();
    let h = start(
        cx,
        Setup {
            config,
            ..Default::default()
        },
    );
    let page = open_detail(&h, cx, "redis", ContainerTab::Terminal);
    let tab = terminal_tab(&page, cx);
    wait_sessions(&h, cx, &tab, 1);
    let shell = cx.read(|cx| tab.read(cx).sessions()[0].shell);
    assert_eq!(shell, Shell::Bash, "default shell from config (TRM-004)");
    tab.update(cx, |t, _| t.set_shell(Shell::Sh));
    h.update(cx, |_, window, cx| {
        window.dispatch_action(Box::new(crate::actions::term_ext::Reconnect), cx)
    });
    h.wait_until(cx, "reconnected", |_, _| h.engine.exec_count() == 2);
    let shell = cx.read(|cx| tab.read(cx).sessions()[0].shell);
    assert_eq!(shell, Shell::Sh);
    h.shutdown();
}

#[gpui_kit::test]
fn trm_008_session_survives_navigation(cx: &mut TestAppContext) {
    let h = start(cx, Setup::default());
    let page = open_detail(&h, cx, "redis", ContainerTab::Terminal);
    let tab = terminal_tab(&page, cx);
    wait_sessions(&h, cx, &tab, 1);
    let hub_id = cx.read(|cx| tab.read(cx).sessions()[0].hub_id);
    let view = cx.read(|cx| tab.read(cx).active_view().cloned()).unwrap();
    // Leave to another page of the same engine (Ctrl+2 inside the terminal goes to the
    // shell, KBD-062, so leave it first)…
    leave_terminal(&h, cx);
    // Our test handles would keep the page alive; the shell's is the only owner.
    drop((page, tab));
    h.press(cx, "ctrl-2");
    h.wait_until(cx, "images page", |_, cx| {
        *h.shell.read(cx).route() == Route::Images
    });
    h.wait_until(cx, "parked", |_, cx| {
        crate::state::TerminalRegistry::parked_count(cx) == 1
    });
    // …and come back: the same session and view are re-attached, no new exec.
    let page = open_detail(&h, cx, "redis", ContainerTab::Terminal);
    let tab = terminal_tab(&page, cx);
    h.draw(cx);
    let (again, view2) = cx.read(|cx| {
        let t = tab.read(cx);
        (
            t.sessions().first().and_then(|s| s.hub_id),
            t.active_view().cloned(),
        )
    });
    assert_eq!(again, hub_id);
    assert_eq!(view2, Some(view));
    assert_eq!(h.engine.exec_count(), 1);
    assert_eq!(cx.read(crate::state::TerminalRegistry::parked_count), 0);
    // The session is still live: typing echoes.
    h.wait_until(cx, "focused", |window, cx| {
        tab.read(cx)
            .active_view()
            .is_some_and(|v| gpui_kit::Focusable::focus_handle(v.read(cx), cx).is_focused(window))
    });
    h.type_text(cx, "again");
    h.wait_until(cx, "echo", |_, cx| {
        tab.read(cx)
            .active_view()
            .is_some_and(|v| v.read(cx).grid_text().contains("again"))
    });
    h.shutdown();
}

#[gpui_kit::test]
fn trm_008_closed_on_engine_switch(cx: &mut TestAppContext) {
    let other = dk_core::fake::FakeEngine::new("other");
    let h = start(
        cx,
        Setup {
            extra_engines: vec![other],
            ..Default::default()
        },
    );
    let page = open_detail(&h, cx, "redis", ContainerTab::Terminal);
    let tab = terminal_tab(&page, cx);
    wait_sessions(&h, cx, &tab, 1);
    let view = cx.read(|cx| tab.read(cx).active_view().cloned()).unwrap();
    leave_terminal(&h, cx);
    drop((page, tab));
    h.press(cx, "ctrl-1");
    h.wait_until(cx, "parked", |_, cx| {
        crate::state::TerminalRegistry::parked_count(cx) == 1
    });
    let target: gpui_kit::SharedString = "other".into();
    h.update(cx, |_, window, cx| {
        window.dispatch_action(Box::new(crate::actions::SwitchEngine { id: target }), cx)
    });
    h.wait_until(cx, "switched", |_, cx| {
        h.shell
            .read(cx)
            .store()
            .is_some_and(|s| s.read(cx).engine_id().as_str() == "other")
    });
    assert_eq!(
        cx.read(crate::state::TerminalRegistry::parked_count),
        0,
        "TRM-008: engine switch closes sessions"
    );
    // The hub-side actor was told to close: the view saw the exit.
    h.wait_until(cx, "session ended", |_, cx| view.read(cx).is_exited());
    h.shutdown();
}

#[gpui_kit::test]
fn trm_008_closed_on_container_removal(cx: &mut TestAppContext) {
    let h = start(cx, Setup::default());
    let page = open_detail(&h, cx, "redis", ContainerTab::Terminal);
    let tab = terminal_tab(&page, cx);
    wait_sessions(&h, cx, &tab, 1);
    let view = cx.read(|cx| tab.read(cx).active_view().cloned()).unwrap();
    h.wait_until(cx, "events", |_, _| h.engine.open_streams().0 > 0);
    let id = id_of("redis");
    let mut cs = crate::demo::containers();
    cs.retain(|c| c.id != id);
    h.engine.set_containers(cs);
    h.engine.emit_event(dk_core::fake::fixtures::event(
        ResourceKind::Container,
        "destroy",
        &id,
    ));
    h.wait_until(cx, "read-only", |_, cx| view.read(cx).is_read_only());
    h.shutdown();
}

// ── Stats (STA-001…011, KBD-070) ──────────────────────────────────────────────────────────

use crate::pages::container_detail::stats::{StatsTab, StatsWindow};

fn stats_tab(page: &Entity<ContainerDetailPage>, cx: &mut TestAppContext) -> Entity<StatsTab> {
    cx.read(|cx| page.read(cx).tabs().stats.clone())
        .expect("stats tab entity")
}

fn sample(i: i64, cpu: f64) -> dk_core::StatsSample {
    dk_core::fake::fixtures::stats_sample(
        time::OffsetDateTime::now_utc() - time::Duration::seconds(60 - i),
        cpu,
        64 * 1024 * 1024,
    )
}

fn wait_stats_stream(h: &Harness, cx: &mut TestAppContext, n: usize) {
    h.wait_until(cx, "stats subscribed", |_, _| {
        h.engine.open_streams().2 == n
    });
}

#[gpui_kit::test]
fn sta_001_samples_render_and_history_replays(cx: &mut TestAppContext) {
    let h = start(cx, Setup::default());
    let page = open_detail(&h, cx, "redis", ContainerTab::Stats);
    let stats = stats_tab(&page, cx);
    wait_stats_stream(&h, cx, 1);
    let redis = id_of("redis");
    for i in 0..5 {
        h.engine.push_stats(&redis, sample(i, 10.0 + i as f64));
    }
    h.wait_until(cx, "5 samples", |_, cx| stats.read(cx).samples().len() == 5);
    // Leave the tab and come back: the hub replays its buffer (STA-006, STA-003).
    focus_tab_bar(&h, &page, cx);
    h.press(cx, "right");
    assert_eq!(cx.read(|cx| page.read(cx).tab()), ContainerTab::Mounts);
    h.press(cx, "left");
    h.wait_until(cx, "replayed", |_, cx| stats.read(cx).samples().len() == 5);
    let last_cpu = cx.read(|cx| stats.read(cx).samples().last().map(|s| s.cpu_percent));
    assert_eq!(last_cpu, Some(14.0));
    h.shutdown();
}

#[gpui_kit::test]
fn sta_006_stats_only_while_visible(cx: &mut TestAppContext) {
    let h = start(cx, Setup::default());
    let page = open_detail(&h, cx, "redis", ContainerTab::Overview);
    h.draw(cx);
    assert!(
        h.engine.calls_to("stats").is_empty(),
        "no stats stream outside the Stats tab"
    );
    focus_tab_bar(&h, &page, cx);
    h.press(cx, "right right right");
    assert_eq!(cx.read(|cx| page.read(cx).tab()), ContainerTab::Stats);
    let stats = stats_tab(&page, cx);
    h.wait_until(cx, "streaming", |_, cx| stats.read(cx).is_streaming());
    wait_stats_stream(&h, cx, 1);
    // Switching away drops the subscription (the hub lingers 5 s upstream, but our stream
    // is gone).
    h.press(cx, "right");
    assert!(!cx.read(|cx| stats.read(cx).is_streaming()));
    h.shutdown();
}

#[gpui_kit::test]
fn kbd_070_window_selector_keys(cx: &mut TestAppContext) {
    let h = start(cx, Setup::default());
    let page = open_detail(&h, cx, "redis", ContainerTab::Stats);
    let stats = stats_tab(&page, cx);
    let f = cx.read(|cx| stats.read(cx).window_focus().clone());
    h.focus(cx, &f);
    let w = |cx: &mut TestAppContext| cx.read(|cx| stats.read(cx).window());
    assert_eq!(w(cx), StatsWindow::M5, "default 5m");
    h.press(cx, "right");
    assert_eq!(w(cx), StatsWindow::M15);
    h.press(cx, "left left");
    assert_eq!(w(cx), StatsWindow::M1);
    h.press(cx, "5");
    assert_eq!(w(cx), StatsWindow::M5);
    h.press(cx, "f");
    assert_eq!(w(cx), StatsWindow::M15);
    h.press(cx, "1");
    assert_eq!(w(cx), StatsWindow::M1);
    h.shutdown();
}

#[gpui_kit::test]
fn sta_008_processes_gated_on_top(cx: &mut TestAppContext) {
    let h = start(cx, Setup::default());
    let page = open_detail(&h, cx, "redis", ContainerTab::Stats);
    let stats = stats_tab(&page, cx);
    h.wait_until(cx, "top loaded", |_, cx| stats.read(cx).top().is_some());
    h.shutdown();

    let h = start(cx, Setup::default());
    h.engine
        .set_capabilities(dk_core::Capabilities::all() - dk_core::Capabilities::TOP);
    h.wait_containers(cx);
    h.update(cx, |_, window, cx| {
        window.dispatch_action(Box::new(crate::actions::Refresh), cx)
    });
    h.wait_until(cx, "caps without TOP", |_, cx| {
        h.shell.read(cx).store().is_some_and(|s| {
            !s.read(cx)
                .capabilities()
                .contains(dk_core::Capabilities::TOP)
                && s.read(cx).info().is_some()
        })
    });
    let page = open_detail(&h, cx, "redis", ContainerTab::Stats);
    let stats = stats_tab(&page, cx);
    h.draw(cx);
    assert!(h.engine.calls_to("top").is_empty(), "no top() without TOP");
    assert!(cx.read(|cx| stats.read(cx).top().is_none()));
    h.shutdown();
}

#[gpui_kit::test]
fn sta_007_stopped_container_keeps_buffer(cx: &mut TestAppContext) {
    let h = start(cx, Setup::default());
    let page = open_detail(&h, cx, "redis", ContainerTab::Stats);
    let stats = stats_tab(&page, cx);
    wait_stats_stream(&h, cx, 1);
    let redis = id_of("redis");
    for i in 0..3 {
        h.engine.push_stats(&redis, sample(i, 5.0));
    }
    h.wait_until(cx, "samples", |_, cx| stats.read(cx).samples().len() == 3);
    focus_tab_bar(&h, &page, cx);
    h.press(cx, "s");
    h.wait_until(cx, "stopped", |_, cx| {
        page.read(cx)
            .detail_state()
            .is_some_and(|s| !s.read(cx).is_running(cx))
    });
    assert_eq!(
        cx.read(|cx| stats.read(cx).samples().len()),
        3,
        "buffer kept"
    );
    h.shutdown();
}

#[gpui_kit::test]
fn sta_010_disk_usage_on_demand(cx: &mut TestAppContext) {
    let mut containers = crate::demo::containers();
    if let Some(c) = containers.iter_mut().find(|c| c.name == "redis") {
        c.size_rw = Some(12_000);
        c.size_root_fs = Some(98_000_000);
    }
    let h = start(
        cx,
        Setup {
            containers,
            ..Default::default()
        },
    );
    let page = open_detail(&h, cx, "redis", ContainerTab::Stats);
    let stats = stats_tab(&page, cx);
    h.draw(cx);
    assert!(
        cx.read(|cx| stats.read(cx).disk().is_none()),
        "not fetched up front"
    );
    // The button / palette entry dispatch from inside the tab (focus on the selector).
    let f = cx.read(|cx| stats.read(cx).window_focus().clone());
    h.focus(cx, &f);
    h.update(cx, |_, window, cx| {
        window.dispatch_action(Box::new(crate::actions::stats::LoadDiskUsage), cx)
    });
    h.wait_until(cx, "disk usage", |_, cx| stats.read(cx).disk().is_some());
    assert_eq!(
        cx.read(|cx| stats.read(cx).disk()),
        Some((Some(12_000), Some(98_000_000)))
    );
    h.shutdown();
}

/// Spec 10 §4.2 / spec 20 §5.4 (WSLC events lost): when the hub reports `Feed::Lagged`, both the
/// `EngineStore` and the container detail state do a full refetch **and keep the subscription**
/// (later events still apply). The lag is real: the UI isn't pumped while the engine floods
/// events past the hub's ring.
#[gpui_kit::test]
fn eng_events_lagged_full_refetch_and_subscription_stays(cx: &mut TestAppContext) {
    let h = start(cx, Setup::default());
    let _page = open_detail(&h, cx, "scratchpad", ContainerTab::Overview);
    let id = id_of("scratchpad");
    // Two subscribers (store + detail) share the engine's one upstream stream.
    h.wait_until(cx, "subscribed", |_, _| h.engine.open_streams().0 > 0);
    h.draw(cx);
    h.engine.clear_calls();
    // Volume events: by themselves they refetch only volumes (store) and nothing (detail), so a
    // containers list / inspect refetch can only come from the `Lagged` handling.
    for i in 0..4000 {
        h.engine.emit_event(dk_core::fake::fixtures::event(
            ResourceKind::Volume,
            "create",
            &format!("v{i}"),
        ));
    }
    // Let the hub overrun its ring while the UI side reads nothing.
    std::thread::sleep(std::time::Duration::from_millis(300)); // nfr-001-allow: test harness only
    h.wait_until(cx, "full refetch after lag", |_, _| {
        !h.engine.calls_to("list_containers").is_empty()
            && !h.engine.calls_to("list_networks").is_empty()
            && !h.engine.calls_to("inspect_container").is_empty()
    });
    // Still subscribed: the store is in events mode (not polling) and a later event applies.
    let mode = cx.read(|cx| h.shell.read(cx).store().map(|s| s.read(cx).live_mode()));
    assert_eq!(mode, Some(crate::state::LiveMode::Events));
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
    h.wait_until(cx, "event after the lag still refreshes the detail", |_, cx| {
        detail_running(&h, cx)
    });
    h.shutdown();
}
