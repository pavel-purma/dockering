//! Command palette (KBD-020) on GPUI Kit `Command`: registry commands available in the
//! current context, plus dynamic "Go to <resource>" jump entries. Shows each entry's
//! binding (the `Command` row resolves `Kbd` from the live keymap, KBD-010).

use gpui_kit::component::IconName;
use gpui_kit::component::command::{Command, CommandGroup, CommandItem, CommandState};
use gpui_kit::{Action, App, Entity, IntoElement, Window};

use crate::actions::Navigate;
use crate::commands::{self, CommandContext};
use crate::nav::{ContainerTab, ImageTab, NetworkTab, Route, VolumeTab};
use crate::state::EngineStore;
use crate::strings as s;

/// Cap on dynamic entries per kind (filtering stays snappy; KBD-020).
pub const MAX_GOTO: usize = 500;

/// Builds the palette contents for the current context.
pub fn build(
    state: &Entity<CommandState>,
    ctx: CommandContext,
    store: Option<&Entity<EngineStore>>,
    cx: &App,
) -> Command {
    let mut groups: Vec<(commands::CommandGroup, Vec<CommandItem>)> = Vec::new();
    for c in commands::available(&ctx) {
        let item = CommandItem::new()
            .label(c.label)
            .keywords(c.keywords.iter().copied())
            .action((c.action)());
        match groups.iter_mut().find(|(g, _)| *g == c.group) {
            Some((_, v)) => v.push(item),
            None => groups.push((c.group, vec![item])),
        }
    }
    groups.sort_by_key(|(g, _)| *g);
    let mut cmd = Command::new(state)
        .placeholder(s::PALETTE_PLACEHOLDER)
        .bordered(false)
        .max_h(gpui_kit::px(420.));
    for (g, items) in groups {
        cmd = cmd.group(CommandGroup::new().label(g.label()).items(items));
    }
    if let Some(store) = store {
        let goto = goto_items(store, cx);
        if !goto.is_empty() {
            cmd = cmd.group(CommandGroup::new().label("Go to").items(goto));
        }
    }
    cmd
}

fn nav(route: Route) -> Box<dyn Action> {
    Box::new(Navigate { route })
}

/// "Go to container/image/volume/network <name>" (KBD-020).
pub fn goto_items(store: &Entity<EngineStore>, cx: &App) -> Vec<CommandItem> {
    let store = store.read(cx);
    let mut out = Vec::new();
    if let Some(cs) = store.containers.data() {
        out.extend(cs.iter().take(MAX_GOTO).map(|c| {
            CommandItem::new()
                .label(s::go_to("container", &c.name))
                .icon(IconName::Inspector)
                .keywords([c.id.chars().take(12).collect::<String>(), c.image.clone()])
                .action(nav(Route::ContainerDetail {
                    id: c.id.clone(),
                    tab: ContainerTab::Overview,
                }))
        }));
    }
    if let Some(is) = store.images.data() {
        out.extend(is.iter().take(MAX_GOTO).map(|i| {
            let name = i
                .repo_tags
                .first()
                .cloned()
                .unwrap_or_else(|| dk_core::format::short_id(&i.id).to_owned());
            CommandItem::new()
                .label(s::go_to("image", &name))
                .action(nav(Route::ImageDetail {
                    id: i.id.clone(),
                    tab: ImageTab::Overview,
                }))
        }));
    }
    if let Some(vs) = store.volumes.data() {
        out.extend(vs.iter().take(MAX_GOTO).map(|v| {
            CommandItem::new()
                .label(s::go_to("volume", &v.name))
                .action(nav(Route::VolumeDetail {
                    name: v.name.clone(),
                    tab: VolumeTab::Overview,
                }))
        }));
    }
    if let Some(ns) = store.networks.data() {
        out.extend(ns.iter().take(MAX_GOTO).map(|n| {
            CommandItem::new()
                .label(s::go_to("network", &n.name))
                .action(nav(Route::NetworkDetail {
                    id: n.id.clone(),
                    tab: NetworkTab::Overview,
                }))
        }));
    }
    out
}

/// The palette body element for the dialog.
pub fn element(
    state: &Entity<CommandState>,
    ctx: CommandContext,
    store: Option<&Entity<EngineStore>>,
    _window: &mut Window,
    cx: &App,
) -> impl IntoElement {
    build(state, ctx, store, cx)
}
