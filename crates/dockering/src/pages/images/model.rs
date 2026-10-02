//! Pure logic of the Images page (IMG-001…003): one row per tag, in-use counts, filters,
//! search (SHL-006), sorting, header totals. Unit-tested; no GPUI.

use std::collections::{HashMap, HashSet};

use dk_core::format::{short_id, split_repo_tag};
use dk_core::{ContainerSummary, ImageSummary};
use gpui_kit::SharedString;
use time::OffsetDateTime;

use crate::strings as s;
use crate::ui::list_table::{ListNode, SortState};

/// Filter segment (IMG-002).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ImageFilter {
    #[default]
    All,
    InUse,
    Unused,
    Dangling,
}

impl ImageFilter {
    pub const OPTIONS: &'static [(&'static str, &'static str)] = &[
        ("all", s::FILTER_ALL),
        ("in-use", s::FILTER_IN_USE),
        ("unused", s::FILTER_UNUSED),
        ("dangling", s::FILTER_DANGLING),
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            ImageFilter::All => "all",
            ImageFilter::InUse => "in-use",
            ImageFilter::Unused => "unused",
            ImageFilter::Dangling => "dangling",
        }
    }

    pub fn parse(s: &str) -> Self {
        match s {
            "in-use" => ImageFilter::InUse,
            "unused" => ImageFilter::Unused,
            "dangling" => ImageFilter::Dangling,
            _ => ImageFilter::All,
        }
    }

    pub fn matches(self, r: &ImageRow) -> bool {
        match self {
            ImageFilter::All => true,
            ImageFilter::InUse => r.in_use > 0,
            ImageFilter::Unused => r.in_use == 0,
            ImageFilter::Dangling => r.dangling,
        }
    }
}

/// One row: an image tag, or a dangling image (IMG-001).
#[derive(Debug, Clone, PartialEq)]
pub struct ImageRow {
    /// Row key: `<id>|<repo:tag>` (or the id for a dangling image).
    pub key: SharedString,
    pub image_id: String,
    pub repo: String,
    pub tag: String,
    pub dangling: bool,
    pub created: OffsetDateTime,
    pub size: u64,
    /// Containers using the image.
    pub in_use: usize,
    /// Names of the containers using it (IMG-006 messages).
    pub used_by: Vec<String>,
    pub digests: Vec<String>,
}

impl ImageRow {
    /// What `run`, `delete` and the detail page use: `repo:tag`, or the id when dangling.
    pub fn reference(&self) -> String {
        if self.dangling || self.repo == s::NONE_TAG {
            self.image_id.clone()
        } else if self.tag == s::NONE_TAG {
            self.repo.clone()
        } else {
            format!("{}:{}", self.repo, self.tag)
        }
    }

    /// Display label (`repo:tag`, `<none>:<none>` for dangling, or the short id).
    pub fn label(&self) -> String {
        if self.dangling {
            format!("{} ({})", s::NONE_TAG, short_id(&self.image_id))
        } else {
            format!("{}:{}", self.repo, self.tag)
        }
    }
}

/// Containers using each image id (`ContainerSummary.image_id`).
pub fn users_by_image(containers: &[ContainerSummary]) -> HashMap<&str, Vec<&ContainerSummary>> {
    let mut m: HashMap<&str, Vec<&ContainerSummary>> = HashMap::new();
    for c in containers {
        m.entry(c.image_id.as_str()).or_default().push(c);
    }
    m
}

/// One row per tag; a dangling image yields one `<none>` row (IMG-001). In-use comes from
/// the container list (falls back to `ImageSummary.containers`).
pub fn rows(images: &[ImageSummary], containers: &[ContainerSummary]) -> Vec<ImageRow> {
    let users = users_by_image(containers);
    let mut out = Vec::with_capacity(images.len());
    for img in images {
        let used: Vec<String> = users
            .get(img.id.as_str())
            .map(|v| v.iter().map(|c| c.name.clone()).collect())
            .unwrap_or_default();
        let in_use = used.len().max(img.containers.unwrap_or(0) as usize);
        let base = |key: String, repo: String, tag: String, dangling: bool| ImageRow {
            key: key.into(),
            image_id: img.id.clone(),
            repo,
            tag,
            dangling,
            created: img.created,
            size: img.size,
            in_use,
            used_by: used.clone(),
            digests: img.repo_digests.clone(),
        };
        if img.repo_tags.is_empty() {
            // Untagged: dangling, or pulled by digest only (`repo@sha256:…`).
            let (repo, dangling) = match img.repo_digests.first() {
                Some(d) if !img.dangling => (d.split('@').next().unwrap_or(d).to_owned(), false),
                _ => (s::NONE_TAG.to_owned(), true),
            };
            out.push(base(img.id.clone(), repo, s::NONE_TAG.to_owned(), dangling));
        } else {
            for t in &img.repo_tags {
                let (repo, tag) = split_repo_tag(t);
                out.push(base(format!("{}|{t}", img.id), repo, tag, false));
            }
        }
    }
    out
}

/// Case-insensitive substring over `repo:tag`, plus an id prefix (SHL-006, IMG-002).
pub fn matches_search(r: &ImageRow, query: &str) -> bool {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return true;
    }
    let id = r.image_id.strip_prefix("sha256:").unwrap_or(&r.image_id);
    format!("{}:{}", r.repo, r.tag).to_lowercase().contains(&q)
        || id.starts_with(q.strip_prefix("sha256:").unwrap_or(&q))
}

pub mod sort_keys {
    pub const NAME: &str = "name";
    pub const TAG: &str = "tag";
    pub const CREATED: &str = "created";
    pub const SIZE: &str = "size";
}

/// Sort (IMG-002). Ties: name, then tag, so the order is stable (SHL-004).
pub fn compare(a: &ImageRow, b: &ImageRow, sort: &SortState) -> std::cmp::Ordering {
    let name = |r: &ImageRow| (r.dangling, r.repo.to_lowercase(), r.tag.to_lowercase());
    let ord = match sort.key.as_ref() {
        sort_keys::TAG => a.tag.to_lowercase().cmp(&b.tag.to_lowercase()),
        sort_keys::CREATED => a.created.cmp(&b.created),
        sort_keys::SIZE => a.size.cmp(&b.size),
        _ => name(a).cmp(&name(b)),
    };
    let ord = if sort.descending { ord.reverse() } else { ord };
    ord.then_with(|| name(a).cmp(&name(b)))
        .then_with(|| a.key.cmp(&b.key))
}

/// Header totals (IMG-003): distinct images and their total size.
pub fn totals(images: &[ImageSummary]) -> (usize, u64) {
    let mut seen = HashSet::new();
    let mut size = 0;
    for i in images {
        if seen.insert(i.id.as_str()) {
            size += i.size;
        }
    }
    (seen.len(), size)
}

/// Filter + search + sort into list nodes.
pub fn build(
    rows: &[ImageRow],
    filter: ImageFilter,
    query: &str,
    sort: Option<&SortState>,
) -> Vec<ListNode<(), ImageRow>> {
    let default_sort = SortState {
        key: sort_keys::NAME.into(),
        descending: false,
    };
    let sort = sort.unwrap_or(&default_sort);
    let mut v: Vec<&ImageRow> = rows
        .iter()
        .filter(|r| filter.matches(r) && matches_search(r, query))
        .collect();
    v.sort_by(|a, b| compare(a, b, sort));
    v.into_iter()
        .map(|r| ListNode::Item {
            key: r.key.clone(),
            item: r.clone(),
        })
        .collect()
}

/// Prune candidates (IMG-003): unused images (dangling only, or all unused), one entry per
/// image id: `(id, label, size)`.
pub fn prune_candidates(rows: &[ImageRow], dangling_only: bool) -> Vec<(String, String, u64)> {
    let mut seen = HashSet::new();
    rows.iter()
        .filter(|r| r.in_use == 0 && (!dangling_only || r.dangling))
        .filter(|r| seen.insert(r.image_id.clone()))
        .map(|r| (r.image_id.clone(), r.label(), r.size))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use dk_core::ContainerState;
    use dk_core::fake::fixtures;

    fn sample() -> (Vec<ImageSummary>, Vec<ContainerSummary>) {
        let mut multi = fixtures::image("nginx:1.27", "nginx:1.27");
        multi.repo_tags.push("nginx:latest".into());
        let images = vec![
            multi,
            fixtures::image("redis:7", "redis"),
            fixtures::image("", "dangling"),
        ];
        // `fixtures::container` uses the nginx image id.
        let containers = vec![fixtures::container("web", ContainerState::Running)];
        (images, containers)
    }

    fn labels(nodes: &[ListNode<(), ImageRow>]) -> Vec<String> {
        nodes
            .iter()
            .map(|n| match n {
                ListNode::Item { item, .. } => format!("{}:{}", item.repo, item.tag),
                ListNode::Group { .. } => unreachable!(),
            })
            .collect()
    }

    #[test]
    fn img_001_one_row_per_tag_and_dangling_none() {
        let (images, containers) = sample();
        let r = rows(&images, &containers);
        assert_eq!(r.len(), 4, "2 tags + 1 + dangling");
        let nginx: Vec<&ImageRow> = r.iter().filter(|r| r.repo == "nginx").collect();
        assert_eq!(nginx.len(), 2);
        assert!(nginx.iter().all(|r| r.in_use == 1 && r.used_by == ["web"]));
        let d = r.iter().find(|r| r.dangling).unwrap();
        assert_eq!((d.repo.as_str(), d.tag.as_str()), ("<none>", "<none>"));
        assert_eq!(d.reference(), d.image_id);
        assert_eq!(nginx[0].reference(), format!("nginx:{}", nginx[0].tag));
    }

    #[test]
    fn img_002_filters_search_and_sort() {
        let (images, containers) = sample();
        let r = rows(&images, &containers);
        let all = build(&r, ImageFilter::All, "", None);
        assert_eq!(
            labels(&all),
            ["nginx:1.27", "nginx:latest", "redis:7", "<none>:<none>"]
        );
        assert_eq!(labels(&build(&r, ImageFilter::InUse, "", None)).len(), 2);
        assert_eq!(
            labels(&build(&r, ImageFilter::Unused, "", None)),
            ["redis:7", "<none>:<none>"]
        );
        assert_eq!(
            labels(&build(&r, ImageFilter::Dangling, "", None)),
            ["<none>:<none>"]
        );
        assert_eq!(
            labels(&build(&r, ImageFilter::All, "REDIS", None)),
            ["redis:7"]
        );
        let id = r
            .iter()
            .find(|r| r.repo == "redis")
            .unwrap()
            .image_id
            .clone();
        let prefix = &id["sha256:".len().."sha256:".len() + 6];
        assert_eq!(
            labels(&build(&r, ImageFilter::All, prefix, None)),
            ["redis:7"]
        );
        let by_tag_desc = SortState {
            key: sort_keys::TAG.into(),
            descending: true,
        };
        assert_eq!(
            labels(&build(&r, ImageFilter::All, "", Some(&by_tag_desc)))[0],
            "nginx:latest"
        );
    }

    #[test]
    fn img_003_totals_count_distinct_images() {
        let (images, _) = sample();
        assert_eq!(totals(&images), (3, 3 * 187_000_000));
        let r = rows(&images, &sample().1);
        assert_eq!(prune_candidates(&r, true).len(), 1);
        assert_eq!(prune_candidates(&r, false).len(), 2);
    }

    #[test]
    fn filter_roundtrip() {
        for (v, _) in ImageFilter::OPTIONS {
            assert_eq!(ImageFilter::parse(v).as_str(), *v);
        }
    }
}
