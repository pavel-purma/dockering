//! Pure list model behind [`super::ListTable`]: tree flattening with group expand state, a
//! cursor separate from the checkbox multi-selection, quick find, and id-stable refresh
//! (KBD-007, KBD-031…038, SHL-004). No GPUI types; unit-tested.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::ops::Range;

use gpui_kit::SharedString;

/// A tree node supplied by the page (already filtered and sorted).
#[derive(Debug, Clone, PartialEq)]
pub enum ListNode<G, I> {
    Group {
        key: SharedString,
        group: G,
        children: Vec<(SharedString, I)>,
    },
    Item {
        key: SharedString,
        item: I,
    },
}

impl<G, I> ListNode<G, I> {
    pub fn key(&self) -> &SharedString {
        match self {
            ListNode::Group { key, .. } | ListNode::Item { key, .. } => key,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum RowKind<G, I> {
    Group {
        group: G,
        members: Vec<SharedString>,
        expanded: bool,
    },
    Item(I),
}

/// One visible row.
#[derive(Debug, Clone, PartialEq)]
pub struct ListRow<G, I> {
    pub key: SharedString,
    pub depth: usize,
    /// Group key of a member row.
    pub parent: Option<SharedString>,
    pub kind: RowKind<G, I>,
}

impl<G, I> ListRow<G, I> {
    pub fn is_group(&self) -> bool {
        matches!(self.kind, RowKind::Group { .. })
    }
    pub fn item(&self) -> Option<&I> {
        match &self.kind {
            RowKind::Item(i) => Some(i),
            RowKind::Group { .. } => None,
        }
    }
    pub fn group(&self) -> Option<&G> {
        match &self.kind {
            RowKind::Group { group, .. } => Some(group),
            RowKind::Item(_) => None,
        }
    }
    /// Item keys this row stands for (itself, or a group's members).
    pub fn item_keys(&self) -> Vec<SharedString> {
        match &self.kind {
            RowKind::Item(_) => vec![self.key.clone()],
            RowKind::Group { members, .. } => members.clone(),
        }
    }
}

/// Current sort (CON-003).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SortState {
    pub key: SharedString,
    pub descending: bool,
}

#[derive(Debug, Clone)]
pub struct ListModel<G, I> {
    nodes: Vec<ListNode<G, I>>,
    rows: Vec<ListRow<G, I>>,
    /// Explicit expand state per group key; missing = `default_expanded`.
    expanded: HashMap<SharedString, bool>,
    pub default_expanded: bool,
    /// Groups forced open (e.g. a search match in a member, CON-005).
    force_expanded: HashSet<SharedString>,
    selected: BTreeSet<SharedString>,
    cursor: Option<SharedString>,
    /// Where the cursor goes when the cursor row disappears after a delete (KBD-007).
    focus_after_remove: Option<SharedString>,
    pub sort: Option<SortState>,
    pub visible: Range<usize>,
}

impl<G: Clone, I: Clone> Default for ListModel<G, I> {
    fn default() -> Self {
        Self::new(false)
    }
}

impl<G: Clone, I: Clone> ListModel<G, I> {
    pub fn new(default_expanded: bool) -> Self {
        Self {
            nodes: Vec::new(),
            rows: Vec::new(),
            expanded: HashMap::new(),
            default_expanded,
            force_expanded: HashSet::new(),
            selected: BTreeSet::new(),
            cursor: None,
            focus_after_remove: None,
            sort: None,
            visible: 0..0,
        }
    }

    // ── data ───────────────────────────────────────────────────────────────────────────

    /// Replaces the tree. Cursor and selection are preserved by key (SHL-004, KBD-007);
    /// selected keys that no longer exist are dropped.
    pub fn set_nodes(&mut self, nodes: Vec<ListNode<G, I>>) {
        self.nodes = nodes;
        self.flatten();
        let existing: HashSet<SharedString> = self.all_item_keys().into_iter().collect();
        self.selected.retain(|k| existing.contains(k));
        if let Some(cursor) = self.cursor.clone()
            && self.index_of(&cursor).is_none()
        {
            self.cursor = self
                .focus_after_remove
                .take()
                .filter(|k| self.index_of(k).is_some())
                .or_else(|| {
                    // A member of a now-collapsed group: put the cursor on its group.
                    self.nodes.iter().find_map(|n| match n {
                        ListNode::Group { key, children, .. }
                            if children.iter().any(|(k, _)| *k == cursor) =>
                        {
                            Some(key.clone())
                        }
                        _ => None,
                    })
                })
                .or_else(|| self.rows.first().map(|r| r.key.clone()));
        } else {
            self.focus_after_remove = None;
        }
    }

    pub fn nodes(&self) -> &[ListNode<G, I>] {
        &self.nodes
    }

    pub fn nodes_mut(&mut self) -> &mut Vec<ListNode<G, I>> {
        &mut self.nodes
    }

    pub fn rows(&self) -> &[ListRow<G, I>] {
        &self.rows
    }

    pub fn row(&self, ix: usize) -> Option<&ListRow<G, I>> {
        self.rows.get(ix)
    }

    pub fn index_of(&self, key: &str) -> Option<usize> {
        self.rows.iter().position(|r| r.key == key)
    }

    pub fn row_by_key(&self, key: &str) -> Option<&ListRow<G, I>> {
        self.rows.iter().find(|r| r.key == key)
    }

    /// Every item key in the tree (including members of collapsed groups).
    pub fn all_item_keys(&self) -> Vec<SharedString> {
        let mut out = Vec::new();
        for n in &self.nodes {
            match n {
                ListNode::Item { key, .. } => out.push(key.clone()),
                ListNode::Group { children, .. } => {
                    out.extend(children.iter().map(|(k, _)| k.clone()))
                }
            }
        }
        out
    }

    /// The item for a key anywhere in the tree.
    pub fn find_item(&self, key: &str) -> Option<&I> {
        for n in &self.nodes {
            match n {
                ListNode::Item { key: k, item } if k == key => return Some(item),
                ListNode::Group { children, .. } => {
                    if let Some((_, i)) = children.iter().find(|(k, _)| k == key) {
                        return Some(i);
                    }
                }
                _ => {}
            }
        }
        None
    }

    pub fn flatten(&mut self) {
        let mut rows = Vec::with_capacity(self.nodes.len());
        for n in &self.nodes {
            match n {
                ListNode::Item { key, item } => rows.push(ListRow {
                    key: key.clone(),
                    depth: 0,
                    parent: None,
                    kind: RowKind::Item(item.clone()),
                }),
                ListNode::Group {
                    key,
                    group,
                    children,
                } => {
                    let expanded = self.is_expanded(key);
                    rows.push(ListRow {
                        key: key.clone(),
                        depth: 0,
                        parent: None,
                        kind: RowKind::Group {
                            group: group.clone(),
                            members: children.iter().map(|(k, _)| k.clone()).collect(),
                            expanded,
                        },
                    });
                    if expanded {
                        rows.extend(children.iter().map(|(k, i)| ListRow {
                            key: k.clone(),
                            depth: 1,
                            parent: Some(key.clone()),
                            kind: RowKind::Item(i.clone()),
                        }));
                    }
                }
            }
        }
        self.rows = rows;
    }

    // ── groups ─────────────────────────────────────────────────────────────────────────

    pub fn is_expanded(&self, group: &str) -> bool {
        self.force_expanded.contains(group)
            || self
                .expanded
                .get(group)
                .copied()
                .unwrap_or(self.default_expanded)
    }

    /// Loads persisted expand state (CON-011).
    pub fn set_expanded_state(&mut self, state: impl IntoIterator<Item = (String, bool)>) {
        self.expanded = state.into_iter().map(|(k, v)| (k.into(), v)).collect();
        self.flatten();
    }

    pub fn expanded_state(&self) -> impl Iterator<Item = (&SharedString, &bool)> {
        self.expanded.iter()
    }

    pub fn set_force_expanded(&mut self, keys: HashSet<SharedString>) {
        self.force_expanded = keys;
    }

    /// Returns true if the state changed.
    pub fn set_expanded(&mut self, group: &str, expanded: bool) -> bool {
        if self.is_expanded(group) == expanded && !self.force_expanded.contains(group) {
            return false;
        }
        self.force_expanded.remove(group);
        self.expanded.insert(group.to_owned().into(), expanded);
        self.flatten();
        true
    }

    pub fn toggle_group(&mut self, group: &str) -> bool {
        let now = self.is_expanded(group);
        self.set_expanded(group, !now)
    }

    pub fn set_all_expanded(&mut self, expanded: bool) -> bool {
        let keys: Vec<SharedString> = self
            .nodes
            .iter()
            .filter_map(|n| match n {
                ListNode::Group { key, .. } => Some(key.clone()),
                _ => None,
            })
            .collect();
        let mut changed = false;
        for k in keys {
            if self.is_expanded(&k) != expanded {
                changed = true;
            }
            self.force_expanded.remove(&k);
            self.expanded.insert(k, expanded);
        }
        self.flatten();
        changed
    }

    // ── cursor ─────────────────────────────────────────────────────────────────────────

    pub fn cursor(&self) -> Option<&SharedString> {
        self.cursor.as_ref()
    }

    pub fn cursor_ix(&self) -> Option<usize> {
        self.cursor.as_deref().and_then(|k| self.index_of(k))
    }

    pub fn cursor_row(&self) -> Option<&ListRow<G, I>> {
        self.cursor_ix().and_then(|ix| self.rows.get(ix))
    }

    pub fn set_cursor(&mut self, key: Option<SharedString>) {
        self.cursor = key;
    }

    pub fn set_cursor_ix(&mut self, ix: usize) {
        self.cursor = self.rows.get(ix).map(|r| r.key.clone());
    }

    /// `←` on a member row moves to its group (KBD-033). Returns the new index.
    pub fn cursor_to_parent(&mut self) -> Option<usize> {
        let parent = self.cursor_row()?.parent.clone()?;
        self.cursor = Some(parent);
        self.cursor_ix()
    }

    // ── multi-selection (checkboxes) ───────────────────────────────────────────────────

    pub fn selected(&self) -> &BTreeSet<SharedString> {
        &self.selected
    }

    pub fn is_selected(&self, key: &str) -> bool {
        self.selected.contains(key)
    }

    /// A group counts as selected when all its members are.
    pub fn row_selected(&self, row: &ListRow<G, I>) -> bool {
        let keys = row.item_keys();
        !keys.is_empty() && keys.iter().all(|k| self.selected.contains(k))
    }

    /// `Space` (KBD-034). On a group row, toggles all members.
    pub fn toggle_selected(&mut self, key: &str) {
        let Some(row) = self.row_by_key(key).cloned() else {
            // Not visible (e.g. a collapsed member): toggle the key itself.
            if !self.selected.remove(key) {
                self.selected.insert(key.to_owned().into());
            }
            return;
        };
        let keys = row.item_keys();
        if self.row_selected(&row) {
            for k in keys {
                self.selected.remove(&k);
            }
        } else {
            self.selected.extend(keys);
        }
    }

    /// `Mod+A`: every visible row; collapsed groups select their members (KBD-038).
    pub fn select_all(&mut self) {
        let keys: Vec<SharedString> = self.rows.iter().flat_map(|r| r.item_keys()).collect();
        self.selected.extend(keys);
    }

    pub fn clear_selection(&mut self) -> bool {
        let had = !self.selected.is_empty();
        self.selected.clear();
        had
    }

    /// `Shift+↑/↓` (KBD-035): select the cursor row, move, select the new row.
    pub fn extend_selection(&mut self, down: bool) -> Option<usize> {
        let ix = self.cursor_ix().unwrap_or(0);
        let row = self.rows.get(ix)?.clone();
        self.selected.extend(row.item_keys());
        let next = if down {
            (ix + 1).min(self.rows.len().saturating_sub(1))
        } else {
            ix.saturating_sub(1)
        };
        let next_row = self.rows.get(next)?.clone();
        self.selected.extend(next_row.item_keys());
        self.cursor = Some(next_row.key);
        Some(next)
    }

    /// What a command acts on (KBD-038): the multi-selection when ≥ 2 items are selected,
    /// otherwise the cursor row (a group row stands for its members).
    pub fn targets(&self) -> Vec<SharedString> {
        if self.selected.len() >= 2 {
            return self.selected.iter().cloned().collect();
        }
        self.cursor_row().map(|r| r.item_keys()).unwrap_or_default()
    }

    /// Before removing `keys`: remember the row the cursor should land on (the next row
    /// after the last removed one, or the previous one at the end, KBD-007).
    pub fn prepare_remove(&mut self, keys: &[SharedString]) {
        let removing: HashSet<&SharedString> = keys.iter().collect();
        let gone = |r: &ListRow<G, I>| {
            removing.contains(&r.key)
                || (r.is_group() && r.item_keys().iter().all(|k| removing.contains(k)))
        };
        let Some(last) = self
            .rows
            .iter()
            .rposition(|r| removing.contains(&r.key))
            .or_else(|| self.cursor_ix())
        else {
            return;
        };
        let after = self.rows[last + 1..].iter().find(|r| !gone(r));
        let before = self.rows[..last].iter().rev().find(|r| !gone(r));
        self.focus_after_remove = after.or(before).map(|r| r.key.clone());
        if self.cursor.as_ref().is_some_and(|c| removing.contains(c)) {
            // Keep the cursor key so `set_nodes` notices it vanished.
        }
    }

    // ── quick find (KBD-037) ───────────────────────────────────────────────────────────

    /// The first row at or after `from` (wrapping) whose text contains `query`
    /// (case-insensitive).
    pub fn find(
        &self,
        query: &str,
        from: usize,
        text: impl Fn(&ListRow<G, I>) -> String,
    ) -> Option<usize> {
        let q = query.trim().to_lowercase();
        if q.is_empty() || self.rows.is_empty() {
            return None;
        }
        let n = self.rows.len();
        (0..n)
            .map(|off| (from + off) % n)
            .find(|&ix| text(&self.rows[ix]).to_lowercase().contains(&q))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    type M = ListModel<&'static str, &'static str>;

    fn s(v: &str) -> SharedString {
        v.to_owned().into()
    }

    fn tree() -> Vec<ListNode<&'static str, &'static str>> {
        vec![
            ListNode::Group {
                key: s("g:shop"),
                group: "shop",
                children: vec![(s("web"), "web"), (s("api"), "api"), (s("db"), "db")],
            },
            ListNode::Item {
                key: s("redis"),
                item: "redis",
            },
            ListNode::Item {
                key: s("scratch"),
                item: "scratch",
            },
        ]
    }

    fn keys(m: &M) -> Vec<&str> {
        m.rows().iter().map(|r| r.key.as_ref()).collect()
    }

    #[test]
    fn con_011_list_default_expands_groups() {
        // `ListTable` builds its model with `default_expanded = true` (CON-011).
        let mut m = M::new(true);
        m.set_nodes(tree());
        assert_eq!(keys(&m), ["g:shop", "web", "api", "db", "redis", "scratch"]);
    }

    #[test]
    fn con_010_groups_start_collapsed_and_flatten_on_expand() {
        let mut m = M::new(false);
        m.set_nodes(tree());
        assert_eq!(keys(&m), ["g:shop", "redis", "scratch"]);
        assert!(m.toggle_group("g:shop"));
        assert_eq!(keys(&m), ["g:shop", "web", "api", "db", "redis", "scratch"]);
        assert_eq!(m.rows()[1].depth, 1);
        assert_eq!(m.rows()[1].parent.as_deref(), Some("g:shop"));
    }

    #[test]
    fn kbd_033_left_on_member_goes_to_group() {
        let mut m = M::new(true);
        m.set_nodes(tree());
        m.set_cursor(Some(s("api")));
        assert_eq!(m.cursor_to_parent(), Some(0));
        assert_eq!(m.cursor().map(|c| c.as_ref()), Some("g:shop"));
    }

    #[test]
    fn kbd_034_space_on_group_toggles_members() {
        let mut m = M::new(false);
        m.set_nodes(tree());
        m.toggle_selected("g:shop");
        assert_eq!(m.selected().len(), 3);
        assert!(m.row_selected(&m.rows()[0].clone()));
        m.toggle_selected("g:shop");
        assert!(m.selected().is_empty());
    }

    #[test]
    fn kbd_038_select_all_includes_collapsed_members_and_targets() {
        let mut m = M::new(false);
        m.set_nodes(tree());
        m.select_all();
        assert_eq!(m.selected().len(), 5);
        assert_eq!(m.targets().len(), 5);
        m.clear_selection();
        m.set_cursor(Some(s("redis")));
        assert_eq!(m.targets(), vec![s("redis")]);
        m.set_cursor(Some(s("g:shop")));
        assert_eq!(m.targets().len(), 3);
    }

    #[test]
    fn shl_004_refresh_keeps_cursor_and_selection_by_key() {
        let mut m = M::new(false);
        m.set_nodes(tree());
        m.set_cursor(Some(s("scratch")));
        m.toggle_selected("redis");
        let mut t = tree();
        t.insert(
            0,
            ListNode::Item {
                key: s("new"),
                item: "new",
            },
        );
        m.set_nodes(t);
        assert_eq!(m.cursor_ix(), Some(3));
        assert!(m.is_selected("redis"));
    }

    #[test]
    fn kbd_007_cursor_moves_to_next_row_after_delete() {
        let mut m = M::new(false);
        m.set_nodes(tree());
        m.set_cursor(Some(s("redis")));
        m.prepare_remove(&[s("redis")]);
        let t: Vec<_> = tree().into_iter().filter(|n| n.key() != "redis").collect();
        m.set_nodes(t);
        assert_eq!(m.cursor().map(|c| c.as_ref()), Some("scratch"));
        // At the end: the previous row.
        m.prepare_remove(&[s("scratch")]);
        let t: Vec<_> = tree()
            .into_iter()
            .filter(|n| n.key() != "redis" && n.key() != "scratch")
            .collect();
        m.set_nodes(t);
        assert_eq!(m.cursor().map(|c| c.as_ref()), Some("g:shop"));
    }

    #[test]
    fn extend_selection_moves_cursor() {
        let mut m = M::new(false);
        m.set_nodes(tree());
        m.set_cursor(Some(s("redis")));
        assert_eq!(m.extend_selection(true), Some(2));
        assert!(m.is_selected("redis") && m.is_selected("scratch"));
    }

    #[test]
    fn kbd_037_find_wraps() {
        let mut m = M::new(true);
        m.set_nodes(tree());
        let text = |r: &ListRow<&str, &str>| r.key.to_string();
        assert_eq!(m.find("RED", 0, text), Some(4));
        assert_eq!(m.find("a", 3, text), Some(5));
        assert_eq!(m.find("api", 3, text), Some(2), "wraps around");
        assert_eq!(m.find("zzz", 0, text), None);
        assert_eq!(m.find("", 0, text), None);
    }

    #[test]
    fn member_cursor_falls_back_to_group_when_collapsed() {
        let mut m = M::new(true);
        m.set_nodes(tree());
        m.set_cursor(Some(s("api")));
        m.set_expanded("g:shop", false);
        m.set_nodes(tree());
        assert_eq!(m.cursor().map(|c| c.as_ref()), Some("g:shop"));
    }

    #[test]
    fn force_expanded_opens_groups_until_toggled() {
        let mut m = M::new(false);
        m.set_force_expanded([s("g:shop")].into_iter().collect());
        m.set_nodes(tree());
        assert_eq!(m.rows().len(), 6);
        m.toggle_group("g:shop");
        assert_eq!(m.rows().len(), 3);
    }
}
