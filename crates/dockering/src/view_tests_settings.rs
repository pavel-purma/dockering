//! Settings view tests (SET-*, ENG-104/105/109/110, KBD-071/072/075/092, SHL-011/024).
//! Real `HubHandle` + `FakeEngine`; test names carry requirement ids.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use dk_core::fake::{FakeEngine, FakeFactory};
use dk_core::{
    DiscoveredEngine, Engine, EngineConfig, EngineConfigSchema, EngineEndpoint, EngineError,
    EngineFactory, EngineId, EngineKind, EngineOrigin, EngineResult,
};
use dk_hub::config::ThemeMode;
use gpui_kit::component::ActiveTheme;
use gpui_kit::{AppContext as _, Entity, Focusable, TestAppContext};

use crate::actions::settings::{AddEngine, EngineOp, EngineOpKind};
use crate::nav::{Page, Route, SettingsSection};
use crate::pages::settings::controls::{BoolKey, Key};
use crate::pages::settings::{AddEngineDialog, SettingsPage};
use crate::shell::ShellPage;
use crate::shell::app_shell::SidebarEntry;
use crate::state::AppState;
use crate::strings as s;
use crate::testing::{Harness, Setup, start, start_with_factories};

// ── harness helpers ──────────────────────────────────────────────────────────────────

fn settings_page(h: &Harness, cx: &mut TestAppContext) -> Option<Entity<SettingsPage>> {
    h.read(cx, |s, _, _| match s.page() {
        ShellPage::Settings(p) => Some(p.clone()),
        _ => None,
    })
}

/// Opens Settings at `section` (as the sidebar/palette would) and waits for the page.
fn open(h: &Harness, cx: &mut TestAppContext, section: SettingsSection) -> Entity<SettingsPage> {
    h.update(cx, |_, window, cx| {
        window.dispatch_action(
            Box::new(crate::actions::Navigate {
                route: Route::Settings { section },
            }),
            cx,
        )
    });
    h.wait_until(cx, "settings page", |_, cx| {
        matches!(h.shell.read(cx).page(), ShellPage::Settings(p) if p.read(cx).section() == section)
    });
    settings_page(h, cx).expect("settings page")
}

/// The shell sidebar: the settings section nav (SET-080).
fn sidebar(h: &Harness, cx: &mut TestAppContext) -> gpui_kit::FocusHandle {
    h.read(cx, |s, _, _| s.sidebar_focus().clone())
}

fn config(cx: &mut TestAppContext) -> dk_hub::Config {
    cx.read(|cx| AppState::config(cx).clone())
}

fn hub_config(h: &Harness) -> dk_hub::Config {
    h.hub.config().get()
}

/// Runs an engine row command the way its button does: dispatched from the settings page's
/// root (focus may be in the shell sidebar, outside the page, SET-080).
fn engine_op(h: &Harness, cx: &mut TestAppContext, id: &str, op: EngineOpKind) {
    let id: gpui_kit::SharedString = id.to_owned().into();
    let page = settings_page(h, cx)
        .expect("settings page")
        .read_with(cx, |p, _| p.focus().clone());
    h.update(cx, |_, window, cx| {
        page.dispatch_action(&EngineOp { id, op }, window, cx)
    });
    h.draw(cx);
}

/// A factory that publishes the real Docker schemas (Unix socket, Named pipe, TCP, TCP+TLS)
/// and connects TCP `10.0.0.9` to a fake engine; any other TCP host is refused with a hint.
/// Other endpoints go to the fake factory. (`EngineFactory` is an `async_trait`; the boxed
/// signatures are written out so `dockering` needs no extra dependency.)
struct SchemaFactory {
    ok: Arc<FakeEngine>,
    fake: Arc<FakeFactory>,
}

const GOOD_HOST: &str = "10.0.0.9";
const REFUSED_HINT: &str = "Is the Docker daemon listening on TCP?";

type Boxed<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

impl EngineFactory for SchemaFactory {
    fn kind(&self) -> EngineKind {
        EngineKind::Docker
    }
    fn handles(&self, endpoint: &EngineEndpoint) -> bool {
        matches!(endpoint, EngineEndpoint::Tcp { .. })
    }
    fn discover<'a, 'b>(&'a self) -> Boxed<'b, Vec<DiscoveredEngine>>
    where
        'a: 'b,
        Self: 'b,
    {
        Box::pin(async { Vec::new() })
    }
    fn connect<'a, 'c, 'b>(
        &'a self,
        cfg: &'c EngineConfig,
    ) -> Boxed<'b, EngineResult<Arc<dyn Engine>>>
    where
        'a: 'b,
        'c: 'b,
        Self: 'b,
    {
        Box::pin(async move {
            match &cfg.endpoint {
                EngineEndpoint::Tcp { host, .. } if host == GOOD_HOST => {
                    Ok(self.ok.clone() as Arc<dyn Engine>)
                }
                EngineEndpoint::Tcp { .. } => Err(EngineError::unreachable_with_hint(
                    "connection refused",
                    REFUSED_HINT,
                )),
                _ => self.fake.connect(cfg).await,
            }
        })
    }
    fn config_schema(&self) -> Vec<EngineConfigSchema> {
        dk_engine_schemas()
    }
}

/// The Docker factory's schemas (the dialog renders whatever factories publish).
fn dk_engine_schemas() -> Vec<EngineConfigSchema> {
    dk_hub::default_factories()
        .into_iter()
        .flat_map(|f| f.config_schema())
        .filter(|s| s.kind == EngineKind::Docker)
        .collect()
}

fn start_with_schemas(cx: &mut TestAppContext) -> Harness {
    let schema = Arc::new(SchemaFactory {
        ok: FakeEngine::new("remote-ok"),
        fake: FakeFactory::new(),
    });
    start_with_factories(cx, Setup::default(), vec![schema as Arc<dyn EngineFactory>])
}

fn open_add_dialog(
    h: &Harness,
    cx: &mut TestAppContext,
) -> (Entity<SettingsPage>, Entity<AddEngineDialog>) {
    h.wait_containers(cx);
    let page = open(h, cx, SettingsSection::Engines);
    h.update(cx, |_, window, cx| {
        window.dispatch_action(Box::new(AddEngine), cx)
    });
    h.draw(cx);
    assert!(h.has_dialog(cx), "Add engine opens a dialog");
    let dialog = cx
        .read(|cx| page.read(cx).add_dialog().cloned())
        .expect("dialog entity");
    (page, dialog)
}

fn set_input(
    h: &Harness,
    cx: &mut TestAppContext,
    input: &Entity<gpui_kit::component::input::InputState>,
    text: &str,
) {
    let input = input.clone();
    let text = text.to_owned();
    cx.update_window(h.any_window(), |_, window, cx| {
        input.update(cx, |i, cx| {
            i.focus(window, cx);
        });
    })
    .unwrap();
    h.draw(cx);
    h.press(cx, "secondary-a");
    h.type_text(cx, &text);
    h.draw(cx);
}

// ── frame (item 1) ───────────────────────────────────────────────────────────────────

#[gpui_kit::test]
fn kbd_024_mod_comma_opens_settings_with_nav_focus(cx: &mut TestAppContext) {
    let h = start(cx, Setup::default());
    h.wait_containers(cx);
    h.focus_table(cx);
    h.press(cx, "secondary-,");
    assert_eq!(
        h.read(cx, |s, _, _| s.route().clone()),
        Route::Settings {
            section: SettingsSection::General
        }
    );
    // SET-080: the section nav is the shell sidebar, cursor on the active section.
    settings_page(&h, cx).expect("settings mounted");
    let nav = sidebar(&h, cx);
    h.wait_until(cx, "nav focused", |window, _| nav.is_focused(window));
    assert_eq!(
        h.read(cx, |s, _, cx| s.sidebar_entries(cx)[s.sidebar_cursor()]
            .clone()),
        SidebarEntry::Section(SettingsSection::General)
    );
    h.shutdown();
}

#[gpui_kit::test]
fn set_080_settings_sidebar_back_returns_to_app_page(cx: &mut TestAppContext) {
    let h = start(cx, Setup::default());
    h.wait_containers(cx);
    h.update(cx, |_, window, cx| {
        window.dispatch_action(Box::new(crate::actions::GoImages), cx)
    });
    h.wait_until(cx, "images", |_, cx| {
        *h.shell.read(cx).route() == Route::Images
    });
    open(&h, cx, SettingsSection::Logs);
    // Settings mode: Back to the last app page, then every section (same nav component).
    let entries = h.read(cx, |s, _, cx| s.sidebar_entries(cx));
    assert_eq!(entries[0], SidebarEntry::Back(Route::Images));
    assert_eq!(
        entries[1..].to_vec(),
        SettingsSection::ALL
            .iter()
            .map(|s| SidebarEntry::Section(*s))
            .collect::<Vec<_>>()
    );
    let nav = sidebar(&h, cx);
    h.focus(cx, &nav);
    h.press(cx, "home");
    assert_eq!(h.read(cx, |s, _, _| s.sidebar_cursor()), 0);
    // Home lands on Back: the route stays on the section.
    assert_eq!(
        h.read(cx, |s, _, _| s.route().clone()),
        Route::Settings {
            section: SettingsSection::Logs
        }
    );
    h.press(cx, "enter");
    h.wait_until(cx, "back on images", |_, cx| {
        *h.shell.read(cx).route() == Route::Images
    });
    // Outside Settings the sidebar lists the main pages again.
    let entries = h.read(cx, |s, _, cx| s.sidebar_entries(cx));
    assert!(entries.iter().all(|e| matches!(e, SidebarEntry::Page(_))));
    h.shutdown();
}

#[gpui_kit::test]
fn set_route_section_arrows_and_back_forward(cx: &mut TestAppContext) {
    let h = start(cx, Setup::default());
    h.wait_containers(cx);
    let page = open(&h, cx, SettingsSection::General);
    let nav = sidebar(&h, cx);
    h.focus(cx, &nav);
    // Arrows move between sections in place (no history entry).
    h.press(cx, "down");
    assert_eq!(
        cx.read(|cx| page.read(cx).section()),
        SettingsSection::Engines
    );
    h.wait_until(cx, "route follows", |_, cx| {
        *h.shell.read(cx).route()
            == Route::Settings {
                section: SettingsSection::Engines,
            }
    });
    h.press(cx, "end");
    assert_eq!(
        cx.read(|cx| page.read(cx).section()),
        SettingsSection::Keyboard
    );
    // Home lands on *Back* (SET-080): the section stays; the next arrow shows General.
    h.press(cx, "home");
    assert_eq!(
        cx.read(|cx| page.read(cx).section()),
        SettingsSection::Keyboard
    );
    h.press(cx, "down");
    assert_eq!(
        cx.read(|cx| page.read(cx).section()),
        SettingsSection::General
    );
    // Deep links push history; Back returns to the previous section, same page entity.
    open(&h, cx, SettingsSection::Logs);
    open(&h, cx, SettingsSection::Stats);
    let (back, forward) = if cfg!(target_os = "macos") {
        ("cmd-[", "cmd-]")
    } else {
        ("alt-left", "alt-right")
    };
    h.press(cx, back);
    h.wait_until(cx, "back to logs", |_, cx| {
        *h.shell.read(cx).route()
            == Route::Settings {
                section: SettingsSection::Logs,
            }
    });
    let same = settings_page(&h, cx).expect("page");
    assert_eq!(
        same.entity_id(),
        page.entity_id(),
        "one entity across sections"
    );
    assert_eq!(cx.read(|cx| same.read(cx).section()), SettingsSection::Logs);
    h.press(cx, forward);
    assert_eq!(
        cx.read(|cx| same.read(cx).section()),
        SettingsSection::Stats
    );
    h.shutdown();
}

// ── General / Containers / Logs / Terminal / Stats (item 2) ──────────────────────────

#[gpui_kit::test]
fn set_001_theme_applies_live_and_persists(cx: &mut TestAppContext) {
    let h = start(cx, Setup::default());
    h.wait_containers(cx);
    let page = open(&h, cx, SettingsSection::General);
    let dark_before = cx.read(|cx| cx.theme().is_dark());
    let target = if dark_before { 1 } else { 2 }; // Light : Dark
    h.update(cx, |_, window, cx| {
        page.update(cx, |p, cx| p.set_choice(Key::Theme, target, window, cx))
    });
    assert_ne!(
        cx.read(|cx| cx.theme().is_dark()),
        dark_before,
        "applied live"
    );
    let want = if dark_before {
        ThemeMode::Light
    } else {
        ThemeMode::Dark
    };
    assert_eq!(config(cx).general.theme, want);
    assert_eq!(
        hub_config(&h).general.theme,
        want,
        "persisted through ConfigHandle"
    );
    // The title-bar toggle updates the select (one source of truth).
    h.press(cx, "secondary-shift-l");
    h.draw(cx);
    let shown = cx.read(|cx| {
        page.read(cx)
            .select(Key::Theme)
            .and_then(|s| s.read(cx).selected_value().copied())
    });
    let expected = if dark_before {
        s::THEME_DARK
    } else {
        s::THEME_LIGHT
    };
    assert_eq!(shown, Some(expected));
    h.shutdown();
}

#[gpui_kit::test]
fn set_001_networks_page_hidden_from_sidebar(cx: &mut TestAppContext) {
    let h = start(cx, Setup::default());
    h.wait_containers(cx);
    let page = open(&h, cx, SettingsSection::General);
    h.update(cx, |_, _, cx| {
        page.update(cx, |p, cx| p.set_bool(BoolKey::ShowNetworks, false, cx))
    });
    h.draw(cx);
    assert!(!hub_config(&h).general.show_networks_page);
    let pages = h.read(cx, |s, _, cx| s.sidebar_pages(cx));
    assert!(!pages.contains(&Page::Networks), "{pages:?}");
    // Mod+4 is a no-op while hidden.
    h.press(cx, "secondary-4");
    assert!(matches!(
        h.read(cx, |s, _, _| s.route().clone()),
        Route::Settings { .. }
    ));
    h.update(cx, |_, _, cx| {
        page.update(cx, |p, cx| p.set_bool(BoolKey::ShowNetworks, true, cx))
    });
    let pages = h.read(cx, |s, _, cx| s.sidebar_pages(cx));
    assert!(pages.contains(&Page::Networks));
    h.shutdown();
}

#[gpui_kit::test]
fn set_001_start_page_and_confirm_stopped_persist(cx: &mut TestAppContext) {
    let h = start(cx, Setup::default());
    h.wait_containers(cx);
    let page = open(&h, cx, SettingsSection::General);
    h.update(cx, |_, window, cx| {
        page.update(cx, |p, cx| {
            p.set_choice(Key::StartPage, 2, window, cx);
            p.set_bool(BoolKey::ConfirmStopped, false, cx);
        })
    });
    let c = hub_config(&h);
    assert_eq!(c.general.start_page, dk_hub::config::StartPage::Volumes);
    assert!(!c.general.confirm_delete_stopped);
    assert!(!cx.read(crate::ui::confirm::should_confirm_stopped_delete));
    h.shutdown();
}

#[gpui_kit::test]
fn set_020_group_by_default_affects_containers_page(cx: &mut TestAppContext) {
    let h = start(cx, Setup::default());
    h.wait_containers(cx);
    let page = open(&h, cx, SettingsSection::Containers);
    // Label grouping without a key: inline error, nothing saved.
    h.update(cx, |_, window, cx| {
        page.update(cx, |p, cx| p.set_choice(Key::GroupBy, 2, window, cx))
    });
    assert_eq!(
        cx.read(|cx| page.read(cx).error(Key::LabelKey).cloned()),
        Some(s::SET_LABEL_KEY_REQUIRED.into())
    );
    assert_eq!(
        hub_config(&h).containers.group_by,
        dk_core::grouping::GroupBy::Compose
    );
    // Typing the key saves label grouping.
    let input = cx
        .read(|cx| page.read(cx).input(Key::LabelKey).cloned())
        .unwrap();
    set_input(&h, cx, &input, "app");
    assert_eq!(
        hub_config(&h).containers.group_by,
        dk_core::grouping::GroupBy::Label("app".into())
    );
    assert!(cx.read(|cx| page.read(cx).error(Key::LabelKey).is_none()));
    // None: the containers page opens flat.
    h.update(cx, |_, window, cx| {
        page.update(cx, |p, cx| p.set_choice(Key::GroupBy, 1, window, cx))
    });
    h.press(cx, "secondary-1");
    let containers = h.wait_containers(cx);
    assert_eq!(
        cx.read(|cx| containers.read(cx).group_by().clone()),
        dk_core::grouping::GroupBy::None
    );
    h.wait_until(cx, "flat rows", |_, cx| {
        containers
            .read(cx)
            .table()
            .read(cx)
            .model(cx)
            .rows()
            .iter()
            .all(|r| !r.is_group())
    });
    h.shutdown();
}

/// The mounted Containers page follows Settings while it lives (no remount needed).
#[gpui_kit::test]
fn set_020_mounted_containers_page_follows_settings(cx: &mut TestAppContext) {
    let h = start(cx, Setup::default());
    let containers = h.wait_containers(cx);
    h.update(cx, |_, _, cx| {
        AppState::update_config(cx, |c| {
            c.containers.group_by = dk_core::grouping::GroupBy::None;
            c.containers.show_cpu_mem_columns = true;
        })
    });
    h.wait_until(cx, "flat + stats columns", |_, cx| {
        let p = containers.read(cx);
        p.table().read(cx).delegate(cx).show_stats
            && p.table()
                .read(cx)
                .model(cx)
                .rows()
                .iter()
                .all(|r| !r.is_group())
    });
    h.shutdown();
}

#[gpui_kit::test]
fn set_020_cpu_columns_toggle_live(cx: &mut TestAppContext) {
    let h = start(cx, Setup::default());
    let containers = h.wait_containers(cx);
    let cols = |cx: &mut TestAppContext| {
        cx.read(|cx| containers.read(cx).table().read(cx).delegate(cx).show_stats)
    };
    assert!(!cols(cx));
    let page = open(&h, cx, SettingsSection::Containers);
    h.update(cx, |_, _, cx| {
        page.update(cx, |p, cx| p.set_bool(BoolKey::ShowStats, true, cx))
    });
    assert!(hub_config(&h).containers.show_cpu_mem_columns);
    h.press(cx, "secondary-1");
    let containers = h.wait_containers(cx);
    h.wait_until(cx, "stats columns", |_, cx| {
        containers.read(cx).table().read(cx).delegate(cx).show_stats
    });
    h.shutdown();
}

#[gpui_kit::test]
fn set_020_polling_interval_validated_inline(cx: &mut TestAppContext) {
    let h = start(cx, Setup::default());
    h.wait_containers(cx);
    let page = open(&h, cx, SettingsSection::Containers);
    let input = cx
        .read(|cx| page.read(cx).input(Key::Polling).cloned())
        .unwrap();
    set_input(&h, cx, &input, "0");
    assert!(
        cx.read(|cx| page.read(cx).error(Key::Polling).is_some()),
        "0 s is rejected inline"
    );
    assert_eq!(hub_config(&h).containers.polling_interval_s, 5, "not saved");
    set_input(&h, cx, &input, "12");
    assert!(cx.read(|cx| page.read(cx).error(Key::Polling).is_none()));
    assert_eq!(hub_config(&h).containers.polling_interval_s, 12);
    h.shutdown();
}

#[gpui_kit::test]
fn set_030_040_050_numbers_text_and_switches_persist(cx: &mut TestAppContext) {
    let h = start(cx, Setup::default());
    h.wait_containers(cx);
    let page = open(&h, cx, SettingsSection::Logs);
    let tail = cx
        .read(|cx| page.read(cx).input(Key::LogsTail).cloned())
        .unwrap();
    set_input(&h, cx, &tail, "250");
    h.update(cx, |_, _, cx| {
        page.update(cx, |p, cx| {
            p.set_bool(BoolKey::LogsTimestamps, true, cx);
            p.set_bool(BoolKey::LogsWrap, true, cx);
        })
    });
    let page = open(&h, cx, SettingsSection::Terminal);
    let size = cx
        .read(|cx| page.read(cx).input(Key::FontSize).cloned())
        .unwrap();
    set_input(&h, cx, &size, "14.5");
    let ext = cx
        .read(|cx| page.read(cx).input(Key::External).cloned())
        .unwrap();
    set_input(&h, cx, &ext, "wt.exe");
    assert_eq!(
        cx.read(|cx| page.read(cx).error(Key::External).cloned()),
        Some(s::SET_EXTERNAL_NEEDS_CMD.into()),
        "TRM-009: {{cmd}} is required"
    );
    set_input(&h, cx, &ext, "wt.exe {cmd}");
    let page = open(&h, cx, SettingsSection::Stats);
    let hist = cx
        .read(|cx| page.read(cx).input(Key::StatsHistory).cloned())
        .unwrap();
    set_input(&h, cx, &hist, "90");
    assert!(
        cx.read(|cx| page.read(cx).error(Key::StatsHistory).is_some()),
        "max 60 min"
    );
    set_input(&h, cx, &hist, "30");
    h.update(cx, |_, _, cx| {
        page.update(cx, |p, cx| p.set_bool(BoolKey::AllCores, true, cx))
    });
    let c = hub_config(&h);
    assert_eq!(c.logs.initial_tail, 250);
    assert!(c.logs.timestamps && c.logs.wrap);
    assert!((c.terminal.font_size - 14.5).abs() < 1e-4);
    assert_eq!(c.terminal.external_terminal, "wt.exe {cmd}");
    assert_eq!(c.stats.history_minutes, 30);
    assert!(c.stats.cpu_relative_to_all_cores);
    h.shutdown();
}

// ── Engines (item 3) ─────────────────────────────────────────────────────────────────

fn two_engine_setup(hidden_other: bool) -> Setup {
    let mut config = dk_hub::Config::default();
    if hidden_other {
        config.engines.entries.push(EngineConfig {
            id: EngineId::new("other"),
            name: "Fake other".into(),
            endpoint: EngineEndpoint::UnixSocket {
                path: "/fake/other.sock".into(),
            },
            origin: EngineOrigin::Discovered,
            enabled: true,
            hidden: true,
        });
    }
    Setup {
        extra_engines: vec![FakeEngine::new("other")],
        config,
        ..Default::default()
    }
}

#[gpui_kit::test]
fn eng_104_engines_section_lists_hidden_engines(cx: &mut TestAppContext) {
    let h = start(cx, two_engine_setup(true));
    h.wait_containers(cx);
    h.wait_until(cx, "two engines", |_, cx| {
        h.shell.read(cx).engines().read(cx).engines().len() == 2
    });
    let page = open(&h, cx, SettingsSection::Engines);
    h.draw(cx);
    let names = cx.read(|cx| {
        ["demo", "other"]
            .iter()
            .map(|id| {
                page.read(cx)
                    .engines_ui()
                    .name_input(&EngineId::new(*id))
                    .map(|i| i.read(cx).value().to_string())
            })
            .collect::<Vec<_>>()
    });
    assert_eq!(
        names,
        vec![Some("Fake demo".into()), Some("Fake other".into())]
    );
    // Unhide (discovered engines can't be removed, only hidden, ENG-104).
    engine_op(&h, cx, "other", EngineOpKind::ToggleHidden);
    h.wait_until(cx, "unhidden", |_, cx| {
        h.shell
            .read(cx)
            .engines()
            .read(cx)
            .get(&EngineId::new("other"))
            .is_some_and(|e| !e.config.hidden)
    });
    h.shutdown();
}

#[gpui_kit::test]
fn eng_025_enable_toggle_calls_hub_update(cx: &mut TestAppContext) {
    let h = start(cx, two_engine_setup(false));
    h.wait_containers(cx);
    open(&h, cx, SettingsSection::Engines);
    engine_op(&h, cx, "other", EngineOpKind::ToggleEnabled);
    h.wait_until(cx, "disabled", |_, cx| {
        h.shell
            .read(cx)
            .engines()
            .read(cx)
            .get(&EngineId::new("other"))
            .is_some_and(|e| !e.config.enabled && e.state == dk_core::EngineState::Disabled)
    });
    let stored = hub_config(&h).engines.entries;
    assert!(
        stored
            .iter()
            .any(|e| e.id.as_str() == "other" && !e.enabled)
    );
    h.shutdown();
}

#[gpui_kit::test]
fn eng_104_rename_inline_updates_engine(cx: &mut TestAppContext) {
    let h = start(cx, two_engine_setup(false));
    h.wait_containers(cx);
    let page = open(&h, cx, SettingsSection::Engines);
    h.draw(cx);
    let input = cx
        .read(|cx| {
            page.read(cx)
                .engines_ui()
                .name_input(&EngineId::new("other"))
                .cloned()
        })
        .expect("name input");
    set_input(&h, cx, &input, "Build server");
    h.press(cx, "enter");
    h.wait_until(cx, "renamed", |_, cx| {
        h.shell
            .read(cx)
            .engines()
            .read(cx)
            .get(&EngineId::new("other"))
            .is_some_and(|e| e.config.name == "Build server")
    });
    h.shutdown();
}

#[gpui_kit::test]
fn eng_104_test_connection_success_and_failure(cx: &mut TestAppContext) {
    let h = start(
        cx,
        Setup {
            extra_engines: vec![FakeEngine::new("other")],
            failing: vec![(
                "other".into(),
                EngineError::unreachable_with_hint("connection refused", "Is Docker running?"),
            )],
            ..Default::default()
        },
    );
    h.wait_containers(cx);
    let page = open(&h, cx, SettingsSection::Engines);
    engine_op(&h, cx, "demo", EngineOpKind::Test);
    h.wait_until(cx, "demo test ok", |_, cx| {
        matches!(
            page.read(cx)
                .engines_ui()
                .test_state(&EngineId::new("demo")),
            Some(crate::pages::settings::engines::TestState::Ok(_))
        )
    });
    let ok = cx.read(|cx| {
        page.read(cx)
            .engines_ui()
            .test_state(&EngineId::new("demo"))
            .cloned()
    });
    match ok {
        Some(crate::pages::settings::engines::TestState::Ok(msg)) => {
            assert!(msg.contains("27.3.1"), "{msg}")
        }
        other => panic!("{other:?}"),
    }
    engine_op(&h, cx, "other", EngineOpKind::Test);
    h.wait_until(cx, "other test failed", |_, cx| {
        matches!(
            page.read(cx)
                .engines_ui()
                .test_state(&EngineId::new("other")),
            Some(crate::pages::settings::engines::TestState::Failed { .. })
        )
    });
    let failed = cx.read(|cx| {
        page.read(cx)
            .engines_ui()
            .test_state(&EngineId::new("other"))
            .cloned()
    });
    match failed {
        Some(crate::pages::settings::engines::TestState::Failed { message, hint }) => {
            assert!(message.contains("connection refused"));
            assert_eq!(hint.as_deref(), Some("Is Docker running?"), "ENG-107 hint");
        }
        other => panic!("{other:?}"),
    }
    h.shutdown();
}

#[gpui_kit::test]
fn eng_109_show_all_toggles_persist(cx: &mut TestAppContext) {
    let h = start(cx, Setup::default());
    h.wait_containers(cx);
    let page = open(&h, cx, SettingsSection::Engines);
    h.update(cx, |_, _, cx| {
        page.update(cx, |p, cx| {
            p.set_bool(BoolKey::ShowAllDistros, true, cx);
            p.set_bool(BoolKey::ShowAllSessions, true, cx);
        })
    });
    let c = hub_config(&h);
    assert!(c.engines.show_all_wsl_distros && c.engines.show_all_wslc_sessions);
    h.shutdown();
}

#[gpui_kit::test]
fn eng_009_unmerge_adds_id_to_config(cx: &mut TestAppContext) {
    let h = start(cx, Setup::default());
    h.wait_containers(cx);
    open(&h, cx, SettingsSection::Engines);
    engine_op(&h, cx, "wsl-ubuntu", EngineOpKind::Unmerge);
    assert_eq!(
        hub_config(&h).engines.unmerged,
        vec![EngineId::new("wsl-ubuntu")]
    );
    // Idempotent.
    engine_op(&h, cx, "wsl-ubuntu", EngineOpKind::Unmerge);
    assert_eq!(hub_config(&h).engines.unmerged.len(), 1);
    h.shutdown();
}

// ── Add engine (item 4) ──────────────────────────────────────────────────────────────

#[gpui_kit::test]
fn eng_105_dialog_renders_schema_fields_with_first_field_focus(cx: &mut TestAppContext) {
    let h = start_with_schemas(cx);
    let (_page, dialog) = open_add_dialog(&h, cx);
    let labels: Vec<String> = cx.read(|cx| {
        dialog
            .read(cx)
            .schemas()
            .iter()
            .map(|s| s.label.clone())
            .collect()
    });
    assert_eq!(
        labels,
        vec!["Unix socket", "Named pipe", "TCP", "TCP + TLS"],
        "Fake schema has no mapping"
    );
    h.update(cx, |_, window, cx| {
        dialog.update(cx, |d, cx| d.set_kind("TCP + TLS", window, cx))
    });
    assert_eq!(
        cx.read(|cx| dialog.read(cx).field_keys()),
        vec!["host", "port", "ca", "cert", "key", "verify"]
    );
    // KBD-071: the first field (connection type) has focus.
    let kind_focus = cx.read(|cx| dialog.read(cx).kind_select().focus_handle(cx));
    assert!(h.is_focused(cx, &kind_focus));
    h.shutdown();
}

#[gpui_kit::test]
fn eng_105_add_engine_test_then_save(cx: &mut TestAppContext) {
    let h = start_with_schemas(cx);
    let (page, dialog) = open_add_dialog(&h, cx);
    h.update(cx, |_, window, cx| {
        dialog.update(cx, |d, cx| d.set_kind("TCP", window, cx))
    });
    h.draw(cx);
    let (name, host, port) = cx.read(|cx| {
        let d = dialog.read(cx);
        (
            d.name_input().clone(),
            d.input("host").cloned().unwrap(),
            d.input("port").cloned().unwrap(),
        )
    });
    set_input(&h, cx, &name, "Build box");
    set_input(&h, cx, &host, GOOD_HOST);
    set_input(&h, cx, &port, "2375");
    // Save is disabled until a test passed.
    assert!(!cx.read(|cx| dialog.read(cx).can_save(cx)));
    // Mod+Enter: tests first (no successful test yet).
    h.press(cx, "secondary-enter");
    h.wait_until(cx, "test ok", |_, cx| {
        matches!(
            dialog.read(cx).test_state(),
            Some(crate::pages::settings::add_engine::DialogTest::Ok(_))
        )
    });
    assert!(cx.read(|cx| dialog.read(cx).can_save(cx)));
    // Mod+Enter again: saves.
    h.press(cx, "secondary-enter");
    h.wait_until(cx, "engine added", |_, cx| {
        h.shell
            .read(cx)
            .engines()
            .read(cx)
            .engines()
            .iter()
            .any(|e| e.config.name == "Build box")
    });
    let added = hub_config(&h)
        .engines
        .entries
        .into_iter()
        .find(|e| e.name == "Build box")
        .expect("stored manual engine");
    assert_eq!(added.id.as_str(), "build-box");
    assert_eq!(added.origin, EngineOrigin::Manual);
    assert!(added.enabled && !added.hidden);
    assert_eq!(
        added.endpoint,
        EngineEndpoint::Tcp {
            host: GOOD_HOST.into(),
            port: 2375,
            tls: None
        }
    );
    h.wait_until(cx, "dialog closed", |_, _| true);
    assert!(!h.has_dialog(cx));
    // KBD-071: focus back on the invoker (*Add engine…*).
    let add = cx.read(|cx| page.read(cx).engines_ui().add_focus().clone());
    h.wait_until(cx, "focus restored", |window, _| add.is_focused(window));
    h.shutdown();
}

#[gpui_kit::test]
fn eng_105_failed_test_shows_hint_and_save_anyway(cx: &mut TestAppContext) {
    let h = start_with_schemas(cx);
    let (_page, dialog) = open_add_dialog(&h, cx);
    h.update(cx, |_, window, cx| {
        dialog.update(cx, |d, cx| d.set_kind("TCP", window, cx))
    });
    let (host, port) = cx.read(|cx| {
        let d = dialog.read(cx);
        (
            d.input("host").cloned().unwrap(),
            d.input("port").cloned().unwrap(),
        )
    });
    // Validation first: a scheme in the host is rejected inline.
    set_input(&h, cx, &host, "tcp://nowhere");
    set_input(&h, cx, &port, "2375");
    h.update(cx, |_, _, cx| dialog.update(cx, |d, cx| d.run_test(cx)));
    assert_eq!(
        cx.read(|cx| dialog.read(cx).errors().get("host").copied()),
        Some(s::INVALID_HOST)
    );
    set_input(&h, cx, &host, "10.0.0.66");
    h.update(cx, |_, _, cx| dialog.update(cx, |d, cx| d.run_test(cx)));
    h.wait_until(cx, "test failed", |_, cx| {
        matches!(
            dialog.read(cx).test_state(),
            Some(crate::pages::settings::add_engine::DialogTest::Failed { .. })
        )
    });
    match cx.read(|cx| dialog.read(cx).test_state().cloned()) {
        Some(crate::pages::settings::add_engine::DialogTest::Failed { hint, .. }) => {
            assert_eq!(hint.as_deref(), Some(REFUSED_HINT))
        }
        other => panic!("{other:?}"),
    }
    assert!(!cx.read(|cx| dialog.read(cx).can_save(cx)));
    h.update(cx, |_, window, cx| {
        dialog.update(cx, |d, cx| d.save(true, window, cx))
    });
    h.wait_until(cx, "saved anyway", |_, _| {
        h.hub
            .config()
            .get()
            .engines
            .entries
            .iter()
            .any(|e| e.endpoint.display() == "tcp://10.0.0.66:2375")
    });
    h.shutdown();
}

#[gpui_kit::test]
fn kbd_071_escape_cancels_add_engine_and_restores_focus(cx: &mut TestAppContext) {
    let h = start_with_schemas(cx);
    let (page, _dialog) = open_add_dialog(&h, cx);
    h.press(cx, "escape");
    h.wait_until(cx, "closed", |_, _| true);
    assert!(!h.has_dialog(cx));
    let add = cx.read(|cx| page.read(cx).engines_ui().add_focus().clone());
    h.wait_until(cx, "invoker focused", |window, _| add.is_focused(window));
    assert!(
        hub_config(&h).engines.entries.is_empty(),
        "nothing added on cancel"
    );
    h.shutdown();
}

#[gpui_kit::test]
fn eng_104_remove_manual_engine_confirms(cx: &mut TestAppContext) {
    let mut config = dk_hub::Config::default();
    config.engines.entries.push(EngineConfig {
        id: EngineId::new("remote"),
        name: "Remote".into(),
        endpoint: EngineEndpoint::Tcp {
            host: "10.0.0.66".into(),
            port: 2375,
            tls: None,
        },
        origin: EngineOrigin::Manual,
        enabled: true,
        hidden: false,
    });
    let h = start(
        cx,
        Setup {
            config,
            ..Default::default()
        },
    );
    h.wait_containers(cx);
    open(&h, cx, SettingsSection::Engines);
    engine_op(&h, cx, "remote", EngineOpKind::Remove);
    assert!(h.has_dialog(cx), "SHL-002: removal confirms");
    assert!(
        cx.read(|cx| h
            .shell
            .read(cx)
            .engines()
            .read(cx)
            .get(&EngineId::new("remote"))
            .is_some()),
        "nothing removed before confirming"
    );
    h.press(cx, "secondary-enter");
    h.wait_until(cx, "removed", |_, cx| {
        h.shell
            .read(cx)
            .engines()
            .read(cx)
            .get(&EngineId::new("remote"))
            .is_none()
    });
    assert!(
        hub_config(&h)
            .engines
            .entries
            .iter()
            .all(|e| e.id.as_str() != "remote")
    );
    h.shutdown();
}

#[gpui_kit::test]
fn eng_111_first_run_add_engine_opens_dialog(cx: &mut TestAppContext) {
    let h = start_with_schemas(cx);
    h.wait_containers(cx);
    h.update(cx, |_, window, cx| {
        window.dispatch_action(Box::new(AddEngine), cx)
    });
    h.draw(cx);
    assert_eq!(
        h.read(cx, |s, _, _| s.route().clone()),
        Route::Settings {
            section: SettingsSection::Engines
        },
        "Add engine… shows Settings › Engines"
    );
    assert!(h.has_dialog(cx));
    h.shutdown();
}

// ── Diagnostics (item 5) ─────────────────────────────────────────────────────────────

#[gpui_kit::test]
fn set_060_copy_diagnostics_to_clipboard(cx: &mut TestAppContext) {
    let h = start(cx, Setup::default());
    h.wait_containers(cx);
    open(&h, cx, SettingsSection::Diagnostics);
    h.update(cx, |_, window, cx| {
        window.dispatch_action(Box::new(crate::actions::settings::CopyDiagnostics), cx)
    });
    h.wait_until(cx, "clipboard", |_, cx| {
        cx.read_from_clipboard()
            .and_then(|c| c.text())
            .is_some_and(|t| t.starts_with("Dockering ") && t.contains("demo"))
    });
    h.shutdown();
}

#[gpui_kit::test]
fn set_060_log_level_persists(cx: &mut TestAppContext) {
    let h = start(cx, Setup::default());
    h.wait_containers(cx);
    let page = open(&h, cx, SettingsSection::Diagnostics);
    h.update(cx, |_, window, cx| {
        page.update(cx, |p, cx| p.set_choice(Key::LogLevel, 1, window, cx))
    });
    assert_eq!(
        hub_config(&h).diagnostics.log_level,
        dk_hub::config::LogLevel::Debug
    );
    h.shutdown();
}

#[test]
fn rel_002_licences_are_embedded() {
    let html = crate::pages::settings::diagnostics::LICENSES_HTML;
    assert!(html.contains("<html") || html.contains("<!DOCTYPE"));
    assert!(
        html.to_lowercase().contains("lucide"),
        "icon notices included"
    );
}

// ── Keyboard (item 6) ────────────────────────────────────────────────────────────────

#[gpui_kit::test]
fn set_070_keyboard_section_lists_bindings(cx: &mut TestAppContext) {
    let h = start(cx, Setup::default());
    h.wait_containers(cx);
    let page = open(&h, cx, SettingsSection::Keyboard);
    let rows = cx.read(|cx| page.read(cx).shortcuts().read(cx).rows().to_vec());
    assert_eq!(
        rows,
        crate::keymap::reference_rows(crate::keymap::Os::current())
    );
    // Mod+F filters the list (KBD-025 on this page).
    let nav = sidebar(&h, cx);
    h.focus(cx, &nav);
    h.press(cx, "secondary-f");
    h.type_text(cx, "palette");
    h.draw(cx);
    let visible = cx.read(|cx| page.read(cx).shortcuts().read(cx).visible_rows().len());
    assert!(visible > 0 && visible < rows.len(), "{visible}");
    h.shutdown();
}

// ── KBD-092: Tab reaches every control of each section, in order, without traps ──────

fn tab_walk(h: &Harness, cx: &mut TestAppContext, _page: &Entity<SettingsPage>) -> usize {
    let nav = sidebar(h, cx);
    h.focus(cx, &nav);
    let mut seen = Vec::new();
    for _ in 0..200 {
        h.press(cx, "tab");
        let f = cx
            .update_window(h.any_window(), |_, window, cx| window.focused(cx))
            .unwrap();
        let Some(f) = f else { break };
        if f == nav {
            return seen.len();
        }
        if seen.contains(&f) {
            panic!("KBD-092: Tab cycled without returning to the section nav");
        }
        seen.push(f);
    }
    seen.len()
}

#[gpui_kit::test]
fn a11y_tab_walk_reaches_all_interactive_settings(cx: &mut TestAppContext) {
    let h = start(cx, two_engine_setup(false));
    h.wait_containers(cx);
    let page = open(&h, cx, SettingsSection::General);
    // General: theme, start page, 2 switches (+ the shell's other regions: Tab cycles the
    // whole window, so the walk returns to the nav after visiting them).
    let general = tab_walk(&h, cx, &page);
    assert!(general >= 4, "{general}");
    let page = open(&h, cx, SettingsSection::Containers);
    let containers = tab_walk(&h, cx, &page);
    // group by, label key, switch, polling (number input: + / − aren't tab stops).
    assert!(containers >= 4, "{containers}");
    let page = open(&h, cx, SettingsSection::Engines);
    h.draw(cx);
    let engines = tab_walk(&h, cx, &page);
    // 2 discovery switches, Rescan, Add, then per engine: name, enabled, test, hide.
    assert!(engines >= 4 + 2 * 4, "{engines}");
    h.shutdown();
}

// ── persistence (item 7) ─────────────────────────────────────────────────────────────

#[gpui_kit::test]
fn shl_011_window_bounds_saved_on_change(cx: &mut TestAppContext) {
    let h = start(cx, Setup::default());
    h.wait_containers(cx);
    cx.update_window(h.any_window(), |_, window, cx| {
        crate::app::save_window_bounds(window, cx)
    })
    .unwrap();
    let saved = h.hub.config().ui_state().window.expect("bounds saved");
    assert!(saved.width >= 400. && saved.height >= 300., "{saved:?}");
    // Restored bounds go back into the window options.
    let opts = cx.update(|cx| crate::app::window_options(Some(saved), cx));
    match opts.window_bounds {
        Some(gpui_kit::WindowBounds::Windowed(b)) => {
            assert_eq!(b.size.width.as_f32(), saved.width);
            assert_eq!(b.origin.x.as_f32(), saved.x);
        }
        other => panic!("{other:?}"),
    }
    h.shutdown();
}

/// The demo build has no addable schemas: the dialog still opens from the palette and Esc
/// still closes it (KBD-071).
#[gpui_kit::test]
fn kbd_071_add_engine_from_palette_escape_closes(cx: &mut TestAppContext) {
    let h = start(cx, Setup::default());
    h.wait_containers(cx);
    open(&h, cx, SettingsSection::Engines);
    h.press(cx, "secondary-shift-p");
    h.type_text(cx, "Add engine");
    h.press(cx, "enter");
    h.wait_until(cx, "dialog open", |window, cx| {
        use gpui_kit::component::WindowExt;
        window.has_active_dialog(cx)
    });
    h.draw(cx);
    let focused = cx
        .update_window(h.any_window(), |_, window, cx| window.focused(cx).is_some())
        .unwrap();
    assert!(focused, "something inside the dialog has focus");
    h.press(cx, "escape");
    h.wait_until(cx, "dialog closed", |window, cx| {
        use gpui_kit::component::WindowExt;
        !window.has_active_dialog(cx)
    });
    h.shutdown();
}
