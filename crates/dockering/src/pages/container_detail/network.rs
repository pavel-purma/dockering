//! Network tab (CDT-030): port bindings (host port links open the browser), networks
//! (name links → network detail; IPv4/IPv6/gateway/MAC/aliases), and hostname / DNS /
//! network mode. One focusable rows panel (KBD-043/044).

use dk_core::ContainerDetails;
use dk_core::format::format_port;
use gpui_kit::{
    App, Context, Entity, FocusHandle, Focusable, IntoElement, Render, Subscription, Window,
};

use super::rows::{self, Cell, Row, RowsState, Section};
use super::state::ContainerDetailState;
use crate::actions::{Navigate, OpenUrl};
use crate::strings as s;
use crate::ui::links::network_route;
use crate::ui::widgets::port_url;

pub struct NetworkTab {
    state: Entity<ContainerDetailState>,
    rows: RowsState,
    _sub: Subscription,
}

impl NetworkTab {
    pub fn new(state: Entity<ContainerDetailState>, cx: &mut Context<Self>) -> Self {
        let sub = cx.observe(&state, |_, _, cx| cx.notify());
        Self {
            state,
            rows: RowsState::new(cx),
            _sub: sub,
        }
    }

    pub fn rows(&self) -> &RowsState {
        &self.rows
    }

    pub fn rows_mut(&mut self) -> &mut RowsState {
        &mut self.rows
    }
}

fn opt(v: Option<&String>) -> String {
    v.filter(|s| !s.is_empty())
        .cloned()
        .unwrap_or_else(|| s::NONE_VALUE.to_owned())
}

pub fn sections(d: &ContainerDetails) -> Vec<Section> {
    // Bindings come from inspect; fall back to the summary's ports.
    let ports = if d.port_bindings.is_empty() {
        &d.summary.ports
    } else {
        &d.port_bindings
    };
    let port_rows: Vec<Row> = ports
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let host_ip =
                p.ip.map(|ip| ip.to_string())
                    .unwrap_or_else(|| s::NONE_VALUE.to_owned());
            let url = port_url(p);
            let host_port = match (p.public, &url) {
                (Some(port), Some(_)) => Cell::Link(port.to_string().into()),
                (Some(port), None) => Cell::mono(port.to_string()),
                (None, _) => Cell::text(s::NONE_VALUE),
            };
            let row = Row::new(
                format!("port:{i}"),
                vec![
                    Cell::mono(format!("{}/{}", p.private, p.proto)),
                    Cell::mono(host_ip),
                    host_port,
                ],
                url.clone().unwrap_or_else(|| format_port(p)),
            );
            match url {
                Some(url) => row.action(Box::new(OpenUrl { url: url.into() })),
                None => row,
            }
        })
        .collect();
    let net_rows: Vec<Row> = d
        .network_settings
        .networks
        .iter()
        .map(|n| {
            let aliases = if n.aliases.is_empty() {
                s::NONE_VALUE.to_owned()
            } else {
                n.aliases.join(", ")
            };
            Row::new(
                format!("net:{}", n.network),
                vec![
                    Cell::Link(n.network.clone().into()),
                    Cell::mono(opt(n.ipv4.as_ref())),
                    Cell::mono(opt(n.ipv6.as_ref())),
                    Cell::mono(opt(n.gateway.as_ref())),
                    Cell::mono(opt(n.mac.as_ref())),
                    Cell::text(aliases),
                ],
                n.ipv4.clone().unwrap_or_else(|| n.network.clone()),
            )
            .action(Box::new(Navigate {
                // The network detail page resolves an id or a name.
                route: network_route(n.network_id.as_deref().unwrap_or(&n.network)),
            }))
        })
        .collect();
    let dns = if d.network_settings.dns.is_empty() {
        s::NONE_VALUE.to_owned()
    } else {
        d.network_settings.dns.join(", ")
    };
    let hostname = opt(d.hostname.as_ref());
    let mode = opt(d.network_settings.network_mode.as_ref());
    vec![
        Section::table(
            "net-ports",
            s::SEC_PORTS,
            &[s::COL_CONTAINER_PORT, s::COL_HOST_IP, s::COL_HOST_PORT],
            port_rows,
            s::NO_PORTS,
        ),
        Section::table(
            "net-networks",
            s::SEC_NETWORKS,
            &[
                s::COL_NETWORK,
                s::COL_IPV4,
                s::COL_IPV6,
                s::COL_GATEWAY,
                s::COL_MAC,
                s::COL_ALIASES,
            ],
            net_rows,
            s::NO_CONTAINER_NETWORKS,
        ),
        Section::list(
            "net-settings",
            s::SEC_NET_SETTINGS,
            vec![
                Row::kv(
                    "hostname",
                    s::F_HOSTNAME,
                    Cell::mono(hostname.clone()),
                    hostname,
                ),
                Row::kv("dns", s::F_DNS, Cell::mono(dns.clone()), dns),
                Row::kv("mode", s::F_NETWORK_MODE, Cell::text(mode.clone()), mode),
            ],
        ),
    ]
}

impl Focusable for NetworkTab {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.rows.focus.clone()
    }
}

impl Render for NetworkTab {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let sections = self
            .state
            .read(cx)
            .details
            .data()
            .map(sections)
            .unwrap_or_default();
        rows::render(
            "network-rows",
            &mut self.rows,
            sections,
            window,
            cx,
            |v: &mut Self| &mut v.rows,
        )
    }
}
