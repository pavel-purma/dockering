//! Toasts (SHL-003, KBD-074): Dockering's own notification stack.
//!
//! One [`ToastHost`] per window renders every toast in the bottom-right corner, above the
//! status bar. Autohiding toasts show a bar along their bottom edge that drains as the time
//! runs out; hovering or focusing a toast pauses it. The timing lives in [`ToastList`], a plain
//! model with no GPUI types, so it is unit-tested directly.
//!
//! Toasts are pushed through [`push`] (or the helpers in [`super::notify`]); a toast with a
//! `key` replaces the live one with the same key and can be removed by it ([`remove`]).
//! Callbacks (an action button, a body click) run deferred, after the host's own update ends,
//! so they may push or remove toasts themselves.

use std::rc::Rc;
use std::time::Duration;

use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::{ActiveTheme, Icon, IconName, Sizable, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::{
    AnyElement, App, BoxShadow, ClickEvent, Context, Entity, FocusHandle, Global, Hsla,
    IntoElement, MouseButton, Pixels, Render, SharedString, Task, Window, div, hsla,
    linear_color_stop, linear_gradient, px, relative,
};

use crate::actions::toast::Dismiss;
use crate::keymap::ctx;

/// How long an autohiding toast stays up once it has settled.
pub const AUTOHIDE: Duration = Duration::from_secs(5);
const ENTER: Duration = Duration::from_millis(240);
const EXIT: Duration = Duration::from_millis(180);
const TICK: Duration = Duration::from_millis(16);
/// Toasts drawn at once; older ones wait (and keep counting down) out of sight.
const MAX_VISIBLE: usize = 5;
const WIDTH: Pixels = px(380.);
const MARGIN: Pixels = px(16.);
/// How far a toast rises while it fades in, and sinks while it fades out.
const SLIDE_IN: f32 = 20.;
const SLIDE_OUT: f32 = 10.;
const BAR_HEIGHT: Pixels = px(3.);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToastKind {
    Info,
    Success,
    Error,
}

type Body = Rc<dyn Fn(&mut Window, &mut App) -> AnyElement>;
type Run = Rc<dyn Fn(&mut Window, &mut App)>;
type Click = Rc<dyn Fn(&ClickEvent, &mut Window, &mut App)>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ActionStyle {
    Default,
    Primary,
    Ghost,
}

/// A button on a toast.
pub struct ToastAction {
    id: SharedString,
    label: SharedString,
    tooltip: Option<SharedString>,
    style: ActionStyle,
    dismiss: bool,
    run: Run,
}

impl ToastAction {
    pub fn new(
        id: impl Into<SharedString>,
        label: impl Into<SharedString>,
        run: impl Fn(&mut Window, &mut App) + 'static,
    ) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            tooltip: None,
            style: ActionStyle::Default,
            dismiss: false,
            run: Rc::new(run),
        }
    }

    pub fn primary(mut self) -> Self {
        self.style = ActionStyle::Primary;
        self
    }

    pub fn ghost(mut self) -> Self {
        self.style = ActionStyle::Ghost;
        self
    }

    pub fn tooltip(mut self, tooltip: impl Into<SharedString>) -> Self {
        self.tooltip = Some(tooltip.into());
        self
    }

    /// Closes the toast when the button is clicked.
    pub fn dismiss(mut self) -> Self {
        self.dismiss = true;
        self
    }
}

/// What to show. A toast with an action, or of kind *error*, doesn't hide by itself.
pub struct Toast {
    key: Option<SharedString>,
    kind: Option<ToastKind>,
    title: Option<SharedString>,
    message: Option<SharedString>,
    body: Option<Body>,
    actions: Vec<ToastAction>,
    on_click: Option<Click>,
    autohide: bool,
}

impl Toast {
    fn of(kind: Option<ToastKind>, autohide: bool) -> Self {
        Self {
            key: None,
            kind,
            title: None,
            message: None,
            body: None,
            actions: Vec::new(),
            on_click: None,
            autohide,
        }
    }

    pub fn info(message: impl Into<SharedString>) -> Self {
        Self::of(Some(ToastKind::Info), true).message(message)
    }

    pub fn success(message: impl Into<SharedString>) -> Self {
        Self::of(Some(ToastKind::Success), true).message(message)
    }

    pub fn error(message: impl Into<SharedString>) -> Self {
        Self::of(Some(ToastKind::Error), false).message(message)
    }

    /// No icon and no message: the content comes from [`Toast::body`].
    pub fn custom() -> Self {
        Self::of(None, true)
    }

    pub fn message(mut self, message: impl Into<SharedString>) -> Self {
        self.message = Some(message.into());
        self
    }

    pub fn title(mut self, title: impl Into<SharedString>) -> Self {
        self.title = Some(title.into());
        self
    }

    /// Replaces a live toast with the same key, and lets [`remove`] find it.
    pub fn key(mut self, key: impl Into<SharedString>) -> Self {
        self.key = Some(key.into());
        self
    }

    /// Extra content under the message, rebuilt on every frame.
    pub fn body(mut self, body: impl Fn(&mut Window, &mut App) -> AnyElement + 'static) -> Self {
        self.body = Some(Rc::new(body));
        self
    }

    pub fn action(mut self, action: ToastAction) -> Self {
        self.actions.push(action);
        self.autohide = false;
        self
    }

    /// A click on the toast body (not on a button); the toast closes first.
    pub fn on_click(
        mut self,
        on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_click = Some(Rc::new(on_click));
        self
    }

    pub fn autohide(mut self, autohide: bool) -> Self {
        self.autohide = autohide;
        self
    }
}

struct Live {
    id: u64,
    toast: Toast,
    born: Duration,
    /// Time left before it hides; `None` for a toast that stays.
    remaining: Option<Duration>,
    dying: Option<Duration>,
    hovered: bool,
}

/// The toasts and their clocks. Time is the span since the host was created.
#[derive(Default)]
struct ToastList {
    live: Vec<Live>,
    next_id: u64,
    last: Duration,
    /// Keyboard focus is inside the stack: nothing counts down.
    focused: bool,
}

impl ToastList {
    fn push(&mut self, toast: Toast, now: Duration) -> u64 {
        if let Some(key) = &toast.key {
            self.live
                .retain(|l| l.dying.is_some() || l.toast.key.as_ref() != Some(key));
        }
        self.next_id += 1;
        let id = self.next_id;
        self.live.push(Live {
            id,
            remaining: toast.autohide.then_some(AUTOHIDE),
            toast,
            born: now,
            dying: None,
            hovered: false,
        });
        id
    }

    fn dismiss(&mut self, id: u64, now: Duration) {
        if let Some(l) = self.live.iter_mut().find(|l| l.id == id) {
            l.dying.get_or_insert(now);
        }
    }

    fn remove_key(&mut self, key: &str, now: Duration) {
        for l in &mut self.live {
            if l.toast.key.as_deref() == Some(key) {
                l.dying.get_or_insert(now);
            }
        }
    }

    #[cfg(test)]
    fn dismiss_all(&mut self, now: Duration) {
        for l in &mut self.live {
            l.dying.get_or_insert(now);
        }
    }

    fn newest(&self) -> Option<u64> {
        self.live
            .iter()
            .rev()
            .find(|l| l.dying.is_none())
            .map(|l| l.id)
    }

    /// Toasts that haven't been dismissed.
    fn count(&self) -> usize {
        self.live.iter().filter(|l| l.dying.is_none()).count()
    }

    /// Spends the time since the last step, then drops toasts that finished leaving. Returns
    /// whether anything still moves or counts down (the clock keeps running).
    fn step(&mut self, now: Duration) -> bool {
        let delta = now.saturating_sub(self.last);
        self.last = now;
        let paused = self.focused;
        for l in &mut self.live {
            let settled = now.saturating_sub(l.born) >= ENTER;
            if l.dying.is_none()
                && settled
                && !paused
                && !l.hovered
                && let Some(remaining) = &mut l.remaining
            {
                *remaining = remaining.saturating_sub(delta);
                if remaining.is_zero() {
                    l.dying = Some(now);
                }
            }
        }
        self.live
            .retain(|l| l.dying.is_none_or(|d| now.saturating_sub(d) < EXIT));
        self.live.iter().any(|l| {
            l.dying.is_some() || now.saturating_sub(l.born) < ENTER || l.remaining.is_some()
        })
    }
}

/// The window's toast stack; created by the shell and reached through [`push`] and friends.
pub struct ToastHost {
    list: ToastList,
    epoch: std::time::Instant,
    focus: FocusHandle,
    /// Where focus returns when the stack empties or is left with `Esc`.
    return_focus: Option<FocusHandle>,
    tick: Option<Task<()>>,
}

struct HostGlobal(Entity<ToastHost>);

impl Global for HostGlobal {}

fn host(cx: &App) -> Option<Entity<ToastHost>> {
    cx.try_global::<HostGlobal>().map(|g| g.0.clone())
}

/// Creates the host and makes it the target of [`push`]. Called once by the shell.
pub fn install(cx: &mut App) -> Entity<ToastHost> {
    let host = cx.new(|cx| ToastHost {
        list: ToastList::default(),
        epoch: cx.background_executor().now(),
        focus: cx.focus_handle().tab_stop(true),
        return_focus: None,
        tick: None,
    });
    cx.set_global(HostGlobal(host.clone()));
    host
}

/// Shows `toast`. Does nothing before the shell has installed the host.
pub fn push(toast: Toast, cx: &mut App) {
    if let Some(host) = host(cx) {
        host.update(cx, |h, cx| h.push(toast, cx));
    }
}

/// Closes the toast pushed with `key`.
pub fn remove(key: &str, cx: &mut App) {
    if let Some(host) = host(cx) {
        host.update(cx, |h, cx| {
            let now = h.now(cx);
            h.list.remove_key(key, now);
            h.wake(cx);
        });
    }
}

/// Repaints the stack: for a toast body that reads state changing outside the host.
pub fn refresh(cx: &mut App) {
    if let Some(host) = host(cx) {
        host.update(cx, |_, cx| cx.notify());
    }
}

#[cfg(test)]
/// Toasts that are showing (not yet dismissed).
pub(crate) fn count(cx: &App) -> usize {
    host(cx).map_or(0, |h| h.read(cx).list.count())
}

#[cfg(test)]
/// Toasts still on screen, including ones fading out.
pub(crate) fn mounted(cx: &App) -> usize {
    host(cx).map_or(0, |h| h.read(cx).list.live.len())
}

#[cfg(test)]
pub(crate) fn clear(cx: &mut App) {
    if let Some(host) = host(cx) {
        host.update(cx, |h, cx| {
            let now = h.now(cx);
            h.list.dismiss_all(now);
            h.wake(cx);
        });
    }
}

impl ToastHost {
    fn now(&self, cx: &App) -> Duration {
        cx.background_executor()
            .now()
            .saturating_duration_since(self.epoch)
    }

    fn push(&mut self, toast: Toast, cx: &mut Context<Self>) {
        let now = self.now(cx);
        self.list.push(toast, now);
        self.wake(cx);
    }

    fn dismiss(&mut self, id: u64, cx: &mut Context<Self>) {
        let now = self.now(cx);
        self.list.dismiss(id, now);
        self.wake(cx);
    }

    fn set_hovered(&mut self, id: u64, hovered: bool, cx: &mut Context<Self>) {
        if let Some(l) = self.list.live.iter_mut().find(|l| l.id == id)
            && l.hovered != hovered
        {
            l.hovered = hovered;
            cx.notify();
        }
    }

    /// `Mod+Shift+N` (KBD-074): focus the stack, remembering where to return to.
    pub fn focus_newest(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.list.count() == 0 {
            return;
        }
        if !self.focus.contains_focused(window, cx) {
            self.return_focus = window.focused(cx);
        }
        window.focus(&self.focus, cx);
        cx.notify();
    }

    /// Repaints and (re)starts the clock that drives animation and countdowns.
    fn wake(&mut self, cx: &mut Context<Self>) {
        cx.notify();
        if self.tick.is_some() {
            return;
        }
        self.list.last = self.now(cx);
        self.tick = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(TICK).await;
                let more = this.update(cx, |h, cx| {
                    let now = h.now(cx);
                    let more = h.list.step(now);
                    if !more {
                        h.tick = None;
                    }
                    cx.notify();
                    more
                });
                if !matches!(more, Ok(true)) {
                    break;
                }
            }
        }));
    }
}

fn ease_out(p: f32) -> f32 {
    1. - (1. - p).powi(3)
}

fn progress(since: Duration, over: Duration) -> f32 {
    (since.as_secs_f32() / over.as_secs_f32()).clamp(0., 1.)
}

fn accent(kind: Option<ToastKind>, cx: &App) -> Hsla {
    let theme = cx.theme();
    match kind {
        Some(ToastKind::Info) => theme.info,
        Some(ToastKind::Success) => theme.success,
        Some(ToastKind::Error) => theme.danger,
        None => theme.primary,
    }
}

fn kind_icon(kind: ToastKind) -> IconName {
    match kind {
        ToastKind::Info => IconName::Info,
        ToastKind::Success => IconName::CircleCheck,
        ToastKind::Error => IconName::CircleX,
    }
}

/// The bar along the bottom edge. Inset by the corner radius so it clears the rounded
/// corners; the leading end is brightest so the shrinking edge reads clearly.
fn countdown_bar(fraction: f32, color: Hsla, radius: Pixels) -> impl IntoElement {
    div()
        .absolute()
        .left(radius)
        .right(radius)
        .bottom(px(1.))
        .h(BAR_HEIGHT)
        .rounded_full()
        .bg(color.opacity(0.14))
        .child(
            div()
                .h_full()
                .w(relative(fraction))
                .rounded_full()
                .bg(linear_gradient(
                    90.,
                    linear_color_stop(color.opacity(0.35), 0.),
                    linear_color_stop(color, 1.),
                )),
        )
}

fn action_button(toast_id: u64, action: &ToastAction, cx: &mut Context<ToastHost>) -> AnyElement {
    let (run, dismiss) = (action.run.clone(), action.dismiss);
    let selector = action.id.clone();
    let button = Button::new(action.id.clone())
        .small()
        .label(action.label.clone())
        .debug_selector(move || selector.to_string());
    let button = match action.style {
        ActionStyle::Default => button,
        ActionStyle::Primary => button.primary(),
        ActionStyle::Ghost => button.ghost(),
    };
    button
        .when_some(action.tooltip.clone(), |b, t| b.tooltip(t))
        .on_click(cx.listener(move |host, _, window, cx| {
            // The card's own click (the toast body) mustn't fire too.
            cx.stop_propagation();
            if dismiss {
                host.dismiss(toast_id, cx);
            }
            let run = run.clone();
            window.defer(cx, move |window, cx| run(window, cx));
        }))
        .into_any_element()
}

fn card(l: &Live, now: Duration, window: &mut Window, cx: &mut Context<ToastHost>) -> AnyElement {
    let id = l.id;
    let color = accent(l.toast.kind, cx);
    let theme = cx.theme();
    let (bg, border, radius, muted, foreground) = (
        theme.popover,
        theme.border,
        theme.radius_lg,
        theme.muted_foreground,
        theme.popover_foreground,
    );
    let enter = ease_out(progress(now.saturating_sub(l.born), ENTER));
    let exit = l
        .dying
        .map_or(0., |d| progress(now.saturating_sub(d), EXIT));
    let alpha = enter * (1. - exit);
    let lift = (1. - enter) * SLIDE_IN + exit * SLIDE_OUT;
    let ink = hsla(0., 0., 0., 0.1 * alpha.powi(3));
    let shadow = vec![
        BoxShadow::new(px(0.), px(10.), ink)
            .blur_radius(px(7.5))
            .spread_radius(px(-3.)),
        BoxShadow::new(px(0.), px(4.), ink)
            .blur_radius(px(3.))
            .spread_radius(px(-4.)),
    ];
    let fraction = l
        .remaining
        .map(|r| (r.as_secs_f32() / AUTOHIDE.as_secs_f32()).clamp(0., 1.));
    let body = l.toast.body.as_ref().map(|b| b(window, cx));
    let has_title = l.toast.title.is_some();
    let actions: Vec<_> = l
        .toast
        .actions
        .iter()
        .map(|a| action_button(id, a, cx))
        .collect();
    let on_click = l.toast.on_click.clone();

    h_flex()
        .id(("toast", id as usize))
        .group("toast")
        .occlude()
        .relative()
        .top(px(lift))
        .opacity(alpha)
        .w_full()
        .items_start()
        .gap_3()
        .py_3p5()
        .px_4()
        .border_1()
        .border_color(border)
        .bg(bg)
        .text_color(foreground)
        .rounded(radius)
        .shadow(shadow)
        // A click must not focus the stack: focus pauses the countdown.
        .on_mouse_down(MouseButton::Left, |_, window, _| window.prevent_default())
        .on_hover(
            cx.listener(move |host, hovered: &bool, _, cx| host.set_hovered(id, *hovered, cx)),
        )
        .when_some(on_click, |this, on_click| {
            this.cursor_pointer()
                .on_click(cx.listener(move |host, e: &ClickEvent, window, cx| {
                    host.dismiss(id, cx);
                    let (on_click, e) = (on_click.clone(), e.clone());
                    window.defer(cx, move |window, cx| on_click(&e, window, cx));
                }))
        })
        .when_some(l.toast.kind, |this, kind| {
            this.child(
                div()
                    .flex_none()
                    .size_7()
                    .rounded_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .bg(color.opacity(0.14))
                    .child(Icon::new(kind_icon(kind)).small().text_color(color)),
            )
        })
        .child(
            v_flex()
                .flex_1()
                .min_w_0()
                .gap_0p5()
                .when_some(l.toast.title.clone(), |this, title| {
                    this.child(
                        div()
                            .text_sm()
                            .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                            .child(title),
                    )
                })
                .when_some(l.toast.message.clone(), |this, message| {
                    this.child(
                        div()
                            .text_sm()
                            .when(has_title, |d| d.text_color(muted))
                            .child(message),
                    )
                })
                .children(body),
        )
        .when(!actions.is_empty(), |this| {
            this.child(h_flex().flex_none().self_center().gap_1().children(actions))
        })
        .when_some(fraction, |this, fraction| {
            this.child(countdown_bar(fraction, color, radius))
        })
        .child(
            div()
                .absolute()
                .top_1()
                .right_1()
                .invisible()
                .group_hover("toast", |this| this.visible())
                .child(
                    Button::new(("toast-close", id as usize))
                        .icon(IconName::Close)
                        .ghost()
                        .xsmall()
                        .on_click(cx.listener(move |host, _, _, cx| {
                            cx.stop_propagation();
                            host.dismiss(id, cx);
                        })),
                ),
        )
        .into_any_element()
}

impl Render for ToastHost {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.list.live.is_empty() {
            // The toast that held focus is gone.
            if std::mem::take(&mut self.list.focused)
                && let Some(handle) = self.return_focus.take()
            {
                window.focus(&handle, cx);
            }
            return div().into_any_element();
        }
        self.list.focused = self.focus.contains_focused(window, cx);
        let now = self.now(cx);
        let hidden = self.list.live.len().saturating_sub(MAX_VISIBLE);
        let cards: Vec<AnyElement> = self
            .list
            .live
            .iter()
            .skip(hidden)
            .map(|l| card(l, now, window, cx))
            .collect();
        v_flex()
            .id("toasts")
            .debug_selector(|| "toasts".into())
            .key_context(ctx::TOASTS)
            .track_focus(&self.focus)
            .absolute()
            .right(MARGIN)
            .bottom(MARGIN)
            .w(WIDTH)
            .max_w_full()
            .gap_2()
            .on_action(cx.listener(|host, _: &Dismiss, _, cx| {
                if let Some(id) = host.list.newest() {
                    host.dismiss(id, cx);
                }
            }))
            .children(cards)
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn secs(s: f32) -> Duration {
        Duration::from_secs_f32(s)
    }

    /// Steps `list` from `from` to `to` in 16 ms ticks, like the host's clock.
    fn run(list: &mut ToastList, from: Duration, to: Duration) {
        let mut t = from;
        while t < to {
            t += TICK;
            list.step(t);
        }
    }

    fn remaining(list: &ToastList) -> Duration {
        list.live[0].remaining.expect("autohiding")
    }

    #[test]
    fn shl_003_autohiding_toast_counts_down_after_it_settles() {
        let mut list = ToastList::default();
        list.push(Toast::info("saved"), Duration::ZERO);
        run(&mut list, Duration::ZERO, ENTER / 2);
        assert_eq!(
            remaining(&list),
            AUTOHIDE,
            "the bar is full while it enters"
        );
        run(&mut list, ENTER / 2, ENTER + secs(1.));
        let left = remaining(&list);
        assert!(left < AUTOHIDE - secs(0.9) && left > AUTOHIDE - secs(1.1));
    }

    #[test]
    fn shl_003_toast_leaves_after_its_time_and_exit() {
        let mut list = ToastList::default();
        list.push(Toast::info("saved"), Duration::ZERO);
        let gone = ENTER + AUTOHIDE + EXIT + secs(0.1);
        run(&mut list, Duration::ZERO, ENTER + AUTOHIDE + secs(0.05));
        assert_eq!(list.count(), 0, "dismissed once the time is up");
        assert_eq!(list.live.len(), 1, "still fading out");
        run(&mut list, ENTER + AUTOHIDE + secs(0.05), gone);
        assert!(list.live.is_empty());
        assert!(
            !list.step(gone + TICK),
            "the clock stops when nothing is left"
        );
    }

    #[test]
    fn shl_003_hover_and_focus_pause_the_countdown() {
        let mut list = ToastList::default();
        list.push(Toast::info("saved"), Duration::ZERO);
        run(&mut list, Duration::ZERO, ENTER + secs(1.));
        let before = remaining(&list);

        list.live[0].hovered = true;
        run(&mut list, ENTER + secs(1.), ENTER + secs(3.));
        assert_eq!(remaining(&list), before, "hovered");

        list.live[0].hovered = false;
        list.focused = true;
        run(&mut list, ENTER + secs(3.), ENTER + secs(5.));
        assert_eq!(remaining(&list), before, "focused");

        list.focused = false;
        run(&mut list, ENTER + secs(5.), ENTER + secs(6.));
        assert!(remaining(&list) < before);
    }

    #[test]
    fn kbd_074_toasts_with_actions_or_errors_stay() {
        let mut list = ToastList::default();
        list.push(Toast::error("boom"), Duration::ZERO);
        list.push(
            Toast::info("update").action(ToastAction::new("go", "Go", |_, _| {})),
            Duration::ZERO,
        );
        list.push(Toast::custom().autohide(false), Duration::ZERO);
        run(&mut list, Duration::ZERO, secs(30.));
        assert_eq!(list.count(), 3);
        assert!(list.live.iter().all(|l| l.remaining.is_none()));
        assert!(!list.step(secs(31.)), "nothing left to animate or count");
    }

    #[test]
    fn shl_003_same_key_replaces_and_remove_finds_it() {
        let mut list = ToastList::default();
        list.push(Toast::custom().key("pull-1"), Duration::ZERO);
        list.push(Toast::custom().key("pull-1"), secs(1.));
        list.push(Toast::custom().key("pull-2"), secs(1.));
        assert_eq!(list.count(), 2);

        list.remove_key("pull-1", secs(2.));
        assert_eq!(list.count(), 1);
        assert_eq!(list.live.iter().filter(|l| l.dying.is_some()).count(), 1);
    }

    #[test]
    fn kbd_074_newest_skips_toasts_that_are_leaving() {
        let mut list = ToastList::default();
        let first = list.push(Toast::info("a"), Duration::ZERO);
        let second = list.push(Toast::info("b"), Duration::ZERO);
        assert_eq!(list.newest(), Some(second));
        list.dismiss(second, secs(1.));
        assert_eq!(list.newest(), Some(first));
        list.dismiss(first, secs(1.));
        assert_eq!(list.newest(), None);
    }

    #[test]
    fn shl_003_hidden_overflow_keeps_counting() {
        let mut list = ToastList::default();
        for n in 0..MAX_VISIBLE + 2 {
            list.push(Toast::info(format!("{n}")), Duration::ZERO);
        }
        run(&mut list, Duration::ZERO, ENTER + AUTOHIDE + secs(0.1));
        assert_eq!(
            list.count(),
            0,
            "toasts past the visible limit also time out"
        );
    }
}
