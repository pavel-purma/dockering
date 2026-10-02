//! Sidebar navigation items (spec 30 §1, KBD-005). A custom `SidebarItem` because the
//! GPUI Kit `SidebarMenuItem` paints hover and the active page with the same token, so the
//! selected page doesn't stand out. Here hover is a quiet neutral tint, and the active page
//! gets an accent tint and an accent icon. The keyboard cursor (sidebar region focused)
//! draws the focus ring on the item itself, not around the whole region, so focus never
//! changes the sidebar's size (KBD-003).
//!
//! The same items render the main pages and, on Settings routes, *Back to <Page>* plus the
//! settings sections (SET-080), so both menus look and behave alike.

use gpui_kit::component::sidebar::SidebarItem;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{ActiveTheme, Collapsible, Icon, Sizable, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::{
    App, ElementId, IntoElement, MouseButton, SharedString, Window, div, px, transparent_black,
};

use crate::actions::Navigate;
use crate::nav::Route;
use crate::theme::accent_text;

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
    fn render(
        self,
        id: impl Into<ElementId>,
        _window: &mut Window,
        cx: &mut App,
    ) -> impl IntoElement {
        let collapsed = self.collapsed;
        let heading_fg = cx.theme().sidebar_foreground.opacity(0.7);
        let divider = cx.theme().sidebar_border;
        let mut children = Vec::new();
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
                children.push(render_item(ix, item, collapsed, cx));
                ix += 1;
            }
        }
        v_flex().id(id.into()).gap_0p5().children(children)
    }
}

fn render_item(ix: usize, item: NavItem, collapsed: bool, cx: &App) -> gpui_kit::AnyElement {
    let theme = cx.theme();
    let accent = accent_text(theme.is_dark());
    let label = item.label.clone();
    let route = item.route.clone();
    let (bg, fg) = if item.active {
        (theme.primary.opacity(0.16), theme.foreground)
    } else {
        (transparent_black(), theme.muted_foreground)
    };
    let hover_bg = theme.sidebar_accent;
    let hover_fg = theme.foreground;
    let ring = theme.ring;
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
        // Always 1 px, transparent unless it's the cursor: focus never shifts layout.
        .border_1()
        .border_color(if item.cursor {
            ring
        } else {
            transparent_black()
        })
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
        // new page takes focus (KBD-007) and no focus ring appears on the sidebar.
        .on_mouse_down(MouseButton::Left, |_, window, _| window.prevent_default())
        .on_click(move |_, window, cx| {
            window.dispatch_action(
                Box::new(Navigate {
                    route: route.clone(),
                }),
                cx,
            )
        })
        .into_any_element()
}
