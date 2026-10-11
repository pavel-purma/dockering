//! Stopped-engine start page tests (ENG-139, ENG-141, SHL-013, KBD-002). Real `HubHandle`
//! with a factory whose engine stays stopped until the user starts it.

use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use dk_core::fake::FakeEngine;
use dk_core::{
    DiscoveredEngine, Engine, EngineConfig, EngineConfigSchema, EngineEndpoint, EngineError,
    EngineFactory, EngineId, EngineKind, EngineOrigin, EngineResult, EngineState, StartPrompt,
};
use gpui_kit::TestAppContext;

use crate::commands::{self, CommandContext};
use crate::nav::{Route, SettingsSection};
use crate::strings as s;
use crate::testing::{Harness, Setup, start_with_factories};

type Boxed<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

const ID: &str = "stopped-one";
const HOST: &str = "stopped.test";

struct StoppedFactory {
    engine: Arc<FakeEngine>,
    prompt: Option<StartPrompt>,
    started: Mutex<bool>,
    starts: Mutex<u32>,
    gate: Mutex<Option<futures::channel::oneshot::Receiver<()>>>,
}

impl StoppedFactory {
    fn new(prompt: Option<StartPrompt>) -> Arc<Self> {
        Arc::new(Self {
            engine: FakeEngine::new(ID),
            prompt,
            started: Mutex::new(false),
            starts: Mutex::new(0),
            gate: Mutex::new(None),
        })
    }
    fn starts(&self) -> u32 {
        *self.starts.lock().unwrap()
    }
}

fn config() -> EngineConfig {
    EngineConfig {
        id: EngineId::new(ID),
        name: "Sleepy".into(),
        endpoint: EngineEndpoint::Tcp {
            host: HOST.into(),
            port: 2375,
            tls: None,
        },
        origin: EngineOrigin::Discovered,
        enabled: true,
        hidden: false,
    }
}

fn prompt() -> StartPrompt {
    StartPrompt {
        title: "Sleepy is not running".into(),
        body: "Nothing is running here yet. Start it to continue.".into(),
        action_label: "Wake Sleepy".into(),
    }
}

impl EngineFactory for StoppedFactory {
    fn kind(&self) -> EngineKind {
        EngineKind::Docker
    }
    fn handles(&self, endpoint: &EngineEndpoint) -> bool {
        matches!(endpoint, EngineEndpoint::Tcp { host, .. } if host == HOST)
    }
    fn discover<'a, 'b>(&'a self) -> Boxed<'b, Vec<DiscoveredEngine>>
    where
        'a: 'b,
        Self: 'b,
    {
        Box::pin(async {
            let mut d = DiscoveredEngine::new(config(), 0);
            d.initial_state = Some(EngineState::Stopped);
            vec![d]
        })
    }
    fn connect<'a, 'c, 'b>(
        &'a self,
        _cfg: &'c EngineConfig,
    ) -> Boxed<'b, EngineResult<Arc<dyn Engine>>>
    where
        'a: 'b,
        'c: 'b,
        Self: 'b,
    {
        Box::pin(async move {
            if *self.started.lock().unwrap() {
                Ok(self.engine.clone() as Arc<dyn Engine>)
            } else {
                Err(EngineError::unreachable("not running"))
            }
        })
    }
    fn config_schema(&self) -> Vec<EngineConfigSchema> {
        Vec::new()
    }
    fn start_prompt(&self, _cfg: &EngineConfig) -> Option<StartPrompt> {
        self.prompt.clone()
    }
    fn start<'a, 'c, 'b>(&'a self, _cfg: &'c EngineConfig) -> Boxed<'b, EngineResult<()>>
    where
        'a: 'b,
        'c: 'b,
        Self: 'b,
    {
        Box::pin(async move {
            *self.starts.lock().unwrap() += 1;
            let gate = self.gate.lock().unwrap().take();
            if let Some(rx) = gate {
                let _ = rx.await;
            }
            *self.started.lock().unwrap() = true;
            Ok(())
        })
    }
}

fn boot(cx: &mut TestAppContext, factory: &Arc<StoppedFactory>) -> Harness {
    let ui_state = dk_hub::UiState {
        last_engine: Some(EngineId::new(ID)),
        ..Default::default()
    };
    let h = start_with_factories(
        cx,
        Setup {
            ui_state,
            ..Default::default()
        },
        vec![factory.clone() as Arc<dyn EngineFactory>],
    );
    h.wait_until(cx, "engine stopped and active", |_, cx| {
        h.shell
            .read(cx)
            .engines()
            .read(cx)
            .active()
            .is_some_and(|e| e.id().as_str() == ID && matches!(e.state, EngineState::Stopped))
    });
    h.draw(cx);
    h
}

fn rendered(h: &Harness, cx: &mut TestAppContext, selector: &'static str) -> bool {
    h.draw(cx);
    gpui_kit::VisualTestContext::from_window(h.any_window(), cx)
        .debug_bounds(selector)
        .is_some()
}

fn active_is(
    h: &Harness,
    cx: &mut TestAppContext,
    f: impl Fn(&EngineState) -> bool + 'static,
) -> bool {
    h.read(cx, |s, _, cx| {
        s.engines().read(cx).active().is_some_and(|e| f(&e.state))
    })
}

fn start_action(h: &Harness, cx: &mut TestAppContext) {
    h.update(cx, |_, window, cx| {
        window.dispatch_action(Box::new(crate::actions::StartEngine), cx)
    });
}

#[gpui_kit::test]
fn eng_139_stopped_page_is_not_an_error_alert(cx: &mut TestAppContext) {
    let f = StoppedFactory::new(Some(prompt()));
    let h = boot(cx, &f);
    assert!(rendered(&h, cx, "engine-stopped"));
    assert!(!rendered(&h, cx, "engine-unreachable"));
    assert!(h.read(cx, |s, _, _| matches!(
        s.page(),
        crate::shell::ShellPage::None
    )));
    assert_eq!(f.starts(), 0, "nothing starts on its own (ENG-138)");
    h.shutdown();
}

#[gpui_kit::test]
fn eng_139_start_prompt_label_used_on_page_switcher_and_settings(cx: &mut TestAppContext) {
    let f = StoppedFactory::new(Some(prompt()));
    let h = boot(cx, &f);
    let status = h.read(cx, |s, _, cx| {
        s.engines().read(cx).active().cloned().unwrap()
    });
    assert_eq!(
        crate::shell::engine_views::start_label(&status),
        "Wake Sleepy"
    );
    assert_eq!(
        crate::shell::switcher::choice_for(&status),
        crate::shell::switcher::Choice::StartAndConnect(ID.into())
    );
    h.update(cx, |_, window, cx| {
        window.dispatch_action(Box::new(crate::actions::EngineSwitcher), cx)
    });
    assert!(
        rendered(&h, cx, "switcher-start-label"),
        "the switcher row offers the start action"
    );
    h.update(cx, |_, window, cx| {
        window.dispatch_action(Box::new(crate::actions::EngineSwitcher), cx)
    });
    h.draw(cx);
    h.update(cx, |_, window, cx| {
        window.dispatch_action(
            Box::new(crate::actions::Navigate {
                route: Route::Settings {
                    section: SettingsSection::Engines,
                },
            }),
            cx,
        )
    });
    h.wait_until(cx, "settings", |_, cx| {
        matches!(
            h.shell.read(cx).page(),
            crate::shell::ShellPage::Settings(_)
        )
    });
    assert!(rendered(&h, cx, "settings-start-label"));
    h.shutdown();
}

#[gpui_kit::test]
fn eng_139_stopped_without_prompt_keeps_start_and_connect(cx: &mut TestAppContext) {
    let f = StoppedFactory::new(None);
    let h = boot(cx, &f);
    let status = h.read(cx, |s, _, cx| {
        s.engines().read(cx).active().cloned().unwrap()
    });
    assert!(status.start_prompt.is_none());
    assert_eq!(
        crate::shell::engine_views::start_label(&status),
        s::START_AND_CONNECT
    );
    assert!(rendered(&h, cx, "engine-unreachable"));
    assert!(!rendered(&h, cx, "engine-stopped"));
    start_action(&h, cx);
    h.wait_until(cx, "connected", |_, cx| {
        h.shell
            .read(cx)
            .engines()
            .read(cx)
            .active()
            .is_some_and(|e| e.state.is_usable())
    });
    assert_eq!(f.starts(), 1);
    h.shutdown();
}

#[gpui_kit::test]
fn eng_141_start_button_focus_and_palette_entry(cx: &mut TestAppContext) {
    let f = StoppedFactory::new(Some(prompt()));
    let (tx, rx) = futures::channel::oneshot::channel();
    *f.gate.lock().unwrap() = Some(rx);
    let h = boot(cx, &f);
    let focus = h.read(cx, |s, _, _| s.start_focus().clone());
    h.wait_until(cx, "start button focused", |window, _| {
        focus.is_focused(window)
    });

    let entry = commands::COMMANDS
        .iter()
        .find(|c| c.label == s::CMD_START_ENGINE)
        .expect("palette entry");
    assert_eq!((entry.action)().name(), "dk::StartEngine");
    assert!(entry.keywords.contains(&"session"));
    assert!(entry.available(&CommandContext {
        has_engine: true,
        ..Default::default()
    }));
    assert!(
        !commands::COMMANDS
            .iter()
            .any(|c| c.label == s::START_AND_CONNECT)
    );

    h.press(cx, "enter");
    h.wait_until(cx, "starting", |_, cx| {
        h.shell
            .read(cx)
            .engines()
            .read(cx)
            .active()
            .is_some_and(|e| matches!(e.state, EngineState::Connecting))
    });
    assert!(
        rendered(&h, cx, "engine-stopped"),
        "Starting… keeps the page"
    );
    start_action(&h, cx);
    h.draw(cx);
    assert_eq!(f.starts(), 1, "a second start while starting is ignored");
    tx.send(()).unwrap();
    h.wait_until(cx, "connected", |_, cx| {
        h.shell
            .read(cx)
            .engines()
            .read(cx)
            .active()
            .is_some_and(|e| e.state.is_usable())
    });
    assert!(!rendered(&h, cx, "engine-stopped"));
    assert!(active_is(&h, cx, EngineState::is_usable));
    h.shutdown();
}
