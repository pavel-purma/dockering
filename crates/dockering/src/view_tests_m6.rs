//! M6 view tests: Images, Volumes, Networks and their detail pages (IMG-*, VOL-*, NET-*,
//! KBD-030/071/072). Real `HubHandle` + `FakeEngine`; test names carry requirement ids.

use dk_core::{Capabilities, EngineError};
use gpui_kit::{Focusable, TestAppContext};

use crate::nav::{ImageTab, Route};
use crate::pages::images::dialogs::{PullDialog, RunDialog};
use crate::pages::images::{ImageFilter, ImagesPage};
use crate::pages::resources::pull::{PullManager, PullPhase};
use crate::testing::{Harness, Setup, start};

fn images_page(h: &Harness, cx: &mut TestAppContext) -> gpui_kit::Entity<ImagesPage> {
    h.wait_containers(cx);
    let page = h.goto::<ImagesPage>(cx, Route::Images);
    h.wait_until(cx, "image rows", |_, cx| {
        !page.read(cx).table().read(cx).model(cx).rows().is_empty()
    });
    page
}

fn row_keys(page: &gpui_kit::Entity<ImagesPage>, cx: &mut TestAppContext) -> Vec<String> {
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

fn focus_images_table(h: &Harness, page: &gpui_kit::Entity<ImagesPage>, cx: &mut TestAppContext) {
    let handle = cx.read(|cx| page.read(cx).table().focus_handle(cx));
    h.focus(cx, &handle);
}

fn select_image(page: &gpui_kit::Entity<ImagesPage>, key: &str, cx: &mut TestAppContext) {
    let key = key.to_owned();
    cx.update(|cx| {
        let t = page.read(cx).table().clone();
        t.update(cx, |t, cx| t.focus_row(&key, cx));
    });
}

fn key_of(page: &gpui_kit::Entity<ImagesPage>, label: &str, cx: &mut TestAppContext) -> String {
    let label = label.to_owned();
    cx.read(|cx| {
        page.read(cx)
            .rows()
            .iter()
            .find(|r| r.label() == label || format!("{}:{}", r.repo, r.tag) == label)
            .map(|r| r.key.to_string())
            .unwrap_or_else(|| panic!("no image row {label}"))
    })
}

// ── IMG-001…003 ──────────────────────────────────────────────────────────────────────

/// Render the delayed tooltip, not just its trigger. A binding predicate passed to
/// Tooltip::action used to recurse forever in GPUI's KeyContext parser on hover.
fn hover_toolbar_tooltip(h: &Harness, cx: &mut TestAppContext, selector: &'static str) {
    h.draw(cx);
    let mut visual = gpui_kit::VisualTestContext::from_window(h.any_window(), cx);
    let bounds = visual
        .debug_bounds(selector)
        .expect("toolbar button rendered");
    visual.simulate_mouse_move(bounds.center(), None, Default::default());
    cx.run_until_parked();
    cx.executor()
        .advance_clock(std::time::Duration::from_secs(1));
    h.draw(cx);
    assert!(!h.has_dialog(cx), "hover must not activate the button");
}

#[gpui_kit::test]
fn images_pull_tooltip_renders_on_hover(cx: &mut TestAppContext) {
    let h = start(cx, Setup::default());
    images_page(&h, cx);
    hover_toolbar_tooltip(&h, cx, "pull-image-trigger");
    h.shutdown();
}

#[gpui_kit::test]
fn volumes_create_tooltip_renders_on_hover(cx: &mut TestAppContext) {
    let h = start(cx, Setup::default());
    volumes_page(&h, cx);
    hover_toolbar_tooltip(&h, cx, "create-volume-trigger");
    h.shutdown();
}

#[gpui_kit::test]
fn img_001_one_row_per_tag_and_dangling(cx: &mut TestAppContext) {
    let h = start(cx, Setup::default());
    let mut multi = dk_core::fake::fixtures::image("nginx:1.27", "nginx");
    multi.repo_tags.push("nginx:latest".into());
    h.engine.set_images(vec![
        multi,
        dk_core::fake::fixtures::image("redis:7", "redis"),
        dk_core::fake::fixtures::image("", "dangling"),
    ]);
    let page = images_page(&h, cx);
    h.wait_until(cx, "4 rows", |_, cx| {
        page.read(cx).table().read(cx).model(cx).rows().len() == 4
    });
    let rows = cx.read(|cx| page.read(cx).rows().to_vec());
    assert_eq!(rows.iter().filter(|r| r.repo == "nginx").count(), 2);
    assert!(
        rows.iter()
            .any(|r| r.dangling && r.repo == "<none>" && r.tag == "<none>")
    );
    h.shutdown();
}

#[gpui_kit::test]
fn img_002_filters_and_search(cx: &mut TestAppContext) {
    let h = start(cx, Setup::default());
    let page = images_page(&h, cx);
    let all = row_keys(&page, cx).len();
    let used = cx.read(|cx| page.read(cx).rows().iter().filter(|r| r.in_use > 0).count());
    assert!(used > 0 && used < all, "demo has used and unused images");
    cx.update(|cx| page.update(cx, |p, cx| p.set_filter(ImageFilter::InUse, cx)));
    assert_eq!(row_keys(&page, cx).len(), used);
    cx.update(|cx| page.update(cx, |p, cx| p.set_filter(ImageFilter::Dangling, cx)));
    assert_eq!(row_keys(&page, cx).len(), 1);
    cx.update(|cx| page.update(cx, |p, cx| p.set_filter(ImageFilter::Unused, cx)));
    assert_eq!(row_keys(&page, cx).len(), all - used);
    cx.update(|cx| page.update(cx, |p, cx| p.set_filter(ImageFilter::All, cx)));
    // Search (debounced, SHL-006).
    h.press(cx, "secondary-f");
    h.type_text(cx, "postgres");
    h.wait_until(cx, "search applied", |_, cx| {
        page.read(cx).table().read(cx).model(cx).rows().len() == 1
    });
    h.shutdown();
}

#[gpui_kit::test]
fn img_000_four_states(cx: &mut TestAppContext) {
    // Loading → error (with Retry = Refresh) → data → empty.
    let h = start(cx, Setup::default());
    h.engine.set_error(
        "list_images",
        Some(EngineError::Api {
            status: 500,
            message: "boom".into(),
        }),
    );
    h.wait_containers(cx);
    let page = h.goto::<ImagesPage>(cx, Route::Images);
    h.wait_until(cx, "images error", |_, cx| {
        h.shell
            .read(cx)
            .store()
            .is_some_and(|s| s.read(cx).images.error().is_some())
    });
    h.engine.set_error("list_images", None);
    h.update(cx, |_, window, cx| {
        window.dispatch_action(Box::new(crate::actions::Refresh), cx)
    });
    h.wait_until(cx, "rows after retry", |_, cx| {
        !page.read(cx).table().read(cx).model(cx).rows().is_empty()
    });
    h.engine.set_images(vec![]);
    h.update(cx, |_, window, cx| {
        window.dispatch_action(Box::new(crate::actions::Refresh), cx)
    });
    h.wait_until(cx, "empty", |_, cx| {
        page.read(cx).table().read(cx).model(cx).rows().is_empty()
            && !h
                .shell
                .read(cx)
                .store()
                .is_some_and(|s| s.read(cx).images.is_loading())
    });
    h.shutdown();
}

#[gpui_kit::test]
fn img_003_prune_confirm_shows_reclaimable(cx: &mut TestAppContext) {
    let h = start(cx, Setup::default());
    h.engine.set_disk_usage(dk_core::DiskUsage {
        images_reclaimable: Some(123_000_000),
        ..Default::default()
    });
    let page = images_page(&h, cx);
    focus_images_table(&h, &page, cx);
    h.update(cx, |_, window, cx| {
        window.dispatch_action(Box::new(crate::actions::res::PruneUnused), cx)
    });
    h.wait_until(cx, "confirm open", |window, cx| {
        use gpui_kit::component::WindowExt;
        window.has_active_dialog(cx)
    });
    assert!(
        !h.engine.calls_to("disk_usage").is_empty(),
        "IMG-003 uses disk_usage"
    );
    // Mod+Enter confirms.
    h.draw(cx);
    h.press(cx, "secondary-enter");
    h.wait_until(cx, "prune called", |_, _| {
        !h.engine.calls_to("prune_images").is_empty()
    });
    h.shutdown();
}

// ── IMG-006 delete / force ───────────────────────────────────────────────────────────

#[gpui_kit::test]
fn img_006_delete_in_use_offers_force(cx: &mut TestAppContext) {
    let h = start(cx, Setup::default());
    let page = images_page(&h, cx);
    let key = key_of(&page, "nginx:1.27", cx);
    focus_images_table(&h, &page, cx);
    select_image(&page, &key, cx);
    h.press(cx, "delete");
    assert!(h.has_dialog(cx), "delete confirms (SHL-002)");
    h.draw(cx);
    h.press(cx, "secondary-enter");
    h.wait_until(cx, "remove called", |_, _| {
        !h.engine.calls_to("remove_image").is_empty()
    });
    // In use → a second dialog with Force.
    h.wait_until(cx, "force dialog", |window, cx| {
        use gpui_kit::component::WindowExt;
        window.has_active_dialog(cx)
    });
    h.draw(cx);
    h.engine.clear_calls();
    h.press(cx, "secondary-enter");
    h.wait_until(cx, "forced remove", |_, _| {
        !h.engine.calls_to("remove_image").is_empty()
    });
    h.wait_until(cx, "nginx gone", |_, cx| {
        h.shell.read(cx).store().is_some_and(|s| {
            s.read(cx).images.data().is_some_and(|d| {
                d.iter()
                    .all(|i| !i.repo_tags.contains(&"nginx:1.27".to_owned()))
            })
        })
    });
    h.shutdown();
}

#[gpui_kit::test]
fn kbd_071_destructive_confirm_focuses_cancel(cx: &mut TestAppContext) {
    let h = start(cx, Setup::default());
    let page = images_page(&h, cx);
    let key = key_of(&page, "redis:7", cx);
    focus_images_table(&h, &page, cx);
    select_image(&page, &key, cx);
    h.press(cx, "delete");
    h.draw(cx);
    // Plain Enter on the focused Cancel closes without deleting.
    h.press(cx, "enter");
    h.draw(cx);
    assert!(h.engine.calls_to("remove_image").is_empty());
    h.shutdown();
}

// ── IMG-004 pull ─────────────────────────────────────────────────────────────────────

#[gpui_kit::test]
fn img_004_pull_progress_notification(cx: &mut TestAppContext) {
    let h = start(cx, Setup::default());
    let page = images_page(&h, cx);
    focus_images_table(&h, &page, cx);
    // `G` opens the pull dialog with the reference field focused (KBD-030, KBD-071).
    h.press(cx, "g");
    assert!(h.has_dialog(cx));
    h.draw(cx);
    h.type_text(cx, "alpine:3.20");
    h.press(cx, "enter");
    h.wait_until(cx, "pull started", |_, _| {
        !h.engine.calls_to("pull_image").is_empty()
    });
    assert!(!h.has_dialog(cx), "the dialog closes immediately (SHL-012)");
    h.wait_until(cx, "pull finished", |_, cx| {
        PullManager::try_global(cx).is_some_and(|m| {
            m.read(cx)
                .finished()
                .iter()
                .any(|p| matches!(p.phase, PullPhase::Done { .. }) && p.structured)
        })
    });
    // The images list refetched and shows the new tag.
    h.wait_until(cx, "alpine row", |_, cx| {
        page.read(cx)
            .rows()
            .iter()
            .any(|r| r.repo == "alpine" && r.tag == "3.20")
    });
    h.shutdown();
}

#[gpui_kit::test]
fn img_007_pull_401_shows_login_hint(cx: &mut TestAppContext) {
    let h = start(cx, Setup::default());
    h.engine.set_error(
        "pull_image",
        Some(EngineError::Api {
            status: 401,
            message: "unauthorized".into(),
        }),
    );
    let page = images_page(&h, cx);
    focus_images_table(&h, &page, cx);
    h.press(cx, "g");
    h.draw(cx);
    h.type_text(cx, "ghcr.io/me/private:1");
    h.press(cx, "enter");
    h.wait_until(cx, "pull failed", |_, cx| {
        PullManager::try_global(cx).is_some_and(|m| {
            m.read(cx).finished().iter().any(|p| {
                matches!(&p.phase, PullPhase::Failed(msg) if msg == "Authentication required: run `docker login ghcr.io`")
            })
        })
    });
    h.shutdown();
}

#[gpui_kit::test]
fn img_004_pull_dialog_validates_reference(cx: &mut TestAppContext) {
    let h = start(cx, Setup::default());
    let _page = images_page(&h, cx);
    let dialog = cx.update(|cx| {
        let window = h.any_window();
        let mut out = None;
        window
            .update(cx, |_, window, cx| {
                out = Some(PullDialog::open(
                    dk_core::EngineId::new("demo"),
                    Capabilities::all(),
                    gpui_kit::WeakEntity::new_invalid(),
                    Some("Not A Ref".into()),
                    window,
                    cx,
                ));
            })
            .ok();
        out.expect("dialog")
    });
    h.draw(cx);
    h.press(cx, "enter");
    assert!(cx.read(|cx| dialog.read(cx).error().is_some()));
    assert!(h.engine.calls_to("pull_image").is_empty());
    h.shutdown();
}

// ── IMG-005 run ──────────────────────────────────────────────────────────────────────

#[gpui_kit::test]
fn img_005_run_dialog_runs_and_navigates(cx: &mut TestAppContext) {
    let h = start(cx, Setup::default());
    let page = images_page(&h, cx);
    let key = key_of(&page, "redis:7", cx);
    focus_images_table(&h, &page, cx);
    select_image(&page, &key, cx);
    h.press(cx, "u");
    assert!(h.has_dialog(cx), "U opens the run dialog");
    h.draw(cx);
    // Initial focus = the first field (KBD-071): typing goes into the name.
    h.type_text(cx, "cache");
    let dialog = find_run_dialog(&page, cx);
    assert_eq!(
        cx.read(|cx| dialog.read(cx).name_input().read(cx).value().to_string()),
        "cache"
    );
    // Add a port row with the button path and an env row with Mod+Shift+Enter (KBD-072).
    let ports = cx.read(|cx| dialog.read(cx).ports().clone());
    let add = cx.read(|cx| ports.read(cx).add_focus().clone());
    h.focus(cx, &add);
    h.press(cx, "enter");
    assert_eq!(
        cx.read(|cx| ports.read(cx).len()),
        1,
        "Enter on Add adds a row"
    );
    h.type_text(cx, "6380");
    h.press(cx, "tab");
    h.type_text(cx, "6379");
    h.press(cx, "secondary-shift-enter");
    assert_eq!(
        cx.read(|cx| ports.read(cx).len()),
        2,
        "Mod+Shift+Enter adds a row"
    );
    h.press(cx, "secondary-shift-backspace");
    assert_eq!(
        cx.read(|cx| ports.read(cx).len()),
        1,
        "Mod+Shift+Backspace removes it"
    );
    let env = cx.read(|cx| dialog.read(cx).env().clone());
    let env_add = cx.read(|cx| env.read(cx).add_focus().clone());
    h.focus(cx, &env_add);
    h.press(cx, "secondary-shift-enter");
    h.type_text(cx, "MODE");
    h.press(cx, "tab");
    h.type_text(cx, "fast");
    cx.update(|cx| dialog.update(cx, |d, cx| d.set_auto_remove(true, cx)));
    h.engine.clear_calls();
    h.press(cx, "secondary-enter");
    h.wait_until(cx, "run_image called", |_, _| {
        !h.engine.calls_to("run_image").is_empty()
    });
    assert_eq!(h.engine.calls_to("run_image")[0].arg, "redis:7");
    h.wait_until(cx, "navigated to container detail", |_, cx| {
        matches!(h.shell.read(cx).route(), Route::ContainerDetail { .. })
    });
    let created = cx.read(|cx| {
        h.shell.read(cx).store().and_then(|s| {
            s.read(cx)
                .containers
                .data()
                .and_then(|d| d.iter().find(|c| c.name == "cache").cloned())
        })
    });
    let c = created.expect("the new container is listed");
    assert_eq!(c.ports.len(), 1);
    assert_eq!((c.ports[0].public, c.ports[0].private), (Some(6380), 6379));
    h.shutdown();
}

fn find_run_dialog(
    page: &gpui_kit::Entity<ImagesPage>,
    cx: &mut TestAppContext,
) -> gpui_kit::Entity<RunDialog> {
    cx.read(|cx| page.read(cx).last_run_dialog())
        .expect("run dialog open")
}

// ── IMG-010/011 detail ───────────────────────────────────────────────────────────────

#[gpui_kit::test]
fn img_010_detail_tabs_and_history_gating(cx: &mut TestAppContext) {
    use crate::pages::image_detail::ImageDetailPage;
    let h = start(cx, Setup::default());
    let page = images_page(&h, cx);
    let key = key_of(&page, "redis:7", cx);
    focus_images_table(&h, &page, cx);
    select_image(&page, &key, cx);
    h.press(cx, "enter");
    let route = h.read(cx, |s, _, _| s.route().clone());
    let detail = h.goto::<ImageDetailPage>(cx, route);
    h.wait_until(cx, "details loaded", |_, cx| {
        detail.read(cx).details().data().is_some()
    });
    assert!(matches!(
        h.read(cx, |s, _, _| s.route().clone()),
        Route::ImageDetail {
            tab: ImageTab::Overview,
            ..
        }
    ));
    // Ctrl+Tab → Layers (history loaded), → Used by, → Inspect.
    h.press(cx, "ctrl-tab");
    assert_eq!(cx.read(|cx| detail.read(cx).tab()), ImageTab::Layers);
    h.wait_until(cx, "history", |_, cx| {
        detail.read(cx).history().data().is_some()
    });
    h.press(cx, "ctrl-tab ctrl-tab");
    assert_eq!(cx.read(|cx| detail.read(cx).tab()), ImageTab::Inspect);
    h.wait_until(cx, "json", |_, cx| {
        !detail.read(cx).inspect().read(cx).text().is_empty()
    });
    // The tab is part of the route (no new history entry).
    assert!(matches!(
        h.read(cx, |s, _, _| s.route().clone()),
        Route::ImageDetail {
            tab: ImageTab::Inspect,
            ..
        }
    ));
    h.shutdown();
}

#[gpui_kit::test]
fn img_010_layers_tab_skipped_without_image_history(cx: &mut TestAppContext) {
    use crate::pages::image_detail::ImageDetailPage;
    let h = start(cx, Setup::default());
    h.engine
        .set_capabilities(Capabilities::all() - Capabilities::IMAGE_HISTORY);
    h.update(cx, |_, window, cx| {
        window.dispatch_action(Box::new(crate::actions::Refresh), cx)
    });
    h.wait_until(cx, "caps without history", |_, cx| {
        h.shell.read(cx).store().is_some_and(|s| {
            s.read(cx)
                .info()
                .is_some_and(|i| !i.capabilities.contains(Capabilities::IMAGE_HISTORY))
        })
    });
    let page = images_page(&h, cx);
    let id = cx.read(|cx| page.read(cx).rows()[0].image_id.clone());
    let detail = h.goto::<ImageDetailPage>(
        cx,
        Route::ImageDetail {
            id,
            tab: ImageTab::Overview,
        },
    );
    h.draw(cx);
    h.press(cx, "ctrl-tab");
    assert_eq!(cx.read(|cx| detail.read(cx).tab()), ImageTab::UsedBy);
    assert!(h.engine.calls_to("image_history").is_empty());
    h.shutdown();
}

#[gpui_kit::test]
fn img_011_tag_dialog_tags_image(cx: &mut TestAppContext) {
    use crate::pages::image_detail::ImageDetailPage;
    let h = start(cx, Setup::default());
    let page = images_page(&h, cx);
    let id = cx.read(|cx| {
        page.read(cx)
            .rows()
            .iter()
            .find(|r| r.repo == "redis")
            .map(|r| r.image_id.clone())
            .expect("redis")
    });
    let detail = h.goto::<ImageDetailPage>(
        cx,
        Route::ImageDetail {
            id,
            tab: ImageTab::Overview,
        },
    );
    h.wait_until(cx, "details loaded", |_, cx| {
        detail.read(cx).details().data().is_some()
    });
    h.update(cx, |_, window, cx| {
        window.dispatch_action(Box::new(crate::actions::res::TagImage), cx)
    });
    assert!(h.has_dialog(cx));
    h.draw(cx);
    // Repo is prefilled ("redis"); move to the tag field and type.
    h.press(cx, "tab");
    h.type_text(cx, "backup");
    h.press(cx, "enter");
    h.wait_until(cx, "tag_image called", |_, _| {
        !h.engine.calls_to("tag_image").is_empty()
    });
    h.wait_until(cx, "new tag listed", |_, cx| {
        page.read(cx).rows().iter().any(|r| r.tag == "backup")
            || h.shell.read(cx).store().is_some_and(|s| {
                s.read(cx).images.data().is_some_and(|d| {
                    d.iter()
                        .any(|i| i.repo_tags.contains(&"redis:backup".to_owned()))
                })
            })
    });
    h.shutdown();
}

// ── VOL-* ────────────────────────────────────────────────────────────────────────────

use crate::pages::volumes::{UsageState, VolumeFilter, VolumesPage};

fn volumes_page(h: &Harness, cx: &mut TestAppContext) -> gpui_kit::Entity<VolumesPage> {
    h.wait_containers(cx);
    let page = h.goto::<VolumesPage>(cx, Route::Volumes);
    h.wait_until(cx, "volume rows", |_, cx| {
        !page.read(cx).table().read(cx).model(cx).rows().is_empty()
    });
    page
}

#[gpui_kit::test]
fn vol_002_lazy_sizes(cx: &mut TestAppContext) {
    let h = start(cx, Setup::default());
    h.wait_containers(cx);
    h.engine.clear_calls();
    // Other pages never call disk_usage (spec 10 §4.2 step 5).
    let _ = h.goto::<ImagesPage>(cx, Route::Images);
    h.draw(cx);
    assert!(h.engine.calls_to("disk_usage").is_empty());
    // Slow df: the list renders first with skeleton sizes, then sizes arrive.
    h.engine.set_latency(std::time::Duration::from_millis(200));
    let page = h.goto::<VolumesPage>(cx, Route::Volumes);
    h.wait_until(cx, "volume rows", |_, cx| {
        !page.read(cx).table().read(cx).model(cx).rows().is_empty()
    });
    let loading = cx.read(|cx| matches!(page.read(cx).usage(), UsageState::Loading));
    assert!(loading, "sizes load after the list renders");
    h.wait_until(cx, "sizes known", |_, cx| {
        matches!(page.read(cx).usage(), UsageState::Known(_))
    });
    h.engine.set_latency(std::time::Duration::ZERO);
    assert_eq!(h.engine.calls_to("disk_usage").len(), 1);
    let sized = cx.read(|cx| page.read(cx).rows().iter().all(|r| r.size.is_some()));
    assert!(sized);
    h.shutdown();
}

#[gpui_kit::test]
fn vol_002_dash_without_disk_usage(cx: &mut TestAppContext) {
    let h = start(cx, Setup::default());
    h.engine
        .set_capabilities(Capabilities::all() - Capabilities::DISK_USAGE);
    h.update(cx, |_, window, cx| {
        window.dispatch_action(Box::new(crate::actions::Refresh), cx)
    });
    h.wait_until(cx, "caps without df", |_, cx| {
        h.shell.read(cx).store().is_some_and(|s| {
            s.read(cx)
                .info()
                .is_some_and(|i| !i.capabilities.contains(Capabilities::DISK_USAGE))
        })
    });
    let page = volumes_page(&h, cx);
    h.wait_until(cx, "unavailable", |_, cx| {
        matches!(page.read(cx).usage(), UsageState::Unavailable)
    });
    assert!(h.engine.calls_to("disk_usage").is_empty());
    let rows = cx.read(|cx| page.read(cx).rows().to_vec());
    assert!(rows.iter().all(|r| r.size.is_none()), "Size shows —");
    // In use from container mounts (myshop-db-1 mounts myshop_pgdata).
    assert_eq!(
        rows.iter()
            .find(|r| r.name == "myshop_pgdata")
            .map(|r| r.in_use()),
        Some(1)
    );
    cx.update(|cx| page.update(cx, |p, cx| p.set_filter(VolumeFilter::Unused, cx)));
    let unused = cx.read(|cx| page.read(cx).table().read(cx).model(cx).rows().len());
    assert_eq!(unused, 1, "only old-cache is unused");
    h.shutdown();
}

#[gpui_kit::test]
fn vol_004_create_volume_with_n(cx: &mut TestAppContext) {
    let h = start(cx, Setup::default());
    let page = volumes_page(&h, cx);
    let handle = cx.read(|cx| page.read(cx).table().focus_handle(cx));
    h.focus(cx, &handle);
    h.press(cx, "n");
    assert!(h.has_dialog(cx), "N opens Create volume");
    h.draw(cx);
    let dialog = cx
        .read(|cx| page.read(cx).last_create_dialog())
        .expect("dialog");
    // Initial focus = name field (KBD-071).
    let name_focus = cx.read(|cx| dialog.read(cx).name_input().focus_handle(cx));
    assert!(h.is_focused(cx, &name_focus));
    h.type_text(cx, "dk-test");
    // Labels: Mod+Shift+Enter adds a row when focus is in the labels section.
    let labels = cx.read(|cx| dialog.read(cx).labels().clone());
    let add = cx.read(|cx| labels.read(cx).add_focus().clone());
    h.focus(cx, &add);
    h.press(cx, "secondary-shift-enter");
    h.type_text(cx, "team");
    h.press(cx, "tab");
    h.type_text(cx, "core");
    h.press(cx, "secondary-enter");
    h.wait_until(cx, "create called", |_, _| {
        !h.engine.calls_to("create_volume").is_empty()
    });
    assert_eq!(h.engine.calls_to("create_volume")[0].arg, "dk-test");
    h.wait_until(cx, "new volume row", |_, cx| {
        page.read(cx).rows().iter().any(|r| r.name == "dk-test")
    });
    assert!(!h.has_dialog(cx));
    h.shutdown();
}

#[gpui_kit::test]
fn vol_005_delete_and_prune(cx: &mut TestAppContext) {
    let h = start(cx, Setup::default());
    let page = volumes_page(&h, cx);
    h.wait_until(cx, "sizes", |_, cx| {
        matches!(page.read(cx).usage(), UsageState::Known(_))
    });
    let handle = cx.read(|cx| page.read(cx).table().focus_handle(cx));
    h.focus(cx, &handle);
    cx.update(|cx| {
        let t = page.read(cx).table().clone();
        t.update(cx, |t, cx| t.focus_row("old-cache", cx));
    });
    h.press(cx, "delete");
    assert!(h.has_dialog(cx));
    h.draw(cx);
    h.press(cx, "secondary-enter");
    h.wait_until(cx, "old-cache gone", |_, cx| {
        page.read(cx).rows().iter().all(|r| r.name != "old-cache")
    });
    // Prune via the palette command (list::Prune on the Volumes page).
    h.update(cx, |_, window, cx| {
        window.dispatch_action(Box::new(crate::actions::list::Prune), cx)
    });
    assert!(h.has_dialog(cx));
    h.draw(cx);
    h.press(cx, "secondary-enter");
    h.wait_until(cx, "prune called", |_, _| {
        !h.engine.calls_to("prune_volumes").is_empty()
    });
    h.shutdown();
}

#[gpui_kit::test]
fn vol_005_in_use_delete_fails_with_containers(cx: &mut TestAppContext) {
    let h = start(cx, Setup::default());
    let page = volumes_page(&h, cx);
    let handle = cx.read(|cx| page.read(cx).table().focus_handle(cx));
    h.focus(cx, &handle);
    cx.update(|cx| {
        let t = page.read(cx).table().clone();
        t.update(cx, |t, cx| t.focus_row("myshop_pgdata", cx));
    });
    h.press(cx, "delete");
    h.draw(cx);
    h.press(cx, "secondary-enter");
    h.wait_until(cx, "remove attempted", |_, _| {
        !h.engine.calls_to("remove_volume").is_empty()
    });
    h.wait_until(cx, "error toast", |window, cx| {
        use gpui_kit::component::WindowExt;
        !window.notifications(cx).is_empty()
    });
    // The volume is still there.
    let still = cx.read(|cx| {
        page.read(cx)
            .rows()
            .iter()
            .any(|r| r.name == "myshop_pgdata")
    });
    assert!(still);
    h.shutdown();
}

#[gpui_kit::test]
fn vol_010_detail_tabs_and_used_by(cx: &mut TestAppContext) {
    use crate::nav::VolumeTab;
    use crate::pages::volume_detail::VolumeDetailPage;
    let h = start(cx, Setup::default());
    let page = volumes_page(&h, cx);
    let handle = cx.read(|cx| page.read(cx).table().focus_handle(cx));
    h.focus(cx, &handle);
    cx.update(|cx| {
        let t = page.read(cx).table().clone();
        t.update(cx, |t, cx| t.focus_row("myshop_pgdata", cx));
    });
    h.press(cx, "enter");
    let detail = h.goto::<VolumeDetailPage>(
        cx,
        Route::VolumeDetail {
            name: "myshop_pgdata".into(),
            tab: VolumeTab::Overview,
        },
    );
    h.wait_until(cx, "details", |_, cx| {
        detail.read(cx).details().data().is_some()
    });
    let used = cx.read(|cx| detail.read(cx).details().data().map(|d| d.used_by.clone()));
    assert_eq!(used.map(|u| u.len()), Some(1));
    h.press(cx, "ctrl-tab");
    assert_eq!(cx.read(|cx| detail.read(cx).tab()), VolumeTab::UsedBy);
    h.press(cx, "ctrl-tab");
    assert_eq!(cx.read(|cx| detail.read(cx).tab()), VolumeTab::Inspect);
    h.shutdown();
}

// ── NET-* ────────────────────────────────────────────────────────────────────────────

use crate::pages::networks::NetworksPage;

fn networks_page(h: &Harness, cx: &mut TestAppContext) -> gpui_kit::Entity<NetworksPage> {
    h.wait_containers(cx);
    let page = h.goto::<NetworksPage>(cx, Route::Networks);
    h.wait_until(cx, "network rows", |_, cx| {
        !page.read(cx).table().read(cx).model(cx).rows().is_empty()
    });
    page
}

fn network_key(
    page: &gpui_kit::Entity<NetworksPage>,
    name: &str,
    cx: &mut TestAppContext,
) -> String {
    let name = name.to_owned();
    cx.read(|cx| {
        page.read(cx)
            .rows()
            .iter()
            .find(|r| r.name == name)
            .map(|r| r.key.to_string())
            .expect("network row")
    })
}

#[gpui_kit::test]
fn net_002_builtin_not_deletable(cx: &mut TestAppContext) {
    let h = start(cx, Setup::default());
    let page = networks_page(&h, cx);
    let key = network_key(&page, "bridge", cx);
    let handle = cx.read(|cx| page.read(cx).table().focus_handle(cx));
    h.focus(cx, &handle);
    cx.update(|cx| {
        let t = page.read(cx).table().clone();
        t.update(cx, |t, cx| t.focus_row(&key, cx));
    });
    h.press(cx, "delete");
    assert!(!h.has_dialog(cx), "no confirm for a built-in network");
    assert!(h.engine.calls_to("remove_network").is_empty());
    // Counts and subnets: myshop_default has the 4 myshop containers.
    let row = cx.read(|cx| {
        page.read(cx)
            .rows()
            .iter()
            .find(|r| r.name == "myshop_default")
            .cloned()
    });
    let row = row.expect("myshop_default");
    assert_eq!(row.containers, 4);
    assert_eq!(row.subnets, ["172.20.0.0/16"]);
    h.shutdown();
}

#[gpui_kit::test]
fn net_002_delete_custom_network_needs_network_mgmt(cx: &mut TestAppContext) {
    let h = start(cx, Setup::default());
    let page = networks_page(&h, cx);
    let key = network_key(&page, "legacy-net", cx);
    let handle = cx.read(|cx| page.read(cx).table().focus_handle(cx));
    h.focus(cx, &handle);
    cx.update(|cx| {
        let t = page.read(cx).table().clone();
        t.update(cx, |t, cx| t.focus_row(&key, cx));
    });
    h.press(cx, "delete");
    assert!(h.has_dialog(cx));
    h.draw(cx);
    h.press(cx, "secondary-enter");
    h.wait_until(cx, "legacy-net removed", |_, cx| {
        page.read(cx).rows().iter().all(|r| r.name != "legacy-net")
    });
    // Without NETWORK_MGMT nothing is deletable.
    h.engine
        .set_capabilities(Capabilities::all() - Capabilities::NETWORK_MGMT);
    h.update(cx, |_, window, cx| {
        window.dispatch_action(Box::new(crate::actions::Refresh), cx)
    });
    h.wait_until(cx, "caps without network mgmt", |_, cx| {
        h.shell.read(cx).store().is_some_and(|s| {
            s.read(cx)
                .info()
                .is_some_and(|i| !i.capabilities.contains(Capabilities::NETWORK_MGMT))
        })
    });
    let key = network_key(&page, "monitoring_default", cx);
    h.focus(cx, &handle);
    cx.update(|cx| {
        let t = page.read(cx).table().clone();
        t.update(cx, |t, cx| t.focus_row(&key, cx));
    });
    h.press(cx, "delete");
    assert!(!h.has_dialog(cx));
    h.shutdown();
}

#[gpui_kit::test]
fn net_004_prune_unused_networks(cx: &mut TestAppContext) {
    let h = start(cx, Setup::default());
    let _page = networks_page(&h, cx);
    h.update(cx, |_, window, cx| {
        window.dispatch_action(Box::new(crate::actions::list::Prune), cx)
    });
    assert!(h.has_dialog(cx));
    h.draw(cx);
    h.press(cx, "secondary-enter");
    h.wait_until(cx, "prune_networks", |_, _| {
        !h.engine.calls_to("prune_networks").is_empty()
    });
    h.shutdown();
}

#[gpui_kit::test]
fn net_003_detail_tabs_and_container_links(cx: &mut TestAppContext) {
    use crate::nav::NetworkTab;
    use crate::pages::network_detail::NetworkDetailPage;
    let h = start(cx, Setup::default());
    let page = networks_page(&h, cx);
    let key = network_key(&page, "myshop_default", cx);
    let handle = cx.read(|cx| page.read(cx).table().focus_handle(cx));
    h.focus(cx, &handle);
    cx.update(|cx| {
        let t = page.read(cx).table().clone();
        t.update(cx, |t, cx| t.focus_row(&key, cx));
    });
    h.press(cx, "enter");
    let detail = h.goto::<NetworkDetailPage>(
        cx,
        Route::NetworkDetail {
            id: key,
            tab: NetworkTab::Overview,
        },
    );
    h.wait_until(cx, "details", |_, cx| {
        detail.read(cx).details().data().is_some()
    });
    h.press(cx, "ctrl-tab");
    assert_eq!(cx.read(|cx| detail.read(cx).tab()), NetworkTab::Containers);
    let n = cx.read(|cx| detail.read(cx).details().data().map(|d| d.containers.len()));
    assert_eq!(n, Some(4));
    h.shutdown();
}

#[gpui_kit::test]
fn net_001_hidden_networks_page(cx: &mut TestAppContext) {
    let mut config = dk_hub::Config::default();
    config.general.show_networks_page = false;
    let h = start(
        cx,
        Setup {
            config,
            ..Default::default()
        },
    );
    h.wait_containers(cx);
    h.focus_table(cx);
    h.press(cx, "secondary-4");
    assert_eq!(h.read(cx, |s, _, _| s.route().clone()), Route::Containers);
    h.shutdown();
}

// ── cross-links (spec 30 §2) and the palette (KBD-020) ──────────────────────────────

#[gpui_kit::test]
fn kbd_020_palette_go_to_volume_opens_detail(cx: &mut TestAppContext) {
    use crate::pages::volume_detail::VolumeDetailPage;
    let h = start(cx, Setup::default());
    h.wait_containers(cx);
    h.focus_table(cx);
    h.press(cx, "secondary-shift-p");
    h.type_text(cx, "Go to volume: scratch");
    cx.run_until_parked();
    h.press(cx, "enter");
    h.wait_until(cx, "volume detail mounted", |_, cx| {
        matches!(h.shell.read(cx).route(), Route::VolumeDetail { name, .. } if name == "scratch")
    });
    let d = h
        .dyn_page::<VolumeDetailPage>(cx)
        .expect("volume detail page");
    h.wait_until(cx, "details", |_, cx| d.read(cx).details().data().is_some());
    h.shutdown();
}

#[gpui_kit::test]
fn spec30_cross_links_resolve_names(cx: &mut TestAppContext) {
    use crate::pages::network_detail::NetworkDetailPage;
    let h = start(cx, Setup::default());
    h.wait_containers(cx);
    // Container detail links networks by name; the detail page resolves it.
    let d = h.goto::<NetworkDetailPage>(cx, crate::ui::links::network_route("myshop_default"));
    h.wait_until(cx, "details", |_, cx| d.read(cx).details().data().is_some());
    // Image links use the container's image id.
    let id = crate::demo::image_id("redis:7");
    let img = h.goto::<crate::pages::image_detail::ImageDetailPage>(
        cx,
        crate::ui::links::image_route(&id),
    );
    h.wait_until(cx, "image details", |_, cx| {
        img.read(cx).details().data().is_some()
    });
    h.shutdown();
}

#[gpui_kit::test]
fn img_010_used_by_enter_follows_link(cx: &mut TestAppContext) {
    use crate::pages::image_detail::ImageDetailPage;
    let h = start(cx, Setup::default());
    h.wait_containers(cx);
    let id = crate::demo::image_id("redis:7");
    let detail = h.goto::<ImageDetailPage>(
        cx,
        Route::ImageDetail {
            id,
            tab: ImageTab::UsedBy,
        },
    );
    h.wait_until(cx, "details", |_, cx| {
        detail.read(cx).details().data().is_some()
    });
    h.draw(cx);
    // Tab from the tab bar (primary focus) into the Used by list, Enter follows the link.
    h.press(cx, "tab");
    h.press(cx, "enter");
    h.wait_until(cx, "container detail", |_, cx| {
        matches!(h.shell.read(cx).route(), Route::ContainerDetail { id, .. } if *id == crate::testing::id_of("redis"))
    });
    h.shutdown();
}

#[gpui_kit::test]
fn kbd_008_g_u_n_ignored_in_search_inputs(cx: &mut TestAppContext) {
    let h = start(cx, Setup::default());
    let page = images_page(&h, cx);
    focus_images_table(&h, &page, cx);
    h.press(cx, "secondary-f");
    h.type_text(cx, "gun");
    h.draw(cx);
    assert!(
        !h.has_dialog(cx),
        "single letters don't fire in the search input"
    );
    h.shutdown();
}

#[gpui_kit::test]
fn kbd_022_reference_lists_m6_letters(cx: &mut TestAppContext) {
    let h = start(cx, Setup::default());
    h.wait_containers(cx);
    let rows = crate::keymap::reference_rows(crate::keymap::Os::current());
    for key in ["U", "G", "N"] {
        assert!(rows.iter().any(|r| r.keys == key), "{key} listed");
    }
    h.shutdown();
}

#[gpui_kit::test]
fn img_004_pull_status_line_without_pull_progress(cx: &mut TestAppContext) {
    let h = start(cx, Setup::default());
    h.engine
        .set_capabilities(Capabilities::all() - Capabilities::PULL_PROGRESS);
    h.update(cx, |_, window, cx| {
        window.dispatch_action(Box::new(crate::actions::Refresh), cx)
    });
    h.wait_until(cx, "caps without pull progress", |_, cx| {
        h.shell.read(cx).store().is_some_and(|s| {
            s.read(cx)
                .info()
                .is_some_and(|i| !i.capabilities.contains(Capabilities::PULL_PROGRESS))
        })
    });
    let page = images_page(&h, cx);
    focus_images_table(&h, &page, cx);
    h.press(cx, "g");
    h.draw(cx);
    h.type_text(cx, "busybox:1.37");
    h.press(cx, "enter");
    h.wait_until(cx, "pull finished", |_, cx| {
        PullManager::try_global(cx).is_some_and(|m| {
            m.read(cx)
                .finished()
                .iter()
                .any(|p| p.reference == "busybox:1.37" && !p.structured)
        })
    });
    h.shutdown();
}

#[gpui_kit::test]
fn img_004_pull_cancel_drops_the_stream(cx: &mut TestAppContext) {
    let h = start(cx, Setup::default());
    let page = images_page(&h, cx);
    let _ = page;
    // Start a pull directly and cancel before the fake's stream is consumed.
    let id = cx.update(|cx| {
        let manager = PullManager::global(cx);
        let window = h.any_window();
        manager.update(cx, |m, cx| {
            let id = m.start(
                dk_core::EngineId::new("demo"),
                "nginx:1.28".into(),
                Capabilities::all(),
                window,
                |_, _| {},
                cx,
            );
            m.cancel(id, cx);
            id
        })
    });
    h.draw(cx);
    let cancelled = cx.read(|cx| {
        PullManager::try_global(cx).is_some_and(|m| {
            m.read(cx)
                .finished()
                .iter()
                .any(|p| p.id == id && p.phase == PullPhase::Cancelled)
        })
    });
    assert!(cancelled);
    assert!(
        cx.read(|cx| PullManager::try_global(cx).is_some_and(|m| m.read(cx).running().is_empty()))
    );
    h.shutdown();
}
