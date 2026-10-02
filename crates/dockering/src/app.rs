//! GPUI bootstrap (spec 10 §7 steps 2–4): assets, `gpui_kit::init`, globals, theme, keymap,
//! menus, the main window with `AppShell`, single-instance focus requests, and shutdown.

use dk_hub::config::WindowBounds as SavedBounds;
use dk_hub::single_instance::InstanceGuard;
use dk_hub::{Config, HubHandle, UiState};
use futures::StreamExt;
use gpui_kit::component::TitleBar;
use gpui_kit::{
    App, AppContext, Bounds, Entity, Point, QuitMode, Size, Window, WindowBounds, WindowOptions, px,
};

use crate::keymap;
use crate::shell::AppShell;
use crate::shell::menus;
use crate::state::{self, AppState, Ticker};
use crate::strings as s;
use crate::theme;

/// Everything `main` prepared before GPUI starts.
pub struct Boot {
    pub hub: HubHandle,
    pub config: Config,
    pub ui_state: UiState,
    pub instance: Option<InstanceGuard>,
    pub demo: bool,
}

/// Linux `app_id` / Wayland (SHL-023).
pub const APP_ID: &str = "dev.dockering.Dockering";

/// App-level setup shared by `run` and view tests: globals, theme, keymap, ticker.
pub fn init(hub: HubHandle, config: Config, demo: bool, cx: &mut App) {
    let general = config.general.clone();
    state::install(hub, config, demo, cx);
    theme::apply(general.theme, general.ui_scale, None, cx);
    keymap::install(cx);
    Ticker::install(cx);
}

pub fn window_options(saved: Option<SavedBounds>, cx: &App) -> WindowOptions {
    let default = Bounds::centered(None, Size::new(px(1280.), px(800.)), cx);
    let bounds = saved
        .filter(|b| b.width >= 400. && b.height >= 300. && b.x.is_finite() && b.y.is_finite())
        .map(|b| {
            let r = Bounds::new(
                Point::new(px(b.x), px(b.y)),
                Size::new(px(b.width), px(b.height)),
            );
            if b.maximized {
                WindowBounds::Maximized(r)
            } else {
                WindowBounds::Windowed(r)
            }
        })
        .unwrap_or(WindowBounds::Windowed(default));
    let mut titlebar = TitleBar::title_bar_options();
    titlebar.title = Some(s::APP_NAME.into());
    WindowOptions {
        window_bounds: Some(bounds),
        titlebar: Some(titlebar),
        app_owns_titlebar_drag: true,
        app_id: Some(APP_ID.to_owned()),
        window_min_size: Some(Size::new(px(720.), px(480.))),
        ..TitleBar::window_options()
    }
}

fn save_window_bounds(window: &Window, cx: &App) {
    let wb = window.window_bounds();
    let (b, maximized) = match wb {
        WindowBounds::Windowed(b) => (b, false),
        WindowBounds::Maximized(b) | WindowBounds::Fullscreen(b) => (b, true),
    };
    let saved = SavedBounds {
        x: b.origin.x.as_f32(),
        y: b.origin.y.as_f32(),
        width: b.size.width.as_f32(),
        height: b.size.height.as_f32(),
        maximized,
    };
    AppState::update_ui_state(cx, move |s| s.window = Some(saved));
}

/// Runs the GPUI application until quit.
pub fn run(boot: Boot) {
    let Boot {
        hub,
        config,
        ui_state,
        mut instance,
        demo,
    } = boot;
    let app = gpui_kit::application()
        .with_assets(crate::assets::AppAssets)
        .with_quit_mode(QuitMode::LastWindowClosed);
    let shutdown_hub = hub.clone();
    app.run(move |cx| {
        gpui_kit::init(cx);
        init(hub.clone(), config, demo, cx);
        menus::set_app_menus(cx);

        let options = window_options(ui_state.window, cx);
        let opened = gpui_kit::open_window(options, cx, |window, cx| {
            window.set_window_title(s::APP_NAME);
            let shell = cx.new(|cx| AppShell::new(window, cx));
            // Persist bounds on resize/move (debounced by the hub).
            let shell_for_bounds = shell.clone();
            window.observe_window_bounds_for(&shell_for_bounds, cx);
            shell
        });
        let (window, shell) = match opened {
            Ok(v) => v,
            Err(err) => {
                tracing::error!(%err, "failed to open the main window");
                cx.quit();
                return;
            }
        };
        let _ = &shell;

        // SHL-022: later launches ask us to focus.
        if let Some(mut rx) = instance.as_mut().and_then(|g| g.take_focus_requests()) {
            cx.spawn(async move |cx| {
                while rx.next().await.is_some() {
                    cx.update(|cx| {
                        cx.activate(true);
                        let _ = window.update(cx, |_, window, _| window.activate_window());
                    });
                }
            })
            .detach();
        }

        // Keep the guard alive for the app's lifetime.
        let guard = instance.take();
        let hub_for_quit = shutdown_hub.clone();
        cx.on_app_quit(move |cx| {
            let _ = &guard;
            let _ = window.update(cx, |_, window, cx| save_window_bounds(window, cx));
            hub_for_quit.shutdown();
            async {}
        })
        .detach();
    });
}

/// Small helper trait so the bootstrap can observe bounds without leaking subscriptions.
trait ObserveBounds {
    fn observe_window_bounds_for(&mut self, shell: &Entity<AppShell>, cx: &mut App);
}

impl ObserveBounds for Window {
    fn observe_window_bounds_for(&mut self, shell: &Entity<AppShell>, cx: &mut App) {
        shell.update(cx, |_, cx| {
            cx.observe_window_bounds(self, |_, window, cx| save_window_bounds(window, cx))
                .detach();
        });
    }
}
