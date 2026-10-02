//! `term_demo`: a GPUI Kit window with a `TerminalView` in local-echo mode.
//!
//! Typed input is fed straight back (Enter → CRLF, Backspace erases), so the view can be tried
//! without a container. Prints frame timings to stderr (TRM-010):
//!
//! ```sh
//! cargo run -p dk-terminal --example term_demo            # interactive
//! cargo run -p dk-terminal --example term_demo -- --perf  # fill a 200×50 grid, print timings
//! ```

use std::time::{Duration, Instant};

use dk_terminal::{TerminalConfig, TerminalEvent, TerminalView};
use gpui_kit::component::{Theme, ThemeMode};
use gpui_kit::{
    AppContext, Bounds, Context, Entity, Focusable, IntoElement, Keystroke, ParentElement, Render,
    Styled, Subscription, Task, Window, WindowBounds, WindowOptions, div, px, size,
};

struct Demo {
    terminal: Entity<TerminalView>,
    /// Keypress → paint latency: set when a key is dispatched, reported after the repaint.
    echo_started: Option<Instant>,
    /// Report the grid prepaint time after the next terminal repaint.
    report_grid: bool,
    _subscriptions: Vec<Subscription>,
    _perf: Option<Task<()>>,
}

impl Demo {
    fn new(perf: bool, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let terminal = cx.new(|cx| TerminalView::new(TerminalConfig::default(), window, cx));
        let events = cx.subscribe_in(&terminal, window, Self::on_terminal_event);
        let repaints = cx.observe_in(&terminal, window, Self::on_terminal_notify);
        terminal.update(cx, |term, cx| term.feed(banner().as_bytes(), cx));
        let focus = terminal.focus_handle(cx);
        window.focus(&focus, cx);
        let perf_task = perf.then(|| Self::run_perf(window, cx));
        Self {
            terminal,
            echo_started: None,
            report_grid: false,
            _subscriptions: vec![events, repaints],
            _perf: perf_task,
        }
    }

    /// The terminal asked for a repaint: print timings once that frame has been drawn.
    fn on_terminal_notify(
        &mut self,
        _terminal: Entity<TerminalView>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.echo_started.is_none() && !self.report_grid {
            return;
        }
        let view = cx.entity();
        window.on_next_frame(move |_window, cx| {
            view.update(cx, |this, cx| {
                let term = this.terminal.read(cx);
                let (cols, rows) = term.size();
                let prepaint = term.last_prepaint_time().as_secs_f64() * 1000.0;
                if let Some(started) = this.echo_started.take() {
                    eprintln!(
                        "[perf] key -> echo painted: {:.2} ms (grid prepaint {prepaint:.2} ms)",
                        started.elapsed().as_secs_f64() * 1000.0,
                    );
                }
                if std::mem::take(&mut this.report_grid) {
                    eprintln!(
                        "[perf] full {cols}x{rows} grid: prepaint {prepaint:.2} ms, rows shaped {}",
                        term.rows_shaped_last_frame(),
                    );
                }
            });
        });
    }

    fn on_terminal_event(
        &mut self,
        terminal: &Entity<TerminalView>,
        event: &TerminalEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            TerminalEvent::Input(bytes) => {
                let echo = local_echo(bytes);
                terminal.update(cx, |term, cx| term.feed(&echo, cx));
            }
            TerminalEvent::ReconnectRequested => {
                terminal.update(cx, |term, cx| {
                    term.reset(cx);
                    term.feed(banner().as_bytes(), cx);
                });
            }
            TerminalEvent::Resize { cols, rows } => eprintln!("[demo] resize {cols}x{rows}"),
            TerminalEvent::Title(title) => window.set_window_title(title),
            TerminalEvent::Bell => {}
        }
    }

    /// Fill the screen with distinct coloured text several times, then type keys through the
    /// real dispatch path (interceptor / input handler → Input → echo → repaint); print timings.
    fn run_perf(window: &mut Window, cx: &mut Context<Self>) -> Task<()> {
        cx.spawn_in(window, async move |this, cx| {
            for round in 0..6 {
                cx.background_executor()
                    .timer(Duration::from_millis(250))
                    .await;
                let ok = this.update(cx, |this, cx| {
                    let (cols, rows) = this.terminal.read(cx).size();
                    let screen = full_screen(cols, rows, round);
                    this.report_grid = true;
                    this.terminal.update(cx, |term, cx| term.feed(&screen, cx));
                });
                if ok.is_err() {
                    return;
                }
            }
            for key in ["e", "c", "h", "o", "enter"] {
                cx.background_executor()
                    .timer(Duration::from_millis(200))
                    .await;
                // Dispatch outside `Demo::update`: the echo comes back through Demo's
                // subscription, which updates Demo.
                if this
                    .update(cx, |this, _| this.echo_started = Some(Instant::now()))
                    .is_err()
                {
                    return;
                }
                let ok = cx.update(|window, cx| {
                    if let Ok(keystroke) = Keystroke::parse(key) {
                        window.dispatch_keystroke(keystroke, cx);
                    }
                });
                if ok.is_err() {
                    return;
                }
            }
            cx.background_executor()
                .timer(Duration::from_millis(250))
                .await;
            let _ = this.update(cx, |this, cx| {
                this.terminal.update(cx, |term, cx| {
                    term.feed(b"\r\n\x1b[1;32mperf done\x1b[0m\r\n", cx)
                });
            });
        })
    }
}

impl Render for Demo {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div().size_full().p(px(8.)).child(self.terminal.clone())
    }
}

/// What a cooked-mode tty would echo for `bytes`.
fn local_echo(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len());
    for &b in bytes {
        match b {
            b'\r' => out.extend_from_slice(b"\r\n$ "),
            0x7f | 0x08 => out.extend_from_slice(b"\x08 \x08"),
            0x03 => out.extend_from_slice(b"^C\r\n$ "),
            0x0c => out.extend_from_slice(b"\x1b[2J\x1b[H$ "),
            0x1b => out.extend_from_slice(b"^["),
            b'\t' => out.push(b'\t'),
            b if b < 0x20 => out.extend_from_slice(&[b'^', b + 0x40]),
            b => out.push(b),
        }
    }
    out
}

fn banner() -> String {
    let mut s = String::from("\x1b[1mDockering terminal demo\x1b[0m (local echo)\r\n\r\n");
    for (i, name) in [
        "black", "red", "green", "yellow", "blue", "magenta", "cyan", "white",
    ]
    .iter()
    .enumerate()
    {
        s.push_str(&format!("\x1b[3{i}m{name:>8}\x1b[0m "));
        s.push_str(&format!("\x1b[9{i}m{name:>8}\x1b[0m "));
        s.push_str(&format!("\x1b[4{i}m        \x1b[0m\r\n"));
    }
    s.push_str("\r\n");
    for i in 0..=255u16 {
        s.push_str(&format!("\x1b[48;5;{i}m  "));
        if i == 15 || (i > 15 && (i - 15) % 36 == 0) {
            s.push_str("\x1b[0m\r\n");
        }
    }
    s.push_str("\x1b[0m\r\n");
    for x in 0..64u16 {
        let r = (x * 4) as u8;
        let b = 255 - r;
        s.push_str(&format!("\x1b[48;2;{r};80;{b}m "));
    }
    s.push_str("\x1b[0m\r\n\r\n");
    s.push_str(
        "\x1b[1mbold\x1b[0m \x1b[3mitalic\x1b[0m \x1b[4munderline\x1b[0m \x1b[2mdim\x1b[0m \
         \x1b[7minverse\x1b[0m \x1b[9mstrike\x1b[0m  wide: \u{4f60}\u{597d} \u{1f433}\r\n\r\n$ ",
    );
    s
}

/// A full screen of distinct text (so every row must be reshaped).
fn full_screen(cols: u16, rows: u16, round: u32) -> Vec<u8> {
    let mut s = String::from("\x1b[H\x1b[2J");
    for row in 0..rows {
        let color = 31 + (row as u32 + round) % 7;
        s.push_str(&format!("\x1b[{color}m"));
        let mut line = String::new();
        while line.len() < cols as usize {
            line.push_str(&format!("r{round}:{row}:{} ", line.len()));
        }
        line.truncate(cols as usize);
        s.push_str(&line);
        if row + 1 < rows {
            s.push_str("\r\n");
        }
    }
    s.push_str("\x1b[0m");
    s.into_bytes()
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let perf = args.iter().any(|a| a == "--perf");
    let dark = !args.iter().any(|a| a == "--light");
    gpui_kit::application().run(move |cx| {
        gpui_kit::init(cx);
        Theme::change(
            if dark {
                ThemeMode::Dark
            } else {
                ThemeMode::Light
            },
            None,
            cx,
        );
        cx.bind_keys(dk_terminal::default_key_bindings());
        // Big enough for a 200×50 grid at 13 px.
        let bounds = Bounds::centered(None, size(px(1620.), px(860.)), cx);
        gpui_kit::open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            cx,
            |window, cx| cx.new(|cx| Demo::new(perf, window, cx)),
        )
        .expect("open window");
        cx.activate(true);
    });
}
