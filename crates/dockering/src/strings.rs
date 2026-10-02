//! Every user-visible string lives here so the UI can be localised later (SHL-024).
//! Dynamic strings are small `fn`s that format their arguments.

pub const APP_NAME: &str = "Dockering";

// ── navigation ──────────────────────────────────────────────────────────────────────────
pub const PAGE_CONTAINERS: &str = "Containers";
pub const PAGE_IMAGES: &str = "Images";
pub const PAGE_VOLUMES: &str = "Volumes";
pub const PAGE_NETWORKS: &str = "Networks";
pub const PAGE_SETTINGS: &str = "Settings";

// ── title bar ───────────────────────────────────────────────────────────────────────────
pub const NO_ENGINE: &str = "No engine";
pub const SWITCH_ENGINE: &str = "Switch engine";
pub const REFRESH: &str = "Refresh";
pub const TOGGLE_THEME: &str = "Toggle light/dark theme";
pub const OPEN_SETTINGS: &str = "Settings";
pub const MORE_COMMANDS: &str = "More commands";
pub const INSECURE_TCP: &str = "Unencrypted TCP";
pub const INSECURE_TCP_TOOLTIP: &str =
    "This engine is reached over plain TCP without TLS. Traffic, including credentials, is not encrypted.";
pub const BACK: &str = "Back";
pub const FORWARD: &str = "Forward";
pub const TOGGLE_SIDEBAR: &str = "Toggle sidebar";

// ── engine states ───────────────────────────────────────────────────────────────────────
pub const STATE_CONNECTED: &str = "Connected";
pub const STATE_CONNECTING: &str = "Connecting…";
pub const STATE_DEGRADED: &str = "Reconnecting…";
pub const STATE_FAILED: &str = "Connection failed";
pub const STATE_DISCONNECTED: &str = "Disconnected";
pub const STATE_DISABLED: &str = "Disabled";
pub const STATE_STOPPED: &str = "Stopped";
pub const STATE_UNSUPPORTED: &str = "Unsupported";
pub const RETRY: &str = "Retry";
pub const ENGINE_UNREACHABLE_TITLE: &str = "Can't reach the engine";
pub const ENGINE_DISCONNECTED_BODY: &str =
    "The engine is not connected. Lists are hidden so you can't act on stale data.";
pub const DEGRADED_BANNER: &str =
    "Connection lost — reconnecting. Data is read-only until the engine is back.";
pub fn retry_in(seconds: u64) -> String {
    format!("Retrying automatically in {seconds} s.")
}
pub fn transport_label(transport: &str) -> String {
    format!("via {transport}")
}

// ── engine switcher ─────────────────────────────────────────────────────────────────────
pub const GROUP_LOCAL: &str = "Local";
pub const GROUP_WSL_DISTROS: &str = "WSL distros";
pub const GROUP_WSL_CONTAINERS: &str = "WSL containers";
pub const GROUP_REMOTE: &str = "Remote";
pub const GROUP_OTHER: &str = "Other";
pub const FILTER_ENGINES: &str = "Filter engines…";
pub const RESCAN: &str = "Rescan";
pub const MANAGE_ENGINES: &str = "Manage engines…";
pub const START_AND_CONNECT: &str = "Start & connect";
pub const NO_MATCHING_ENGINES: &str = "No matching engines";
pub fn also_reachable_via(endpoints: &str) -> String {
    format!("Also reachable via {endpoints}")
}

// ── first run (ENG-111) ─────────────────────────────────────────────────────────────────
pub const FIRST_RUN_TITLE: &str = "No container engine found";
pub const FIRST_RUN_BODY: &str =
    "Dockering didn't find a running container engine. Start one, then rescan.";
#[cfg(target_os = "linux")]
pub const FIRST_RUN_GUIDANCE: &[&str] = &[
    "Install Docker Engine (docs.docker.com/engine/install).",
    "Start it: sudo systemctl start docker",
    "Add your user to the docker group: sudo usermod -aG docker $USER, then log in again.",
];
#[cfg(target_os = "macos")]
pub const FIRST_RUN_GUIDANCE: &[&str] = &[
    "Install and start Docker Desktop, Colima, or OrbStack.",
    "Make sure `docker context ls` shows a running context.",
];
#[cfg(target_os = "windows")]
pub const FIRST_RUN_GUIDANCE: &[&str] = &[
    "Install and start Docker Desktop, or",
    "run Docker Engine inside a WSL distro (sudo service docker start), or",
    "update WSL (`wsl --update`) to use WSL containers.",
];
#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
pub const FIRST_RUN_GUIDANCE: &[&str] = &["Install and start a Docker-compatible engine."];
pub const ADD_ENGINE: &str = "Add engine…";
pub const NO_ACTIVE_ENGINE_TITLE: &str = "No engine connected";
pub const NO_ACTIVE_ENGINE_BODY: &str = "Pick an engine to connect to.";

// ── status bar ──────────────────────────────────────────────────────────────────────────
pub fn status_bar_engine(version: &str, api: Option<&str>, os: &str, arch: &str) -> String {
    match api {
        Some(api) => format!("Engine {version} · API {api} · {os}/{arch}"),
        None => format!("Engine {version} · {os}/{arch}"),
    }
}
pub fn status_bar_resources(cpus: Option<u32>, mem: Option<String>) -> String {
    let cpus = cpus.map_or_else(|| "—".to_owned(), |c| c.to_string());
    let mem = mem.unwrap_or_else(|| "—".to_owned());
    format!("CPUs {cpus} · RAM {mem}")
}

// ── generic list UI ─────────────────────────────────────────────────────────────────────
pub const SEARCH: &str = "Search…";
pub const QUICK_FIND: &str = "Find in list…";
pub const LOADING: &str = "Loading…";
pub const COPY_ID: &str = "Copy id";
pub const COPIED: &str = "Copied to the clipboard";
pub const COPY_DETAILS: &str = "Copy details";
pub const CANCEL: &str = "Cancel";
pub const DELETE: &str = "Delete";
pub const FORCE_DELETE: &str = "Force delete";
pub const CLEAR_SELECTION: &str = "Clear selection";
pub const LIST_LOAD_FAILED: &str = "Couldn't load the list";
pub fn selected_count(n: usize) -> String {
    format!("{n} selected")
}
pub fn reclaimable(size: &str) -> String {
    format!("{size} will be reclaimed.")
}
pub fn and_more(n: usize) -> String {
    format!("…and {n} more")
}

// ── containers ──────────────────────────────────────────────────────────────────────────
pub const COL_NAME: &str = "Name";
pub const COL_IMAGE: &str = "Image";
pub const COL_STATUS: &str = "Status";
pub const COL_CPU: &str = "CPU %";
pub const COL_MEMORY: &str = "Memory";
pub const COL_PORTS: &str = "Port(s)";
pub const COL_CREATED: &str = "Created";
pub const COL_ACTIONS: &str = "Actions";
pub const FILTER_ALL: &str = "All";
pub const FILTER_RUNNING: &str = "Running";
pub const FILTER_STOPPED: &str = "Stopped";
pub const GROUP_BY: &str = "Group by";
pub const GROUP_BY_COMPOSE: &str = "Compose project";
pub const GROUP_BY_NONE: &str = "None";
pub const GROUP_BY_LABEL: &str = "Label…";
pub const GROUP_LABEL_KEY: &str = "Label key, e.g. app";
pub const SORT_BY: &str = "Sort by";
pub const PRUNE_STOPPED: &str = "Prune stopped containers";
pub const COMPOSE_TAG: &str = "compose";
pub const ONEOFF_TAG: &str = "one-off";
pub const NO_CONTAINERS: &str = "No containers yet";
pub const NO_CONTAINERS_BODY: &str = "Containers you create or run will show up here.";
pub const NO_MATCHING_CONTAINERS: &str = "No containers match the filter";
pub const RUN_AN_IMAGE: &str = "Run an image";
pub const ACTION_START: &str = "Start";
pub const ACTION_STOP: &str = "Stop";
pub const ACTION_RESTART: &str = "Restart";
pub const ACTION_PAUSE: &str = "Pause";
pub const ACTION_UNPAUSE: &str = "Unpause";
pub const ACTION_KILL: &str = "Kill";
pub const ACTION_LOGS: &str = "View logs";
pub const ACTION_TERMINAL: &str = "Open in terminal";
pub const ACTION_INSPECT: &str = "Inspect";
pub const ACTION_OPEN_PORT: &str = "Open port in browser";
pub const ACTION_DELETE: &str = "Delete";
pub const ACTION_START_ALL: &str = "Start all";
pub const ACTION_STOP_ALL: &str = "Stop all";
pub const ACTION_RESTART_ALL: &str = "Restart all";
pub const ACTION_DELETE_ALL: &str = "Delete all";
pub const MORE_ACTIONS: &str = "More actions";
pub const DONT_ASK_AGAIN: &str = "Don't ask again for stopped containers";
pub const FORCE_RUNNING: &str = "Force-delete running containers";
pub const KEEP_NETWORKS_NOTE: &str =
    "Project networks and volumes are kept. Use `docker compose down` to remove them.";
pub fn containers_counts(running: usize, stopped: usize) -> String {
    format!("{running} running · {stopped} stopped")
}
pub fn group_running(running: usize, total: usize) -> String {
    format!("{running}/{total} running")
}
pub fn confirm_delete_title(n: usize) -> String {
    if n == 1 {
        "Delete container?".to_owned()
    } else {
        format!("Delete {n} containers?")
    }
}
pub const CONFIRM_FORCE_RUNNING_TITLE: &str = "Force delete running container?";
pub fn confirm_force_running_body(n: usize) -> String {
    if n == 1 {
        "The container is running. It will be killed and removed.".to_owned()
    } else {
        format!("{n} of the selected containers are running. They will be killed and removed.")
    }
}
pub fn confirm_delete_group_title(project: &str) -> String {
    format!("Delete all containers of {project}?")
}
pub fn confirm_prune_title(n: usize) -> String {
    format!("Prune {n} stopped containers?")
}
pub const CONFIRM_PRUNE_BODY: &str = "All stopped containers will be removed.";
pub const CONFIRM_KILL_TITLE: &str = "Kill container?";
pub const CONFIRM_KILL_BODY: &str = "The process gets SIGKILL and can't clean up.";
pub fn action_done(verb: &str, n: usize) -> String {
    if n == 1 {
        format!("{} 1 container", past_tense(verb))
    } else {
        format!("{} {n} containers", past_tense(verb))
    }
}
pub fn action_partial(verb: &str, ok: usize, failed: usize) -> String {
    format!("{} {ok}, {failed} failed", past_tense(verb))
}
pub fn action_failed(verb: &str, name: &str) -> String {
    format!("Couldn't {verb} {name}")
}
pub fn pruned(n: usize, size: &str) -> String {
    format!("Pruned {n} containers, reclaimed {size}")
}
fn past_tense(verb: &str) -> String {
    match verb {
        "start" => "Started".into(),
        "stop" => "Stopped".into(),
        "restart" => "Restarted".into(),
        "kill" => "Killed".into(),
        "pause" => "Paused".into(),
        "unpause" => "Unpaused".into(),
        "delete" | "remove" => "Deleted".into(),
        other => format!("{other}ed"),
    }
}
pub const NO_PUBLISHED_PORT: &str = "This container publishes no ports";

// ── container detail ────────────────────────────────────────────────────────────────────
pub const CONTAINER_GONE: &str = "This container no longer exists";
pub const BACK_TO_LIST: &str = "Back to list";
pub const DETAIL_COMING: &str = "Details for this container land in a later milestone.";

// ── palette & reference ─────────────────────────────────────────────────────────────────
pub const PALETTE_PLACEHOLDER: &str = "Type a command or search…";
pub const PALETTE_NO_RESULTS: &str = "No matching commands";
pub const SHORTCUTS_TITLE: &str = "Keyboard shortcuts";
pub const SHORTCUTS_FILTER: &str = "Filter shortcuts…";
pub fn go_to(kind: &str, name: &str) -> String {
    format!("Go to {kind}: {name}")
}

// ── command labels (palette, menus) ─────────────────────────────────────────────────────
pub const CMD_PALETTE: &str = "Command palette";
pub const CMD_SHORTCUTS: &str = "Keyboard shortcuts";
pub const CMD_ENGINE_SWITCHER: &str = "Switch engine…";
pub const CMD_GO_CONTAINERS: &str = "Go to Containers";
pub const CMD_GO_IMAGES: &str = "Go to Images";
pub const CMD_GO_VOLUMES: &str = "Go to Volumes";
pub const CMD_GO_NETWORKS: &str = "Go to Networks";
pub const CMD_SETTINGS: &str = "Open Settings";
pub const CMD_FOCUS_SEARCH: &str = "Focus search";
pub const CMD_REFRESH: &str = "Refresh";
pub const CMD_BACK: &str = "Go back";
pub const CMD_FORWARD: &str = "Go forward";
pub const CMD_TOGGLE_SIDEBAR: &str = "Toggle sidebar";
pub const CMD_NEXT_REGION: &str = "Focus next region";
pub const CMD_PREV_REGION: &str = "Focus previous region";
pub const CMD_TOGGLE_THEME: &str = "Toggle light/dark theme";
pub const CMD_FOCUS_NOTIFICATIONS: &str = "Focus notifications";
pub const CMD_ZOOM_IN: &str = "Zoom in";
pub const CMD_ZOOM_OUT: &str = "Zoom out";
pub const CMD_ZOOM_RESET: &str = "Reset zoom";
pub const CMD_QUIT: &str = "Quit Dockering";
pub const CMD_RESCAN: &str = "Rescan engines";
pub const CMD_MANAGE_ENGINES: &str = "Manage engines…";
pub const CMD_ABOUT: &str = "About Dockering";
pub const CMD_OPEN_LOGS_FOLDER: &str = "Open logs folder";
pub const CMD_MINIMIZE: &str = "Minimize";
pub const CMD_ZOOM_WINDOW: &str = "Zoom window";
pub const CMD_HIDE: &str = "Hide Dockering";
pub const CMD_HIDE_OTHERS: &str = "Hide others";
pub const CMD_PRUNE_CONTAINERS: &str = "Prune stopped containers";
pub const CMD_GROUP_BY: &str = "Group containers by…";
pub const CMD_SORT_BY: &str = "Sort by column…";
pub const CMD_FOCUS_FILTER: &str = "Focus status filter";
pub const CMD_COLLAPSE_ALL: &str = "Collapse all groups";
pub const CMD_EXPAND_ALL: &str = "Expand all groups";
pub const CMD_SELECT_ALL: &str = "Select all rows";
pub const CMD_CLEAR_SELECTION: &str = "Clear selection";
pub const CMD_BULK_START: &str = "Start selected";
pub const CMD_BULK_STOP: &str = "Stop selected";
pub const CMD_BULK_DELETE: &str = "Delete selected";
pub const CMD_QUICK_FIND: &str = "Find in list";
pub const CMD_COPY_ID: &str = "Copy id";
pub const CMD_DELETE: &str = "Delete";
pub const CMD_OPEN_DETAIL: &str = "Open details";
pub const CMD_CONTEXT_MENU: &str = "Row menu";
pub const CMD_START_STOP: &str = "Start/stop container";
pub const CMD_RESTART: &str = "Restart container";
pub const CMD_PAUSE: &str = "Pause/unpause container";
pub const CMD_KILL: &str = "Kill container";
pub const CMD_LOGS: &str = "Container logs";
pub const CMD_TERMINAL: &str = "Container terminal";
pub const CMD_INSPECT: &str = "Inspect container";
pub const CMD_OPEN_PORT: &str = "Open published port";
pub const CMD_PARENT_LIST: &str = "Go to parent list";
pub const CMD_NEXT_TAB: &str = "Next tab";
pub const CMD_PREV_TAB: &str = "Previous tab";
pub const CMD_UNDO: &str = "Undo";
pub const CMD_REDO: &str = "Redo";
pub const CMD_CUT: &str = "Cut";
pub const CMD_COPY: &str = "Copy";
pub const CMD_PASTE: &str = "Paste";
pub const CMD_SELECT_ALL_TEXT: &str = "Select all";

// ── reference groups ────────────────────────────────────────────────────────────────────
pub const REF_GLOBAL: &str = "Global";
pub const REF_LISTS: &str = "Lists";
pub const REF_DETAIL: &str = "Detail";
pub const REF_LOGS: &str = "Logs";
pub const REF_TERMINAL: &str = "Terminal";
pub const REF_DIALOGS: &str = "Dialogs";

// ── menus (SHL-020) ─────────────────────────────────────────────────────────────────────
pub const MENU_EDIT: &str = "Edit";
pub const MENU_VIEW: &str = "View";
pub const MENU_WINDOW: &str = "Window";
pub const MENU_HELP: &str = "Help";

// ── placeholders (phase 2) ──────────────────────────────────────────────────────────────
pub const COMING_SOON: &str = "This page lands in a later milestone.";
