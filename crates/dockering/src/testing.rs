//! View-test harness: a real `HubHandle` over `FakeFactory`/`FakeEngine` (no Docker) and the
//! full `AppShell` in a headless GPUI Kit window.
//!
//! The hub runs its own tokio threads, so tests call `allow_parking()` and wait with
//! [`Harness::wait_until`], which pumps both the GPUI test executor and real time.

use std::sync::Arc;
use std::time::{Duration, Instant};

use dk_core::fake::{FakeEngine, FakeFactory};
use dk_core::{ContainerSummary, EngineFactory};
use dk_hub::{Config, EngineHub, HubHandle, HubOptions, Paths, UiState};
use gpui_kit::base::Root;
use gpui_kit::{
    AnyWindowHandle, App, AppContext, Bounds, Entity, Point, Size, TestAppContext, Window,
    WindowBounds, WindowHandle, WindowOptions, px,
};

use crate::pages::containers::ContainersPage;
use crate::shell::AppShell;

pub struct Harness {
    pub hub: HubHandle,
    pub engine: Arc<FakeEngine>,
    pub factory: Arc<FakeFactory>,
    pub window: WindowHandle<Root>,
    pub shell: Entity<AppShell>,
    _dir: tempfile::TempDir,
}

pub struct Setup {
    pub containers: Vec<ContainerSummary>,
    pub config: Config,
    pub ui_state: UiState,
    /// Extra engines (by id) added to the factory before start.
    pub extra_engines: Vec<Arc<FakeEngine>>,
    /// Engine ids whose connect fails.
    pub failing: Vec<(String, dk_core::EngineError)>,
}

impl Default for Setup {
    fn default() -> Self {
        Self {
            containers: crate::demo::containers(),
            config: Config::default(),
            ui_state: UiState::default(),
            extra_engines: Vec::new(),
            failing: Vec::new(),
        }
    }
}

pub fn start(cx: &mut TestAppContext, setup: Setup) -> Harness {
    start_with_factories(cx, setup, Vec::new())
}

/// [`start`] with extra factories registered *before* the fake one (M9: Add-engine tests use
/// one that publishes real config schemas).
pub fn start_with_factories(
    cx: &mut TestAppContext,
    setup: Setup,
    extra: Vec<Arc<dyn EngineFactory>>,
) -> Harness {
    cx.executor().allow_parking();
    let dir = tempfile::tempdir().expect("tempdir");
    let engine = crate::demo::engine();
    engine.set_containers(setup.containers);
    let factory = FakeFactory::new();
    factory.add(engine.clone());
    for e in &setup.extra_engines {
        factory.add(e.clone());
    }
    for (id, err) in &setup.failing {
        factory.set_connect_error(&dk_core::EngineId::new(id.clone()), Some(err.clone()));
    }
    let hub = EngineHub::start(HubOptions {
        paths: Paths::in_dir(dir.path()),
        config: setup.config.clone(),
        ui_state: setup.ui_state,
        factories: Some(
            extra
                .into_iter()
                .chain([factory.clone() as Arc<dyn EngineFactory>])
                .collect(),
        ),
        discover_on_start: true,
        worker_threads: 2,
        demo: false,
    })
    .expect("hub starts");
    let config = setup.config;
    let hub2 = hub.clone();
    let (window, shell) = cx.update(|cx| {
        gpui_kit::init(cx);
        crate::app::init(hub2, config, true, cx);
        let bounds = Bounds::new(Point::default(), Size::new(px(1280.), px(800.)));
        let (window, shell) = gpui_kit::open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            cx,
            |window, cx| cx.new(|cx| AppShell::new(window, cx)),
        )
        .expect("open test window");
        (window.downcast::<Root>().expect("Base Root"), shell)
    });
    Harness {
        hub,
        engine,
        factory,
        window,
        shell,
        _dir: dir,
    }
}

impl Harness {
    pub fn any_window(&self) -> AnyWindowHandle {
        self.window.into()
    }

    /// Runs the GPUI executor and real time until `f` holds (or panics after `timeout`).
    pub fn wait_until(
        &self,
        cx: &mut TestAppContext,
        what: &str,
        mut f: impl FnMut(&mut Window, &mut App) -> bool,
    ) {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            cx.run_until_parked();
            let ok = cx
                .update_window(self.any_window(), |_, window, cx| {
                    window.draw(cx).clear(cx);
                    f(window, cx)
                })
                .expect("window alive");
            if ok {
                return;
            }
            assert!(Instant::now() < deadline, "timed out waiting for: {what}");
            // Let hub threads make progress, then advance the GPUI clock (debounce timers).
            std::thread::sleep(Duration::from_millis(5)); // nfr-001-allow: test harness only
            cx.executor().advance_clock(Duration::from_millis(20));
        }
    }

    pub fn update<R>(
        &self,
        cx: &mut TestAppContext,
        f: impl FnOnce(&mut AppShell, &mut Window, &mut gpui_kit::Context<AppShell>) -> R,
    ) -> R {
        let shell = self.shell.clone();
        cx.update_window(self.any_window(), |_, window, cx| {
            shell.update(cx, |s, cx| f(s, window, cx))
        })
        .expect("window alive")
    }

    pub fn read<R>(
        &self,
        cx: &mut TestAppContext,
        f: impl FnOnce(&AppShell, &Window, &App) -> R,
    ) -> R {
        let shell = self.shell.clone();
        cx.update_window(self.any_window(), |_, window, cx| {
            f(shell.read(cx), window, cx)
        })
        .expect("window alive")
    }

    pub fn page(&self, cx: &mut TestAppContext) -> Option<Entity<ContainersPage>> {
        self.read(cx, |s, _, _| s.containers_page().cloned())
    }

    /// Waits until the Containers page shows its rows.
    pub fn wait_containers(&self, cx: &mut TestAppContext) -> Entity<ContainersPage> {
        self.wait_until(cx, "containers page with rows", |_, cx| {
            let shell = self.shell.read(cx);
            shell
                .containers_page()
                .is_some_and(|p| !p.read(cx).table().read(cx).model(cx).rows().is_empty())
        });
        self.page(cx).expect("containers page")
    }

    /// Sends a keystroke to the focused element (GPUI key dispatch + bindings).
    /// Write Mod as `secondary-` (`cmd` on macOS); `ctrl-` is only for true Ctrl chords.
    pub fn press(&self, cx: &mut TestAppContext, keys: &str) {
        for k in keys.split(' ') {
            let ks = gpui_kit::Keystroke::parse(k).expect("valid keystroke");
            cx.dispatch_keystroke(self.any_window(), ks);
            self.draw(cx);
        }
    }

    /// Types text into the focused input.
    pub fn type_text(&self, cx: &mut TestAppContext, text: &str) {
        cx.simulate_input(self.any_window(), text);
    }

    /// Focuses the containers table.
    pub fn focus_table(&self, cx: &mut TestAppContext) {
        let page = self.page(cx).expect("containers page");
        cx.update_window(self.any_window(), |_, window, cx| {
            let h = gpui_kit::Focusable::focus_handle(page.read(cx).table().read(cx), cx);
            window.focus(&h, cx);
        })
        .expect("window");
        cx.run_until_parked();
    }

    /// Moves the table cursor to the row with `key`.
    pub fn select_row(&self, cx: &mut TestAppContext, key: &str) {
        let page = self.page(cx).expect("containers page");
        let key = key.to_owned();
        cx.update_window(self.any_window(), |_, _, cx| {
            let table = page.read(cx).table().clone();
            table.update(cx, |t, cx| t.focus_row(&key, cx));
        })
        .expect("window");
        cx.run_until_parked();
    }

    pub fn cursor_key(&self, cx: &mut TestAppContext) -> Option<String> {
        let page = self.page(cx)?;
        cx.read(|cx| {
            page.read(cx)
                .table()
                .read(cx)
                .model(cx)
                .cursor()
                .map(|k| k.to_string())
        })
    }

    pub fn shutdown(self) {
        self.hub.shutdown();
    }
}

/// Container id of a demo container by name.
pub fn id_of(name: &str) -> String {
    crate::demo::containers()
        .into_iter()
        .find(|c| c.name == name)
        .map(|c| c.id)
        .unwrap_or_else(|| panic!("no demo container {name}"))
}

impl Harness {
    /// Runs pending work and draws a frame (renders react to state changes, e.g. dialogs
    /// focusing their initial control).
    pub fn draw(&self, cx: &mut TestAppContext) {
        cx.run_until_parked();
        cx.update_window(self.any_window(), |_, window, cx| {
            window.draw(cx).clear(cx);
        })
        .expect("window alive");
        cx.run_until_parked();
    }
}

// ── M6 helpers (Images, Volumes, Networks) ──────────────────────────────────────────────

impl Harness {
    /// The mounted M6 page of type `V`, if the current page is one.
    pub fn dyn_page<V: 'static>(&self, cx: &mut TestAppContext) -> Option<Entity<V>> {
        self.read(cx, |s, _, _| match s.page() {
            crate::shell::ShellPage::Dyn(p) => p.downcast::<V>(),
            _ => None,
        })
    }

    /// Navigates (as a palette/sidebar would) and waits until a `V` page is mounted.
    pub fn goto<V: 'static>(&self, cx: &mut TestAppContext, route: crate::nav::Route) -> Entity<V> {
        self.update(cx, |_, window, cx| {
            window.dispatch_action(Box::new(crate::actions::Navigate { route }), cx)
        });
        self.wait_until(cx, "page mounted", |_, cx| {
            match self.shell.read(cx).page() {
                crate::shell::ShellPage::Dyn(p) => p.downcast::<V>().is_some(),
                _ => false,
            }
        });
        self.dyn_page::<V>(cx).expect("page")
    }

    /// Focuses `handle` and runs pending work.
    pub fn focus(&self, cx: &mut TestAppContext, handle: &gpui_kit::FocusHandle) {
        let h = handle.clone();
        cx.update_window(self.any_window(), |_, window, cx| window.focus(&h, cx))
            .expect("window");
        self.draw(cx);
    }

    pub fn has_dialog(&self, cx: &mut TestAppContext) -> bool {
        use gpui_kit::component::WindowExt;
        cx.update_window(self.any_window(), |_, window, cx| {
            window.has_active_dialog(cx)
        })
        .expect("window")
    }

    pub fn is_focused(&self, cx: &mut TestAppContext, handle: &gpui_kit::FocusHandle) -> bool {
        let h = handle.clone();
        cx.update_window(self.any_window(), |_, window, _| h.is_focused(window))
            .expect("window")
    }
}

/// Image fixture whose id matches `fixtures::container`'s `image_id` (so it is "in use").
pub fn in_use_image(reference: &str) -> dk_core::ImageSummary {
    dk_core::fake::fixtures::image(reference, "nginx:1.27")
}
