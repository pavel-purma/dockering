//! The title-bar update button (UPD-015) and the once-per-version notification (UPD-008).
//!
//! Both come from the retained offer alone ([`UpdateOffer`]), never from the check activity, so
//! a re-check, a failed check, an offline start or the automatic-check switch can't hide them.
//! The rules are pure ([`button_for`], [`notice_for`]) and tested row by row; the builders
//! below only draw what the rules say. Nothing here reads the operating system: the button is
//! the same on Windows, macOS and Linux (a view test scans this file for OS checks).
//!
//! No action of its own: a click runs `RestartToUpdate` or `ViewReleaseNotes`, both palette
//! entries (KBD-076), dispatched from the title bar with `ui::dispatch` (KBD-002).

use dk_hub::{OfferMode, OfferStage, UpdateOffer};
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::{Disableable, Sizable};
use gpui_kit::prelude::*;
use gpui_kit::{Action, AnyElement, App, FocusHandle, IntoElement, Window};

use crate::actions::{RestartToUpdate, ViewReleaseNotes};
use crate::assets::Lucide;
use crate::strings as s;
use crate::ui::dispatch;
use crate::ui::toast::{self, Toast, ToastAction};
use crate::ui::widgets::focus_wrap;

/// The element id of the title-bar button (also its `debug_selector`).
pub const BUTTON_ID: &str = "title-update";

/// What a click runs. Both are existing actions (KBD-076).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    /// Starts the install, or only retries starting the new process (*Installed, restart*).
    RestartToUpdate,
    /// Opens the release page (a *Download* offer).
    ViewReleaseNotes,
}

impl Command {
    fn action(self) -> Box<dyn Action> {
        match self {
            Self::RestartToUpdate => Box::new(RestartToUpdate),
            Self::ViewReleaseNotes => Box::new(ViewReleaseNotes),
        }
    }
}

/// How the title-bar button looks and what it does for one offer (the table of plan §6.2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ButtonSpec {
    /// The offered version (feeds the accessible name).
    pub version: String,
    /// The accent (`primary`) button: a verified install, ready to restart.
    pub accent: bool,
    /// Disabled with a spinner: *Restart to update* is running.
    pub busy: bool,
    /// Tooltip text; none while busy.
    pub tooltip: Option<String>,
    /// What a click (or Enter on the focused button) runs; none while busy.
    pub command: Option<Command>,
}

impl ButtonSpec {
    /// The accessible name: starts with 'Update available' (UPD-015).
    pub fn name(&self) -> String {
        s::upd_button_name(&self.version)
    }
}

/// The button for `offer`, or `None` when it is hidden: no offer is handled by the caller; the
/// *Downloading* stage has no button (the status bar shows the progress). `applying_here` is
/// this window's own *Restart to update* in flight, so the button turns busy at the click,
/// before the hub's snapshot says so (SHL-001).
#[must_use]
pub fn button_for(offer: &UpdateOffer, applying_here: bool) -> Option<ButtonSpec> {
    let version = offer.version.clone();
    if offer.stage == OfferStage::Downloading {
        return None;
    }
    // A *Download* offer: this install can't update itself (UPD-006), or installing failed.
    if offer.mode == OfferMode::Download || offer.install_failed {
        return Some(ButtonSpec {
            tooltip: Some(s::upd_tooltip_available(&version)),
            version,
            accent: false,
            busy: false,
            command: Some(Command::ViewReleaseNotes),
        });
    }
    if offer.stage == OfferStage::Applying || applying_here {
        return Some(ButtonSpec {
            version,
            accent: false,
            busy: true,
            tooltip: None,
            command: None,
        });
    }
    let (accent, tooltip) = match offer.stage {
        OfferStage::Ready => (true, s::upd_restart(&version, offer.needs_elevation)),
        OfferStage::InstalledRestart => (false, s::UPD_TOOLTIP_INSTALLED_RESTART.to_owned()),
        OfferStage::Downloading | OfferStage::Applying => return None,
    };
    Some(ButtonSpec {
        version,
        accent,
        busy: false,
        tooltip: Some(tooltip),
        command: Some(Command::RestartToUpdate),
    })
}

/// The title-bar button. `wrap` is the focus handle the shell owns (so focus can go back to the
/// title bar when the button disappears, UPD-015); `origin` is the title region the click
/// dispatches from (KBD-002). The group it sits in `occlude()`s the window drag area.
pub fn render_button(
    spec: &ButtonSpec,
    wrap: &FocusHandle,
    origin: &FocusHandle,
    cx: &App,
) -> AnyElement {
    let mut button = Button::new(BUTTON_ID)
        .small()
        .icon(Lucide::CircleArrowUp)
        .accessibility_label(spec.name())
        .debug_selector(|| BUTTON_ID.into());
    button = if spec.accent {
        button.primary()
    } else {
        button.ghost()
    };
    if spec.busy {
        button = button.loading(true).disabled(true);
    }
    if let Some(tip) = &spec.tooltip {
        button = match spec.command {
            Some(Command::ViewReleaseNotes) => {
                button.tooltip_with_action(tip.clone(), &ViewReleaseNotes, None)
            }
            Some(Command::RestartToUpdate) => {
                button.tooltip_with_action(tip.clone(), &RestartToUpdate, None)
            }
            None => button.tooltip(tip.clone()),
        };
    }
    if let Some(command) = spec.command {
        button = button.on_click(dispatch::on_click(origin, command.action()));
    }
    let command = spec.command;
    let origin = origin.clone();
    focus_wrap(
        "title-update-wrap",
        wrap,
        button,
        // Enter / Space on the focused wrapper do what a click does.
        move |_, window, cx| {
            if let Some(command) = command {
                dispatch::dispatch_from(&origin, command.action().as_ref(), window, cx);
            }
        },
        cx,
    )
    .into_any_element()
}

/// The one notification of a version (UPD-008).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notice {
    /// The version `mark_update_notified` is asked about.
    pub version: String,
    pub message: String,
    /// The label of the action button; a GPUI Kit notification has one.
    pub action_label: &'static str,
    /// Element id and `debug_selector` of the action button.
    pub action_id: &'static str,
    /// What the action button runs.
    pub command: Command,
}

/// The notification for `offer` when it is actionable, else `None` (nothing to announce yet,
/// or nothing to announce at all):
///
/// - install, *Ready*: 'Dockering X.Y.Z is ready to install.' with *Restart now*;
/// - *Download* (or a failed install): 'Dockering X.Y.Z is available.' with *Download*;
/// - *Downloading*, *Applying*, *Installed, restart*: none (the first never became actionable;
///   the others come after the announcement).
#[must_use]
pub fn notice_for(offer: &UpdateOffer) -> Option<Notice> {
    let version = offer.version.clone();
    if offer.stage == OfferStage::Downloading {
        return None;
    }
    if offer.mode == OfferMode::Download || offer.install_failed {
        return Some(Notice {
            message: s::upd_available(&version),
            version,
            action_label: s::UPD_DOWNLOAD,
            action_id: "upd-download",
            command: Command::ViewReleaseNotes,
        });
    }
    (offer.stage == OfferStage::Ready).then(|| Notice {
        message: s::upd_ready(&version),
        version,
        action_label: s::UPD_RESTART_NOW,
        action_id: "upd-restart-now",
        command: Command::RestartToUpdate,
    })
}

/// Shows `notice`. Toasts render outside the shell, so their clicks dispatch from `root`. The
/// body opens the release notes; the action button runs the notice's command and closes the toast
/// (an action keeps a toast from hiding by itself).
pub fn push_notice(notice: &Notice, root: &FocusHandle, _: &mut Window, cx: &mut App) {
    let (label, id, command) = (notice.action_label, notice.action_id, notice.command);
    let body_root = root.clone();
    let action_root = root.clone();
    // The toast body opens the release notes too: the action button's click doesn't reach it,
    // so *Restart now* doesn't also open a browser.
    let action = ToastAction::new(id, label, move |window, cx| {
        dispatch::dispatch_from(&action_root, command.action().as_ref(), window, cx)
    })
    .dismiss();
    let action = match command {
        Command::RestartToUpdate => action.primary(),
        Command::ViewReleaseNotes => action,
    };
    let note = Toast::info(notice.message.clone())
        .on_click(dispatch::on_click(&body_root, Box::new(ViewReleaseNotes)))
        .action(action);
    toast::push(note, cx);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn offer(mode: OfferMode, stage: OfferStage) -> UpdateOffer {
        UpdateOffer {
            version: "9.9.9".into(),
            notes_url: "https://example.test/notes".into(),
            mode,
            stage,
            needs_elevation: false,
            install_failed: false,
        }
    }

    /// One row of the stage table: what the button looks like, or `None` when it is hidden.
    type Look = Option<(bool, bool, Option<&'static str>, Option<Command>)>;

    #[test]
    fn upd_015_button_follows_offer_stage() {
        use Command::{RestartToUpdate as Restart, ViewReleaseNotes as Notes};
        use OfferMode::{Download, Install};
        use OfferStage::{Applying, Downloading, InstalledRestart, Ready};
        let admin = UpdateOffer {
            needs_elevation: true,
            ..offer(Install, Ready)
        };
        let failed = UpdateOffer {
            install_failed: true,
            ..offer(Download, Ready)
        };
        let available: Look = Some((
            false,
            false,
            Some("Dockering 9.9.9 is available"),
            Some(Notes),
        ));
        let busy: Look = Some((false, true, None, None));
        // (offer, applying here, accent, busy, tooltip, command): every row of plan §6.2.
        let rows: Vec<(UpdateOffer, bool, Look)> = vec![
            // *Downloading*: hidden (the status bar shows the progress)
            (offer(Install, Downloading), false, None),
            (offer(Download, Downloading), false, None),
            // install, *Ready*: accent, 'Restart to update (X.Y.Z)', RestartToUpdate
            (
                offer(Install, Ready),
                false,
                Some((
                    true,
                    false,
                    Some("Restart to update (9.9.9)"),
                    Some(Restart),
                )),
            ),
            // an all-users Windows install says so
            (
                admin,
                false,
                Some((
                    true,
                    false,
                    Some("Restart to update (9.9.9, admin)"),
                    Some(Restart),
                )),
            ),
            // install, *Applying*: disabled with a spinner, no click
            (offer(Install, Applying), false, busy),
            // this window's own click, before the hub reports *Applying*
            (offer(Install, Ready), true, busy),
            // *Installed, restart*: shown; a click retries the start
            (
                offer(Install, InstalledRestart),
                false,
                Some((
                    false,
                    false,
                    Some("Dockering was updated. Restart to finish"),
                    Some(Restart),
                )),
            ),
            // *Download*: shown; opens the release page
            (offer(Download, Ready), false, available),
            // a failed install degrades to *Download*
            (failed, false, available),
        ];
        for (offer, applying_here, expected) in rows {
            let got = button_for(&offer, applying_here).map(|b| {
                assert_eq!(b.version, "9.9.9");
                assert_eq!(b.name(), "Update available: Dockering 9.9.9");
                (b.accent, b.busy, b.tooltip, b.command)
            });
            let expected = expected.map(|(a, b, t, c)| (a, b, t.map(str::to_owned), c));
            assert_eq!(got, expected, "{offer:?} (applying here: {applying_here})");
        }
    }

    #[test]
    fn upd_008_notice_only_for_actionable_offers() {
        use OfferMode::{Download, Install};
        use OfferStage::{Applying, Downloading, InstalledRestart, Ready};
        let ready = notice_for(&offer(Install, Ready)).expect("ready notice");
        assert_eq!(ready.message, "Dockering 9.9.9 is ready to install.");
        assert_eq!(ready.action_label, "Restart now");
        assert_eq!(ready.command, Command::RestartToUpdate);
        let download = notice_for(&offer(Download, Ready)).expect("download notice");
        assert_eq!(download.message, "Dockering 9.9.9 is available.");
        assert_eq!(download.action_label, "Download");
        assert_eq!(download.command, Command::ViewReleaseNotes);
        let failed = UpdateOffer {
            install_failed: true,
            ..offer(Download, Ready)
        };
        assert_eq!(
            notice_for(&failed).map(|n| n.command),
            Some(Command::ViewReleaseNotes)
        );
        // Hidden or already announced stages never notify.
        for (mode, stage) in [
            (Install, Downloading),
            (Download, Downloading),
            (Install, Applying),
            (Install, InstalledRestart),
        ] {
            assert_eq!(notice_for(&offer(mode, stage)), None, "{mode:?} {stage:?}");
        }
    }
}
