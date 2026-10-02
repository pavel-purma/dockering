//! Native menus (SHL-020, macOS) — the same actions as the keymap (KBD-002). On Windows and
//! Linux there's no menu bar; the title-bar overflow menu and the palette list the commands
//! (SHL-021).

use gpui_kit::component::menu::PopupMenu;
use gpui_kit::{App, Menu, MenuItem, OsAction};

use crate::actions::*;
use crate::strings as s;

/// macOS app menu.
pub fn set_app_menus(cx: &mut App) {
    if !cfg!(target_os = "macos") {
        return;
    }
    cx.set_menus(vec![
        Menu {
            name: s::APP_NAME.into(),
            items: vec![
                MenuItem::action(s::CMD_ABOUT, About),
                MenuItem::separator(),
                MenuItem::action(s::CMD_SETTINGS, OpenSettings),
                MenuItem::separator(),
                MenuItem::action(s::CMD_HIDE, Hide),
                MenuItem::action(s::CMD_HIDE_OTHERS, HideOthers),
                MenuItem::separator(),
                MenuItem::action(s::CMD_QUIT, Quit),
            ],
            disabled: false,
        },
        Menu {
            name: s::MENU_EDIT.into(),
            items: vec![
                MenuItem::os_action(
                    s::CMD_UNDO,
                    gpui_kit::component::input::Undo,
                    OsAction::Undo,
                ),
                MenuItem::os_action(
                    s::CMD_REDO,
                    gpui_kit::component::input::Redo,
                    OsAction::Redo,
                ),
                MenuItem::separator(),
                MenuItem::os_action(s::CMD_CUT, gpui_kit::component::input::Cut, OsAction::Cut),
                MenuItem::os_action(
                    s::CMD_COPY,
                    gpui_kit::component::input::Copy,
                    OsAction::Copy,
                ),
                MenuItem::os_action(
                    s::CMD_PASTE,
                    gpui_kit::component::input::Paste,
                    OsAction::Paste,
                ),
                MenuItem::os_action(
                    s::CMD_SELECT_ALL_TEXT,
                    gpui_kit::component::input::SelectAll,
                    OsAction::SelectAll,
                ),
            ],
            disabled: false,
        },
        Menu {
            name: s::MENU_VIEW.into(),
            items: vec![
                MenuItem::action(s::CMD_GO_CONTAINERS, GoContainers),
                MenuItem::action(s::CMD_GO_IMAGES, GoImages),
                MenuItem::action(s::CMD_GO_VOLUMES, GoVolumes),
                MenuItem::action(s::CMD_GO_NETWORKS, GoNetworks),
                MenuItem::separator(),
                MenuItem::action(s::CMD_TOGGLE_SIDEBAR, ToggleSidebar),
                MenuItem::action(s::CMD_PALETTE, CommandPalette),
                MenuItem::action(s::CMD_TOGGLE_THEME, ToggleTheme),
                MenuItem::separator(),
                MenuItem::action(s::CMD_ZOOM_IN, ZoomIn),
                MenuItem::action(s::CMD_ZOOM_OUT, ZoomOut),
                MenuItem::action(s::CMD_ZOOM_RESET, ZoomReset),
            ],
            disabled: false,
        },
        Menu {
            name: s::MENU_WINDOW.into(),
            items: vec![
                MenuItem::action(s::CMD_MINIMIZE, Minimize),
                MenuItem::action(s::CMD_ZOOM_WINDOW, ZoomWindow),
            ],
            disabled: false,
        },
        Menu {
            name: s::MENU_HELP.into(),
            items: vec![
                MenuItem::action(s::CMD_SHORTCUTS, ShortcutReference),
                MenuItem::action(s::CMD_OPEN_LOGS_FOLDER, OpenLogsFolder),
            ],
            disabled: false,
        },
    ]);
}

/// Windows/Linux title-bar overflow menu (SHL-021): the same commands as the macOS menus.
pub fn overflow_menu(menu: PopupMenu) -> PopupMenu {
    menu.menu(s::CMD_PALETTE, Box::new(CommandPalette))
        .menu(s::CMD_ENGINE_SWITCHER, Box::new(EngineSwitcher))
        .separator()
        .menu(s::CMD_GO_CONTAINERS, Box::new(GoContainers))
        .menu(s::CMD_GO_IMAGES, Box::new(GoImages))
        .menu(s::CMD_GO_VOLUMES, Box::new(GoVolumes))
        .menu(s::CMD_GO_NETWORKS, Box::new(GoNetworks))
        .menu(s::CMD_SETTINGS, Box::new(OpenSettings))
        .separator()
        .menu(s::CMD_TOGGLE_SIDEBAR, Box::new(ToggleSidebar))
        .menu(s::CMD_TOGGLE_THEME, Box::new(ToggleTheme))
        .menu(s::CMD_ZOOM_IN, Box::new(ZoomIn))
        .menu(s::CMD_ZOOM_OUT, Box::new(ZoomOut))
        .menu(s::CMD_ZOOM_RESET, Box::new(ZoomReset))
        .separator()
        .menu(s::CMD_SHORTCUTS, Box::new(ShortcutReference))
        .menu(s::CMD_OPEN_LOGS_FOLDER, Box::new(OpenLogsFolder))
        .menu(s::CMD_ABOUT, Box::new(About))
        .separator()
        .menu(s::CMD_QUIT, Box::new(Quit))
}
