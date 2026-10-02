//! Detail-page breadcrumb (`Containers › myshop › web-1`, spec 30 §2). Built on the GPUI Kit
//! `Breadcrumb`; the parent items dispatch [`Navigate`].

use gpui_kit::component::breadcrumb::{Breadcrumb, BreadcrumbItem};
use gpui_kit::{IntoElement, SharedString};

use crate::actions::Navigate;
use crate::nav::Route;

/// One crumb: a label and the route it links to (`None` = current page, not a link).
pub struct Crumb {
    pub label: SharedString,
    pub route: Option<Route>,
}

impl Crumb {
    pub fn link(label: impl Into<SharedString>, route: Route) -> Self {
        Self {
            label: label.into(),
            route: Some(route),
        }
    }
    pub fn here(label: impl Into<SharedString>) -> Self {
        Self {
            label: label.into(),
            route: None,
        }
    }
}

pub fn breadcrumb(crumbs: Vec<Crumb>) -> impl IntoElement {
    Breadcrumb::new().children(crumbs.into_iter().map(|c| {
        let item = BreadcrumbItem::new(c.label);
        match c.route {
            Some(route) => item.on_click(move |_, window, cx| {
                window.dispatch_action(
                    Box::new(Navigate {
                        route: route.clone(),
                    }),
                    cx,
                )
            }),
            None => item.disabled(true),
        }
    }))
}
