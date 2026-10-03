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
pub const INSECURE_TCP_TOOLTIP: &str = "This engine is reached over plain TCP without TLS. Traffic, including credentials, is not encrypted.";
pub const BACK: &str = "Back";
/// The first sidebar item on Settings routes (SET-080).
pub fn back_to(page: &str) -> String {
    format!("Back to {page}")
}
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
pub const ALL_PORTS: &str = "All ports";
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

// ════════════════════════════════════════════════════════════════════════════════════════
// M6: Images, Volumes, Networks (IMG-*, VOL-*, NET-*). Self-contained section.
// ════════════════════════════════════════════════════════════════════════════════════════

// ── shared resource UI ──────────────────────────────────────────────────────────────────
pub const NONE_TAG: &str = "<none>";
pub const DASH: &str = "—";
pub const IN_USE: &str = "In use";
pub const FILTER_IN_USE: &str = "In use";
pub const FILTER_UNUSED: &str = "Unused";
pub const FILTER_DANGLING: &str = "Dangling";
pub const COL_TAG: &str = "Tag";
pub const COL_IMAGE_ID: &str = "Image ID";
pub const COL_SIZE: &str = "Size";
pub const COL_DRIVER: &str = "Driver";
pub const COL_SCOPE: &str = "Scope";
pub const COL_SUBNETS: &str = "Subnet(s)";
pub const COL_GATEWAY: &str = "Gateway";
pub const COL_CONTAINERS: &str = "Containers";
pub const COL_COMPOSE: &str = "Compose project";
pub const COL_CREATED_BY: &str = "Created by";
pub const COL_COMMENT: &str = "Comment";
pub const COL_DESTINATION: &str = "Destination";
pub const COL_MODE: &str = "Mode";
pub const COL_IPV4: &str = "IPv4";
pub const COL_IPV6: &str = "IPv6";
pub const COL_MAC: &str = "MAC";
pub const READ_WRITE: &str = "RW";
pub const READ_ONLY_SHORT: &str = "RO";
pub const COPY: &str = "Copy";
pub const COPY_NAME: &str = "Copy name";
pub const COPY_DIGEST: &str = "Copy digest";
pub const YES: &str = "Yes";
pub const NO: &str = "No";
pub const DETAIL_LOAD_FAILED: &str = "Couldn't load the details";
pub const TAB_OVERVIEW: &str = "Overview";
pub const TAB_LAYERS: &str = "Layers";
pub const TAB_USED_BY: &str = "Used by";
pub const TAB_INSPECT: &str = "Inspect";
pub const TAB_CONTAINERS: &str = "Containers";
pub const NOT_USED: &str = "No containers use this.";
pub const NO_ATTACHED: &str = "No containers are attached.";
pub const ADD_ROW: &str = "Add row";
pub const REMOVE_ROW: &str = "Remove row";
pub fn in_use_by(n: usize) -> String {
    if n == 1 {
        "In use by 1 container".to_owned()
    } else {
        format!("In use by {n} containers")
    }
}
/// The text after an *In use* pill: `by 2 containers`.
pub fn by_containers(n: usize) -> String {
    if n == 1 {
        "by 1 container".to_owned()
    } else {
        format!("by {n} containers")
    }
}
pub fn resource_gone(kind: &str) -> String {
    format!("This {kind} no longer exists")
}
pub fn deleted_n(kind: &str, n: usize) -> String {
    if n == 1 {
        format!("Deleted 1 {kind}")
    } else {
        format!("Deleted {n} {kind}s")
    }
}
pub fn delete_failed(kind: &str, name: &str) -> String {
    format!("Couldn't delete {kind} {name}")
}
pub fn delete_partial(kind: &str, ok: usize, failed: usize) -> String {
    format!("Deleted {ok} {kind}s, {failed} failed")
}
pub fn pruned_kind(kind: &str, n: usize, size: Option<&str>) -> String {
    match size {
        Some(size) => format!("Pruned {n} {kind}s, reclaimed {size}"),
        None => format!("Pruned {n} {kind}s"),
    }
}
pub fn total_count(kind: &str, n: usize) -> String {
    format!("{n} {kind}{}", if n == 1 { "" } else { "s" })
}
pub fn total_count_size(kind: &str, n: usize, size: &str) -> String {
    format!("{} · {size}", total_count(kind, n))
}

// ── images ──────────────────────────────────────────────────────────────────────────────
pub const IMAGE: &str = "image";
pub const PULL: &str = "Pull";
pub const PULL_IMAGE: &str = "Pull image";
pub const PULL_IMAGE_TITLE: &str = "Pull an image";
pub const PULL_REFERENCE: &str = "Image reference";
pub const PULL_REFERENCE_HINT: &str = "For example nginx:latest or ghcr.io/owner/app:1.0";
pub const PULL_CANCEL: &str = "Cancel pull";
pub const PULL_CANCELLED: &str = "Pull cancelled";
pub const RUN: &str = "Run";
pub const RUN_IMAGE: &str = "Run…";
pub const TAG_IMAGE: &str = "Tag…";
pub const TAG_IMAGE_TITLE: &str = "Tag image";
pub const TAG: &str = "Tag";
pub const REPOSITORY: &str = "Repository";
pub const PRUNE_DANGLING: &str = "Prune dangling images";
pub const PRUNE_UNUSED_IMAGES: &str = "Prune unused images";
pub const NO_IMAGES: &str = "No images yet";
pub const NO_IMAGES_BODY: &str = "Pull an image to get started.";
pub const NO_MATCHING_IMAGES: &str = "No images match the filter";
pub const IMAGE_IN_USE_TITLE: &str = "Image is in use";
pub const IMAGE_IN_USE_BODY: &str = "These containers use the image. Force delete removes it anyway; the containers keep running from the untagged image.";
pub const FORCE: &str = "Force";
pub const NO_DIGEST: &str = "This image has no digest (built locally or never pushed)";
pub const PRUNE_DANGLING_BODY: &str = "Untagged images that no container uses will be removed.";
pub const PRUNE_UNUSED_BODY: &str =
    "Images that no container uses will be removed, including tagged ones.";
pub const ARCHITECTURE: &str = "Architecture";
pub const OS: &str = "OS";
pub const VARIANT: &str = "Variant";
pub const AUTHOR: &str = "Author";
pub const ENTRYPOINT: &str = "Entrypoint";
pub const CMD: &str = "Cmd";
pub const ENV: &str = "Environment";
pub const EXPOSED_PORTS: &str = "Exposed ports";
pub const WORKDIR: &str = "Working dir";
pub const USER: &str = "User";
pub const VOLUMES_LABEL: &str = "Volumes";
pub const LABELS: &str = "Labels";
pub const DIGESTS: &str = "Digests";
pub const TAGS: &str = "Tags";
pub const ID: &str = "ID";
pub const CREATED: &str = "Created";
pub const SIZE: &str = "Size";
pub const NO_HISTORY: &str = "Layer history isn't available for this engine.";
pub fn confirm_delete_images(n: usize) -> String {
    if n == 1 {
        "Delete image?".to_owned()
    } else {
        format!("Delete {n} images?")
    }
}
pub fn confirm_prune_images(n: usize) -> String {
    if n == 1 {
        "Prune 1 image?".to_owned()
    } else {
        format!("Prune {n} images?")
    }
}
pub fn pulling(reference: &str) -> String {
    format!("Pulling {reference}")
}
pub fn pulled(reference: &str) -> String {
    format!("Pulled {reference}")
}
pub fn pull_failed(reference: &str) -> String {
    format!("Couldn't pull {reference}")
}
pub fn run_title(reference: &str) -> String {
    format!("Run {reference}")
}
pub fn tagged(reference: &str) -> String {
    format!("Tagged as {reference}")
}
pub fn layer_status(id: &str, status: &str) -> String {
    format!("{id}: {status}")
}

// ── run dialog (IMG-005) ────────────────────────────────────────────────────────────────
pub const CONTAINER_NAME: &str = "Container name";
pub const OPTIONAL_RANDOM: &str = "Optional; a random name is used when empty";
pub const PORTS: &str = "Ports";
pub const PORT_HOST: &str = "Host port";
pub const PORT_CONTAINER: &str = "Container port[/proto]";
pub const ADD_PORT: &str = "Add port";
pub const ENV_VARS: &str = "Environment variables";
pub const ENV_KEY: &str = "Key";
pub const ENV_VALUE: &str = "Value";
pub const ADD_ENV: &str = "Add variable";
pub const MOUNTS: &str = "Volumes";
pub const MOUNT_SOURCE: &str = "Volume name or host path";
pub const MOUNT_TARGET: &str = "Container path";
pub const ADD_MOUNT: &str = "Add volume";
pub const READ_ONLY: &str = "Read-only";
pub const REMOVE_WHEN_STOPPED: &str = "Remove when stopped";
pub const START: &str = "Start";
pub const ERR_NAME: &str = "Use letters, digits, '_', '.' or '-', starting with a letter or digit.";
pub const ERR_PORT: &str = "Ports are numbers from 1 to 65535.";
pub const ERR_PROTO: &str = "The protocol must be tcp, udp or sctp.";
pub const ERR_CONTAINER_PORT: &str = "The container port is required.";
pub const ERR_ENV_KEY: &str = "Keys can't be empty or contain '=' or spaces.";
pub const ERR_MOUNT_TARGET: &str = "The container path must be absolute (start with /).";
pub const ERR_MOUNT_SOURCE: &str = "Enter a volume name or an absolute host path.";
pub const ERR_IMAGE_REF: &str = "Not a valid image reference, for example nginx:latest.";
pub const ERR_REQUIRED: &str = "Required.";
pub const ERR_DRIVER: &str = "Not a valid driver name.";
pub fn run_failed(reference: &str) -> String {
    format!("Couldn't run {reference}")
}

// ── volumes ─────────────────────────────────────────────────────────────────────────────
pub const VOLUME: &str = "volume";
pub const CREATE_VOLUME: &str = "Create volume";
pub const CREATE_VOLUME_TITLE: &str = "Create a volume";
pub const CREATE: &str = "Create";
pub const DRIVER: &str = "Driver";
pub const DRIVER_OPTIONS: &str = "Driver options";
pub const ADD_OPTION: &str = "Add option";
pub const ADD_LABEL: &str = "Add label";
pub const PRUNE_UNUSED_VOLUMES: &str = "Prune unused volumes";
pub const PRUNE_VOLUMES_BODY: &str =
    "Volumes that no container uses will be removed together with their data.";
pub const NO_VOLUMES: &str = "No volumes yet";
pub const NO_VOLUMES_BODY: &str =
    "Volumes keep container data. Create one, or run a container that uses one.";
pub const NO_MATCHING_VOLUMES: &str = "No volumes match the filter";
pub const VOLUME_IN_USE_TITLE: &str = "Volume is in use";
pub const VOLUME_IN_USE_BODY: &str =
    "Remove these containers first; a volume that a container uses can't be deleted.";
pub const MOUNTPOINT: &str = "Mountpoint";
pub const OPTIONS: &str = "Options";
pub const STATUS: &str = "Status";
pub const DRIVER_STATUS: &str = "Driver status";
pub const NAME: &str = "Name";
pub const CONFIRM_DELETE_VOLUMES_BODY: &str = "The data in the volume is deleted permanently.";
pub fn confirm_delete_volumes(n: usize) -> String {
    if n == 1 {
        "Delete volume?".to_owned()
    } else {
        format!("Delete {n} volumes?")
    }
}
pub fn confirm_prune_volumes(n: usize) -> String {
    if n == 1 {
        "Prune 1 volume?".to_owned()
    } else {
        format!("Prune {n} volumes?")
    }
}
pub fn created_volume(name: &str) -> String {
    format!("Created volume {name}")
}
pub fn create_volume_failed(name: &str) -> String {
    if name.is_empty() {
        "Couldn't create the volume".to_owned()
    } else {
        format!("Couldn't create volume {name}")
    }
}

// ── networks ────────────────────────────────────────────────────────────────────────────
pub const NETWORK: &str = "network";
pub const PRUNE_UNUSED_NETWORKS: &str = "Prune unused networks";
pub const PRUNE_NETWORKS_BODY: &str =
    "Custom networks that no container is attached to will be removed.";
pub const NO_NETWORKS: &str = "No networks";
pub const NO_NETWORKS_BODY: &str = "Networks appear here when the engine or Compose creates them.";
pub const NO_MATCHING_NETWORKS: &str = "No networks match the search";
pub const BUILTIN_NETWORK: &str = "Built-in networks can't be deleted";
pub const NETWORK_MGMT_UNSUPPORTED: &str = "This engine doesn't support managing networks";
pub const INTERNAL: &str = "Internal";
pub const ATTACHABLE: &str = "Attachable";
pub const IPV6: &str = "IPv6";
pub const IPAM: &str = "IPAM config";
pub const SUBNET: &str = "Subnet";
pub const GATEWAY: &str = "Gateway";
pub const IP_RANGE: &str = "IP range";
pub fn confirm_delete_networks(n: usize) -> String {
    if n == 1 {
        "Delete network?".to_owned()
    } else {
        format!("Delete {n} networks?")
    }
}
pub fn confirm_prune_networks(n: usize) -> String {
    if n == 1 {
        "Prune 1 network?".to_owned()
    } else {
        format!("Prune {n} networks?")
    }
}

// ── palette (M6) ────────────────────────────────────────────────────────────────────────
pub const CMD_PULL_IMAGE: &str = "Pull image…";
pub const CMD_RUN_IMAGE: &str = "Run image…";
pub const CMD_TAG_IMAGE: &str = "Tag image…";
pub const CMD_COPY_DIGEST: &str = "Copy image digest";
pub const CMD_PRUNE_DANGLING: &str = "Prune dangling images";
pub const CMD_PRUNE_UNUSED_IMAGES: &str = "Prune unused images";
pub const CMD_NEW_VOLUME: &str = "New volume…";
pub const CMD_PRUNE_VOLUMES: &str = "Prune unused volumes";
pub const CMD_PRUNE_NETWORKS: &str = "Prune unused networks";
pub const CMD_DELETE_RESOURCE: &str = "Delete";
pub const CMD_COPY_RESOURCE_ID: &str = "Copy id";

// ── settings (SET-*, ENG-104/105/110) ───────────────────────────────────────────────────
pub const SETTINGS_SECTIONS: &str = "Settings sections";
pub const SET_DESC_GENERAL: &str = "Appearance and app behaviour.";
pub const SET_DESC_ENGINES: &str =
    "Discovered and manually added container engines. Changes apply immediately.";
pub const SET_DESC_CONTAINERS: &str = "Defaults for the Containers page.";
pub const SET_DESC_LOGS: &str = "Defaults for container logs.";
pub const SET_DESC_TERMINAL: &str = "Container terminal appearance and behaviour.";
pub const SET_DESC_STATS: &str = "Container resource statistics.";
pub const SET_DESC_DIAGNOSTICS: &str = "Logs, diagnostics, version, and licences.";
pub const SET_DESC_KEYBOARD: &str = "Every keyboard shortcut on this system.";
pub const SET_THEME: &str = "Theme";
pub const SET_THEME_DESC: &str = "System follows the operating system's light or dark setting.";
pub const THEME_SYSTEM: &str = "System";
pub const THEME_LIGHT: &str = "Light";
pub const THEME_DARK: &str = "Dark";
pub const SET_START_PAGE: &str = "Start page";
pub const SET_START_PAGE_DESC: &str = "The page Dockering opens on.";
pub const SET_CONFIRM_STOPPED: &str = "Confirm before deleting stopped containers";
pub const SET_CONFIRM_STOPPED_DESC: &str = "Deleting a running container always asks first.";
pub const SET_SHOW_NETWORKS: &str = "Show Networks page";
pub const SET_SHOW_NETWORKS_DESC: &str = "Adds Networks to the sidebar.";
pub const SET_GROUP_BY: &str = "Group containers by";
pub const SET_GROUP_BY_DESC: &str = "The default grouping. Mod+Shift+G changes it on the page.";
pub const SET_GROUP_LABEL_KEY: &str = "Group label key";
pub const SET_GROUP_LABEL_KEY_DESC: &str = "Used when grouping by label, for example app.";
pub const SET_LABEL_KEY_REQUIRED: &str = "Enter a label key to group by label.";
pub const SET_SHOW_STATS: &str = "Show CPU and memory columns";
pub const SET_SHOW_STATS_DESC: &str = "Streams statistics for visible running containers. Engines that can't stream them cheaply hide the columns.";
pub const SET_POLLING: &str = "Polling interval";
pub const SET_POLLING_DESC: &str = "How often lists refresh when the engine doesn't send events.";
pub const SET_LOGS_TAIL: &str = "Initial lines";
pub const SET_LOGS_TAIL_DESC: &str = "Recent lines loaded when the log view opens.";
pub const SET_LOGS_MAX: &str = "Maximum lines kept";
pub const SET_LOGS_MAX_DESC: &str = "Older lines are dropped from the view.";
pub const SET_LOGS_TIMESTAMPS: &str = "Show timestamps";
pub const SET_LOGS_TIMESTAMPS_DESC: &str = "Default for new log views.";
pub const SET_LOGS_WRAP: &str = "Wrap long lines";
pub const SET_LOGS_WRAP_DESC: &str = "Default for new log views.";
pub const SET_TERM_FONT: &str = "Font family";
pub const SET_TERM_FONT_DESC: &str = "Leave empty for the platform monospace font.";
pub const SET_TERM_FONT_PLACEHOLDER: &str = "Platform monospace";
pub const SET_TERM_FONT_SIZE: &str = "Font size";
pub const SET_TERM_SHELL: &str = "Default shell";
pub const SET_TERM_SHELL_DESC: &str = "Leave empty to detect bash, then sh.";
pub const SET_TERM_SHELL_PLACEHOLDER: &str = "Auto-detect";
pub const SET_TERM_SCROLLBACK: &str = "Scrollback lines";
pub const SET_TERM_EXTERNAL: &str = "External terminal command";
pub const SET_TERM_EXTERNAL_DESC: &str =
    "Used by Open in external terminal. {cmd} is replaced with the exec command.";
pub const SET_TERM_EXTERNAL_PLACEHOLDER: &str = "e.g. wt.exe {cmd}";
pub const SET_EXTERNAL_NEEDS_CMD: &str = "The command must contain {cmd}.";
pub const SET_STATS_HISTORY: &str = "History window";
pub const SET_STATS_HISTORY_DESC: &str = "How much history the stats charts keep.";
pub const SET_STATS_ALL_CORES: &str = "CPU % relative to all cores";
pub const SET_STATS_ALL_CORES_DESC: &str = "Off: 100 % is one core. On: 100 % is every core.";
pub const NOT_A_NUMBER: &str = "Enter a number.";
pub const NOT_A_WHOLE_NUMBER: &str = "Enter a whole number.";
pub const SET_GROUP_APPEARANCE: &str = "Appearance";
pub const SET_GROUP_BEHAVIOUR: &str = "Behaviour";
pub const SET_GROUP_LIST: &str = "List";
pub const SET_GROUP_UPDATES: &str = "Updates";
pub const SET_GROUP_VIEW: &str = "Log view";
pub const SET_GROUP_FONT: &str = "Font";
pub const SET_GROUP_SESSION: &str = "Sessions";
pub const SET_GROUP_HISTORY: &str = "History";
pub const UNIT_SECONDS: &str = "s";
pub const UNIT_MINUTES: &str = "min";
pub const UNIT_PX: &str = "px";
pub fn out_of_range(min: f64, max: f64, unit: &str) -> String {
    let unit = if unit.is_empty() {
        String::new()
    } else {
        format!(" {unit}")
    };
    format!("Must be between {min} and {max}{unit}.")
}

// Engines section (ENG-104, ENG-109, ENG-110, SET-010)
pub const ENGINES_DISCOVERY: &str = "Discovery";
pub const SET_SHOW_ALL_DISTROS: &str = "Show all WSL distros";
pub const SET_SHOW_ALL_DISTROS_DESC: &str = "Also list distros without Docker.";
pub const SET_SHOW_ALL_SESSIONS: &str = "Show all WSLC sessions";
pub const SET_SHOW_ALL_SESSIONS_DESC: &str =
    "Also list WSL container sessions other than the default one.";
pub const ENGINES_LIST: &str = "Engines";
pub const NO_ENGINES: &str = "No engines yet. Rescan, or add one manually.";
pub const ENGINE_NAME: &str = "Engine name";
pub const ENABLED: &str = "Enabled";
pub const TEST_CONNECTION: &str = "Test connection";
pub const TESTING: &str = "Testing…";
pub const REMOVE: &str = "Remove";
pub const HIDE: &str = "Hide";
pub const UNHIDE: &str = "Unhide";
pub const UNMERGE: &str = "Un-merge";
pub const TAG_HIDDEN: &str = "Hidden";
pub const TAG_ACTIVE: &str = "Active";
pub const ORIGIN_MANUAL: &str = "Manual";
pub const ORIGIN_DISCOVERED: &str = "Discovered";
pub const RENAME_ENGINE: &str = "Couldn't rename the engine";
pub const UPDATE_ENGINE: &str = "Couldn't update the engine";
pub const REMOVE_ENGINE: &str = "Couldn't remove the engine";
pub const TEST_FAILED: &str = "Connection test failed";
pub const REMOVE_ENGINE_BODY: &str =
    "The engine is removed from Dockering. Nothing changes on the engine itself.";
pub fn remove_engine_title(name: &str) -> String {
    format!("Remove {name}?")
}
pub fn engine_removed(name: &str) -> String {
    format!("Removed {name}")
}
pub fn test_ok(name: &str, version: &str, api: Option<&str>, os: &str, arch: &str) -> String {
    match api {
        Some(api) => format!("Connected to {name}: {version} · API {api} · {os}/{arch}"),
        None => format!("Connected to {name}: {version} · {os}/{arch}"),
    }
}
pub fn engine_version(version: &str, api: Option<&str>, os: &str, arch: &str) -> String {
    match api {
        Some(api) => format!("Version {version} · API {api} · {os}/{arch}"),
        None => format!("Version {version} · {os}/{arch}"),
    }
}
pub fn wsl_version(v: &str) -> String {
    format!("WSL {v}")
}

// Add engine dialog (ENG-105)
pub const ADD_ENGINE_TITLE: &str = "Add engine";
pub const ENGINE_KIND: &str = "Connection type";
pub const ENGINE_NAME_PLACEHOLDER: &str = "Optional; defaults to the endpoint";
pub const BROWSE: &str = "Browse…";
pub const KNOWN_VALUES: &str = "Detected:";
pub const TEST: &str = "Test";
pub const SAVE: &str = "Save";
pub const SAVE_ANYWAY: &str = "Save anyway";
pub const SAVE_NEEDS_TEST: &str = "Test the connection before saving, or choose Save anyway.";
pub const FIELD_REQUIRED: &str = "Required.";
pub const INVALID_PORT: &str = "Enter a port between 1 and 65535.";
pub const INVALID_HOST: &str = "Enter a host name or IP address without a scheme or path.";
pub const KIND_NOT_ADDABLE: &str = "This connection type can't be added manually.";
pub const NO_ENGINE_TYPES: &str = "No engine backends can be added on this system.";
pub const ADD_ENGINE_FAILED: &str = "Couldn't add the engine";
pub fn engine_added(name: &str) -> String {
    format!("Added {name}")
}

// Diagnostics (SET-060, REL-002)
pub const DIAG_LOGGING: &str = "Logging";
pub const DIAG_LOG_LEVEL: &str = "Log level";
pub const DIAG_LOG_LEVEL_DESC: &str =
    "Applies on restart. The RUST_LOG environment variable overrides it.";
pub const LOG_INFO: &str = "Info";
pub const LOG_DEBUG: &str = "Debug";
pub const DIAG_LOGS_FOLDER: &str = "Logs folder";
pub const DIAG_SUPPORT: &str = "Support";
pub const COPY_DIAGNOSTICS: &str = "Copy diagnostics";
pub const COPY_DIAGNOSTICS_DESC: &str =
    "Version, OS, engines, and capabilities. Never includes secrets or environment values.";
pub const DIAGNOSTICS_COPIED: &str = "Diagnostics copied to the clipboard";
pub const DIAGNOSTICS_FAILED: &str = "Couldn't collect diagnostics";
pub const DIAG_ENGINES: &str = "Engine transports";
pub const DIAG_NOT_CONNECTED: &str = "Not connected yet";
pub const DIAG_ABOUT: &str = "About";
pub const DIAG_VERSION: &str = "Version";
pub const VIEW_LICENSES: &str = "View licences";
pub const VIEW_LICENSES_DESC: &str =
    "Third-party notices for the libraries and icons Dockering uses.";
pub const LICENSES_FAILED: &str = "Couldn't open the licences";

// Keyboard (SET-070)
pub const KEYBOARD_NOTE: &str =
    "Shortcuts are read-only in this version. Custom key bindings are planned for a later release.";

// Commands
pub const CMD_ADD_ENGINE: &str = "Add engine…";
pub const CMD_COPY_DIAGNOSTICS: &str = "Copy diagnostics";
pub const CMD_VIEW_LICENSES: &str = "View third-party licences";

// ── container detail (CDT-*, LOG-*, TRM-*, STA-*; wip/detail) ─────────────────────────────
pub const CONTAINER_DETAIL_LOAD_FAILED: &str = "Couldn't load the container details";
pub const DETAIL_READ_ONLY: &str = "Read-only: the container was removed.";
pub const COPY_VALUE: &str = "Copy value";
pub const REVEAL_VALUE: &str = "Reveal value";
pub const HIDE_VALUE: &str = "Hide value";
pub const MASKED_VALUE: &str = "••••••••";
pub const NONE_VALUE: &str = "—";
pub const UNLIMITED: &str = "Unlimited";
pub const OPEN_IMAGE: &str = "Open image";
// Overview (CDT-010)
pub const SEC_GENERAL: &str = "General";
pub const SEC_COMMAND: &str = "Command";
pub const SEC_COMPOSE: &str = "Compose";
pub const SEC_ENVIRONMENT: &str = "Environment";
pub const SEC_LABELS: &str = "Labels";
pub const SEC_RESOURCES: &str = "Resources";
pub const SEC_HEALTH: &str = "Health";
pub const F_ID: &str = "ID";
pub const F_NAME: &str = "Name";
pub const F_IMAGE: &str = "Image";
pub const F_IMAGE_ID: &str = "Image ID";
pub const F_CREATED: &str = "Created";
pub const F_STARTED: &str = "Started";
pub const F_FINISHED: &str = "Finished";
pub const F_RESTART_COUNT: &str = "Restart count";
pub const F_RESTART_POLICY: &str = "Restart policy";
pub const F_PLATFORM: &str = "Platform";
pub const F_PID: &str = "PID";
pub const F_EXIT_CODE: &str = "Exit code";
pub const F_ENTRYPOINT: &str = "Entrypoint";
pub const F_CMD: &str = "Command";
pub const F_WORKDIR: &str = "Working dir";
pub const F_USER: &str = "User";
pub const F_TTY: &str = "TTY";
pub const F_PROJECT: &str = "Project";
pub const F_SERVICE: &str = "Service";
pub const F_NUMBER: &str = "Number";
pub const F_CONFIG_FILES: &str = "Config files";
pub const F_CPU_LIMIT: &str = "CPU limit";
pub const F_MEMORY_LIMIT: &str = "Memory limit";
pub const F_PIDS_LIMIT: &str = "PIDs limit";
pub const NO_ENV: &str = "No environment variables";
pub const NO_LABELS: &str = "No labels";
pub const F_KEY: &str = "Key";
pub const F_VALUE: &str = "Value";
pub const NO_HEALTH: &str = "No health check results yet";
pub fn cpus(n: f64) -> String {
    if (n - 1.0).abs() < f64::EPSILON {
        "1 CPU".to_owned()
    } else {
        format!("{n} CPUs")
    }
}
pub fn health_result(code: i64, when: &str) -> String {
    format!("exit {code} · {when}")
}
// Mounts (CDT-020)
pub const COL_TYPE: &str = "Type";
pub const COL_SOURCE: &str = "Source";
pub const NO_MOUNTS: &str = "This container has no mounts";
// Network (CDT-030)
pub const SEC_PORTS: &str = "Port bindings";
pub const SEC_NETWORKS: &str = "Networks";
pub const SEC_NET_SETTINGS: &str = "Settings";
pub const COL_CONTAINER_PORT: &str = "Container port";
pub const COL_HOST_IP: &str = "Host IP";
pub const COL_HOST_PORT: &str = "Host port";
pub const COL_NETWORK: &str = "Network";
pub const COL_ALIASES: &str = "Aliases";
pub const F_HOSTNAME: &str = "Hostname";
pub const F_DNS: &str = "DNS";
pub const F_NETWORK_MODE: &str = "Network mode";
pub const NO_PORTS: &str = "No published ports";
pub const NO_CONTAINER_NETWORKS: &str = "Not attached to any network";
// Inspect (CDT-040)
pub const INSPECT_UNMASKED: &str =
    "Environment values are not masked here. Be careful when sharing this output.";
pub const COPY_JSON: &str = "Copy JSON";
pub const SEARCH_JSON: &str = "Search";
pub const FORMATTING: &str = "Formatting…";
// Logs (LOG-*)
pub const LOGS_SEARCH: &str = "Search logs…";
pub const LOGS_TIMESTAMPS: &str = "Timestamps";
pub const LOGS_WRAP: &str = "Wrap lines";
pub const LOGS_CLEAR: &str = "Clear view";
pub const LOGS_COPY_ALL: &str = "Copy all";
pub const LOGS_SAVE: &str = "Save to file…";
pub const LOGS_PREV: &str = "Previous match";
pub const LOGS_NEXT: &str = "Next match";
pub const LOGS_FOLLOW: &str = "Jump to bottom and follow";
pub const LOGS_EMPTY: &str = "No log output yet";
pub const LOGS_FAILED: &str = "The log stream failed";
pub const LOGS_SAVED: &str = "Logs saved";
pub const LOGS_SAVE_FAILED: &str = "Couldn't save the logs";
pub fn logs_dropped(n: u64) -> String {
    format!("{n} earlier lines dropped (buffer limit)")
}
pub fn logs_jump(n: usize) -> String {
    format!("Jump to bottom ({n} new)")
}
pub fn logs_matches(current: usize, total: usize) -> String {
    if total == 0 {
        "No matches".to_owned()
    } else {
        format!("{current} of {total}")
    }
}
pub fn container_exited(code: Option<i64>) -> String {
    match code {
        Some(code) => format!("Container exited (code {code})"),
        None => "Container exited".to_owned(),
    }
}
pub fn logs_file_name(name: &str) -> String {
    format!("{name}.log")
}
// Terminal (TRM-*)
pub const TERMINAL_START_HINT: &str = "Start the container to open a terminal";
pub const TERMINAL_UNSUPPORTED: &str = "This engine doesn't support interactive terminals";
pub const TERMINAL_NEW: &str = "New terminal";
pub const TERMINAL_CLOSE: &str = "Close session";
pub const TERMINAL_RECONNECT: &str = "Reconnect";
pub const TERMINAL_SHELL: &str = "Shell";
pub const TERMINAL_USER: &str = "User (optional)";
pub const TERMINAL_CUSTOM_CMD: &str = "Command, e.g. /bin/sh -l";
pub const TERMINAL_CONNECTING: &str = "Connecting…";
pub const TERMINAL_EXTERNAL: &str = "Open in external terminal";
pub const TERMINAL_EXTERNAL_FAILED: &str = "Couldn't open the external terminal";
pub const TERMINAL_EXTERNAL_UNAVAILABLE: &str = "External terminal isn't available for this engine. Set a terminal command in Settings, or use the built-in terminal.";
pub const TERMINAL_OPEN_FAILED: &str = "Couldn't open a terminal";
pub const SHELL_AUTO: &str = "Auto";
pub const SHELL_CUSTOM: &str = "Custom";
pub fn terminal_leave_hint(keys: &str) -> String {
    format!("The terminal captures all keys. Press {keys} to leave it.")
}
pub fn terminal_session(n: usize, shell: &str) -> String {
    format!("{n}: {shell}")
}
// Stats (STA-*)
pub const STATS_CPU: &str = "CPU";
pub const STATS_MEMORY: &str = "Memory";
pub const STATS_NETWORK: &str = "Network I/O";
pub const STATS_DISK: &str = "Disk I/O";
pub const STATS_WINDOW: &str = "Window";
pub const STATS_PROCESSES: &str = "Processes";
pub const STATS_WAITING: &str = "Waiting for samples…";
pub const STATS_DISK_USAGE: &str = "Disk usage";
pub const STATS_LOAD_DISK: &str = "Load disk usage";
pub const STATS_RX: &str = "Received";
pub const STATS_TX: &str = "Sent";
pub const STATS_READ: &str = "Read";
pub const STATS_WRITE: &str = "Write";
pub const STATS_FAILED: &str = "Couldn't read stats";
pub const NOT_RUNNING: &str = "Container not running";
pub fn stats_mem(used: &str, limit: Option<&str>) -> String {
    match limit {
        Some(l) => format!("{used} / {l}"),
        None => used.to_owned(),
    }
}
pub fn stats_pids(n: u64) -> String {
    format!("PIDs: {n}")
}
pub fn stats_totals(rx: &str, tx: &str, r: &str, w: &str) -> String {
    format!("Totals: ↓ {rx} ↑ {tx} · R {r} W {w}")
}
pub fn stats_gap(n: u64) -> String {
    format!("{n} samples skipped")
}
pub fn stats_disk(rw: &str, root: &str) -> String {
    format!("Writable layer: {rw} · Root FS: {root}")
}
// Command labels (palette)
pub const CMD_DETAIL_TAB_OVERVIEW: &str = "Go to tab: Overview";
pub const CMD_DETAIL_TAB_LOGS: &str = "Go to tab: Logs";
pub const CMD_DETAIL_TAB_TERMINAL: &str = "Go to tab: Terminal";
pub const CMD_DETAIL_TAB_STATS: &str = "Go to tab: Stats";
pub const CMD_DETAIL_TAB_MOUNTS: &str = "Go to tab: Mounts";
pub const CMD_DETAIL_TAB_NETWORK: &str = "Go to tab: Network";
pub const CMD_DETAIL_TAB_INSPECT: &str = "Go to tab: Inspect";
pub const CMD_TERM_RECONNECT: &str = "Reconnect terminal session";
pub const CMD_TERM_EXTERNAL: &str = "Open in external terminal";
pub const CMD_STATS_DISK: &str = "Load container disk usage";
pub const CMD_STATS_1M: &str = "Stats window: 1 minute";
pub const CMD_STATS_5M: &str = "Stats window: 5 minutes";
pub const CMD_STATS_15M: &str = "Stats window: 15 minutes";
pub const CMD_LOGS_FIND_NEXT: &str = "Logs: next match";
pub const CMD_LOGS_FIND_PREV: &str = "Logs: previous match";
pub const CMD_LOGS_TIMESTAMPS: &str = "Logs: toggle timestamps";
pub const CMD_LOGS_WRAP: &str = "Logs: toggle wrap";
pub const CMD_LOGS_CLEAR: &str = "Logs: clear view";
pub const CMD_LOGS_SAVE: &str = "Logs: save to file…";
pub const CMD_LOGS_COPY: &str = "Logs: copy all";
pub const CMD_TERM_NEW: &str = "New terminal session";
pub const CMD_TERM_CLOSE: &str = "Close terminal session";

// Updates (UPD-008, UPD-009, SET-090, KBD-076)
pub const CMD_CHECK_FOR_UPDATES: &str = "Check for updates";
pub const CMD_CHECK_FOR_UPDATES_MENU: &str = "Check for Updates…";
pub const CMD_RESTART_TO_UPDATE: &str = "Restart to update";
pub const CMD_VIEW_RELEASE_NOTES: &str = "View release notes";
pub const SET_DESC_UPDATES: &str =
    "Dockering checks GitHub Releases once a day. The request is anonymous and carries no ids.";
pub const UPD_GROUP: &str = "Updates";
pub const UPD_AUTO_CHECK: &str = "Check for updates automatically";
pub const UPD_AUTO_CHECK_DESC: &str =
    "Downloads new versions in the background. Nothing is installed until you restart to update.";
pub const UPD_LAST_CHECKED: &str = "Last checked";
pub const UPD_NEVER_CHECKED: &str = "Never";
pub const UPD_CHECK_NOW: &str = "Check now";
pub const UPD_CHECK_NOW_DESC: &str = "Check GitHub Releases for a newer version.";
pub const UPD_UP_TO_DATE: &str = "You're up to date.";
pub const UPD_CHECK_FAILED: &str = "Update check failed:";
pub const UPD_CHECKING: &str = "Checking for updates…";
pub const UPD_POLICY: &str = "Updates are turned off by your administrator.";
pub const UPD_UNAVAILABLE: &str = "Updates aren't available in this build.";
pub const UPD_NOTHING_READY: &str = "No update is ready to install.";
pub const UPD_APPLY_FAILED: &str = "Couldn't start the update";
pub const UPD_WHATS_NEW: &str = "What's new";
pub const UPD_RESTART_NOW: &str = "Restart now";
pub const RELEASES_URL: &str = "https://github.com/pavel-purma/dockering/releases";
pub fn upd_available(version: &str) -> String {
    format!("Dockering {version} is available.")
}
pub fn upd_available_short(version: &str) -> String {
    format!("Dockering {version} available")
}
pub fn upd_downloading(percent: u64) -> String {
    format!("Downloading update… {percent}%")
}
pub fn upd_restart(version: &str, needs_elevation: bool) -> String {
    if needs_elevation {
        format!("Restart to update ({version}, admin)")
    } else {
        format!("Restart to update ({version})")
    }
}
pub fn upd_ready(version: &str) -> String {
    format!("Dockering {version} is ready to install.")
}
pub fn upd_updated(version: &str) -> String {
    format!("Updated to Dockering {version}.")
}
pub fn release_notes_url(version: &str) -> String {
    format!("{RELEASES_URL}/tag/v{version}")
}
