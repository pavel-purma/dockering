//! Sidebar navigation items (spec 30 §1, KBD-005). A custom `SidebarItem` because the
//! GPUI Kit `SidebarMenuItem` paints hover and the active page with the same token, so the
//! selected page doesn't stand out. Here hover is a quiet neutral tint, and the active page
//! gets an accent tint and an accent icon. The keyboard cursor (sidebar region focused)
//! gets the hover tint; nothing draws a focus ring.
//!
//! The same items render the main pages and, on Settings routes, *Back to <Page>* plus the
//! settings sections (SET-080), so both menus look and behave alike. The footer's
//! [`EngineCard`] shows the active engine and opens the switcher (ENG-165).

use gpui_kit::component::sidebar::SidebarItem;
use gpui_kit::component::tag::Tag;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{ActiveTheme, Collapsible, Icon, IconName, Sizable, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::{
    App, ElementId, FocusHandle, Hsla, IntoElement, MouseButton, SharedString, Window, div, px,
    transparent_black,
};

use crate::actions::{EngineSwitcher, Navigate};
use crate::nav::Route;
use crate::strings as s;
use crate::theme::accent_text;
use crate::ui::dispatch;
use crate::ui::status_chip::dot;

/// One sidebar entry: a page, a settings section, or *Back*.
#[derive(Clone)]
pub struct NavItem {
    pub label: SharedString,
    /// Where a click navigates.
    pub route: Route,
    pub icon: Icon,
    pub count: Option<usize>,
    /// The current route's entry.
    pub active: bool,
    /// The keyboard cursor while the sidebar region has focus.
    pub cursor: bool,
}

/// The [`NavItem`]s of one sidebar, in groups. A group may carry a heading (styled like a
/// GPUI Kit `SidebarGroup` label); collapsed, headings become a thin divider.
#[derive(Clone)]
pub struct NavMenu {
    groups: Vec<(Option<SharedString>, Vec<NavItem>)>,
    collapsed: bool,
}

impl NavMenu {
    pub fn new(items: Vec<NavItem>) -> Self {
        Self {
            groups: vec![(None, items)],
            collapsed: false,
        }
    }

    /// Appends a group under `heading`.
    pub fn group(mut self, heading: impl Into<SharedString>, items: Vec<NavItem>) -> Self {
        self.groups.push((Some(heading.into()), items));
        self
    }
}

impl Collapsible for NavMenu {
    fn collapsed(mut self, collapsed: bool) -> Self {
        self.collapsed = collapsed;
        self
    }
    fn is_collapsed(&self) -> bool {
        self.collapsed
    }
}

impl SidebarItem for NavMenu {
    fn render(self, id: impl Into<ElementId>, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let collapsed = self.collapsed;
        let heading_fg = cx.theme().sidebar_foreground.opacity(0.7);
        let divider = cx.theme().sidebar_border;
        let anchor = dispatch::DispatchAnchor::new(cx);
        let mut children = vec![anchor.element().into_any_element()];
        let mut ix = 0;
        for (heading, items) in self.groups {
            if let Some(heading) = heading {
                children.push(if collapsed {
                    div().my_1().mx_2().h(px(1.)).bg(divider).into_any_element()
                } else {
                    h_flex()
                        .mt_2()
                        .px_2()
                        .h_8()
                        .text_xs()
                        .text_color(heading_fg)
                        .child(heading)
                        .into_any_element()
                });
            }
            for item in items {
                children.push(render_item(ix, item, collapsed, anchor.handle(), cx));
                ix += 1;
            }
        }
        v_flex()
            .id(id.into())
            .relative()
            .gap_0p5()
            .children(children)
    }
}

fn render_item(
    ix: usize,
    item: NavItem,
    collapsed: bool,
    origin: &FocusHandle,
    cx: &App,
) -> gpui_kit::AnyElement {
    let theme = cx.theme();
    let accent = accent_text(theme.is_dark());
    let label = item.label.clone();
    let route = item.route.clone();
    let hover_bg = theme.sidebar_accent;
    // The keyboard cursor reads like hover: no ring, no border.
    let (bg, fg) = if item.active {
        (theme.primary.opacity(0.16), theme.foreground)
    } else if item.cursor {
        (hover_bg, theme.foreground)
    } else {
        (transparent_black(), theme.muted_foreground)
    };
    let hover_fg = theme.foreground;
    let badge_bg = if item.active {
        theme.primary.opacity(0.22)
    } else {
        theme.muted
    };
    let badge_fg = if item.active {
        theme.foreground
    } else {
        theme.muted_foreground
    };
    let radius = theme.radius;
    let icon = item
        .icon
        .small()
        .when(item.active, |this| this.text_color(accent));
    h_flex()
        .id(("nav-item", ix))
        .role(gpui_kit::Role::TreeItem)
        .aria_label(label.clone())
        .aria_selected(item.active)
        .h(px(32.))
        .px_2()
        .gap_2()
        .items_center()
        .rounded(radius)
        .text_sm()
        .cursor_pointer()
        .bg(bg)
        .text_color(fg)
        .when(item.active, |this| {
            this.font_weight(gpui_kit::FontWeight::MEDIUM)
        })
        .when(!item.active, |this| {
            this.hover(move |s| s.bg(hover_bg).text_color(hover_fg))
        })
        .when(collapsed, |this| this.justify_center())
        .child(icon)
        .when(!collapsed, |this| {
            this.child(div().flex_1().truncate().child(label.clone()))
                .when_some(item.count, |this, n| {
                    this.child(
                        div()
                            .text_xs()
                            .px_1p5()
                            .rounded(radius)
                            .bg(badge_bg)
                            .text_color(badge_fg)
                            .child(n.to_string()),
                    )
                })
        })
        .when(collapsed, |this| {
            this.tooltip(move |window, cx| Tooltip::new(label.clone()).build(window, cx))
        })
        // A mouse click navigates without moving focus into the sidebar region, so the
        // new page takes focus (KBD-007) and no cursor appears on the sidebar.
        .on_mouse_down(MouseButton::Left, |_, window, _| window.prevent_default())
        .on_click(dispatch::on_click(origin, Box::new(Navigate { route })))
        .into_any_element()
}

/// The active engine in the sidebar footer (ENG-165): a card, or an icon button when collapsed.
pub struct EngineCard {
    pub name: SharedString,
    pub icon: IconName,
    pub dot: Hsla,
    /// The engine's state label; empty without an engine.
    pub state: &'static str,
    pub insecure: bool,
    /// The keyboard cursor while the sidebar region has focus.
    pub cursor: bool,
    pub collapsed: bool,
}

impl EngineCard {
    pub fn render(self, origin: &FocusHandle, cx: &App) -> gpui_kit::AnyElement {
        let theme = cx.theme();
        let hover_bg = theme.sidebar_accent;
        let muted = theme.muted_foreground;
        let radius = theme.radius;
        let collapsed = self.collapsed;
        let tip_name = collapsed.then(|| self.name.clone());
        let tip_insecure = collapsed && self.insecure;
        let state = self.state;
        let card = h_flex()
            .id("sidebar-engine")
            .debug_selector(|| "sidebar-engine".into())
            .role(gpui_kit::Role::Button)
            .aria_label(SharedString::from(s::SWITCH_ENGINE))
            .relative()
            .h(px(32.))
            .gap_2()
            .items_center()
            .rounded(radius)
            .text_sm()
            .cursor_pointer()
            .when(self.cursor, |this| this.bg(hover_bg))
            .hover(move |s| s.bg(hover_bg))
            .tooltip(move |window, cx| {
                let name = tip_name.clone();
                Tooltip::element(move |_, cx| {
                    let muted = cx.theme().muted_foreground;
                    let detail = match (&name, state.is_empty()) {
                        (Some(name), false) => Some(s::engine_and_state(name, state)),
                        (Some(name), true) => Some(name.to_string()),
                        (None, false) => Some(state.to_owned()),
                        (None, true) => None,
                    };
                    v_flex()
                        .child(s::SWITCH_ENGINE)
                        .when_some(detail, |this, d| {
                            this.child(div().text_xs().text_color(muted).child(d))
                        })
                        .when(tip_insecure, |this| {
                            this.child(
                                div()
                                    .text_xs()
                                    .text_color(cx.theme().warning)
                                    .child(s::INSECURE_TCP),
                            )
                        })
                })
                .action(&EngineSwitcher, None)
                .build(window, cx)
            })
            // Like a nav item: a click doesn't move focus into the sidebar region.
            .on_mouse_down(MouseButton::Left, |_, window, _| window.prevent_default())
            .on_click(dispatch::on_click(origin, Box::new(EngineSwitcher)));
        let card = if collapsed {
            card.size(px(32.))
                .justify_center()
                .child(Icon::new(IconName::ChevronsUpDown).small())
                .child(
                    div()
                        .absolute()
                        .top(px(4.))
                        .right(px(4.))
                        .child(dot(self.dot)),
                )
        } else {
            card.w_full()
                .px_2()
                .text_color(theme.foreground)
                .child(Icon::new(self.icon).small())
                .child(div().flex_1().min_w_0().truncate().child(self.name))
                .when(self.insecure, |this| {
                    this.child(
                        div()
                            .id("sidebar-engine-insecure")
                            .tooltip(|window, cx| {
                                Tooltip::new(s::INSECURE_TCP_TOOLTIP).build(window, cx)
                            })
                            .child(Tag::warning().xsmall().child(s::INSECURE_TAG)),
                    )
                })
                .child(dot(self.dot))
                .child(
                    Icon::new(IconName::ChevronsUpDown)
                        .small()
                        .text_color(muted),
                )
        };
        div()
            .w_full()
            .pt_2()
            .border_t_1()
            .border_color(theme.sidebar_border)
            .when(collapsed, |this| this.flex().justify_center())
            .child(card)
            .into_any_element()
    }
}
