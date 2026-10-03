//! Start/stop row action buttons: a chalk-tinted outline icon that fills with the same
//! colour while the pointer is over that button.
//!
//! Custom element: GPUI Kit's `Button` can't swap its icon on hover, and the Lucide `play`
//! and `square` glyphs are outlines only, so the button's content is the outline icon with
//! a filled copy layered on top, shown by a `group_hover` on the button itself.

use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::{Disableable, Icon, IconName, Sizable};
use gpui_kit::prelude::*;
use gpui_kit::{Action, App, ElementId, FocusHandle, SharedString, div, svg};

use crate::assets::Lucide;
use crate::ui::dispatch;
use crate::ui::status_chip::{Tone, chalk};

/// Lucide `play`, filled.
const PLAY_FILLED: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="currentColor" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M5 5a2 2 0 0 1 3.008-1.728l11.997 6.998a2 2 0 0 1 .003 3.458l-12 7A2 2 0 0 1 5 19z"/></svg>"#;
/// Lucide `square`, filled.
const SQUARE_FILLED: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="currentColor" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><rect width="18" height="18" x="3" y="3" rx="2"/></svg>"#;

/// Which glyph and tint a [`state_button`] shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StateIcon {
    /// Play, chalk green (start, run).
    Start,
    /// Square, chalk red (stop).
    Stop,
}

impl StateIcon {
    fn outline(self) -> Icon {
        match self {
            StateIcon::Start => Icon::new(IconName::Play),
            StateIcon::Stop => Icon::new(Lucide::Square),
        }
    }

    fn filled(self) -> &'static [u8] {
        match self {
            StateIcon::Start => PLAY_FILLED,
            StateIcon::Stop => SQUARE_FILLED,
        }
    }

    fn tone(self) -> Tone {
        match self {
            StateIcon::Start => Tone::Success,
            StateIcon::Stop => Tone::Danger,
        }
    }
}

/// An xsmall ghost row button with a [`StateIcon`]: chalk outline at rest, filled while
/// hovered. Disabled buttons keep the kit's plain disabled icon. Not a Tab stop (the table
/// is one stop, KBD-004); the click dispatches `action` from `origin` (KBD-002) and doesn't
/// open the row.
pub fn state_button(
    id: impl Into<ElementId>,
    group: impl Into<SharedString>,
    icon: StateIcon,
    tooltip: impl Into<SharedString>,
    disabled: bool,
    (origin, action): (&FocusHandle, Box<dyn Action>),
    cx: &App,
) -> Button {
    let group: SharedString = group.into();
    let button = Button::new(id)
        .ghost()
        .xsmall()
        .tooltip(tooltip)
        .disabled(disabled)
        .tab_stop(false)
        .on_click(dispatch::on_click_stop(origin, action));
    if disabled {
        return button.icon(icon.outline());
    }
    let color = chalk(icon.tone(), cx);
    button.group(group.clone()).child(
        div()
            .relative()
            .size_3()
            .child(icon.outline().size_3().text_color(color))
            .child(
                svg()
                    .data(icon.filled())
                    .absolute()
                    .top_0()
                    .left_0()
                    .size_3()
                    .text_color(color)
                    .invisible()
                    .group_hover(group, |s| s.visible()),
            ),
    )
}
