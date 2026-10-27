//! Fork: a whole tab moving to another workspace with its layout and running panes
//! (`tab.move_to_workspace`).

use super::{Tab, Workspace};

impl Workspace {
    /// Takes tab `tab_idx` out with its panes still running, for another workspace's
    /// `insert_moved_tab`. A workspace left without tabs must be removed by the caller.
    pub(crate) fn take_tab_for_move(&mut self, tab_idx: usize) -> Option<Tab> {
        if tab_idx >= self.tabs.len() {
            return None;
        }
        let tab = self.tabs.remove(tab_idx);
        for pane_id in tab.panes.keys() {
            self.unregister_pane(*pane_id);
        }
        self.adjust_active_tab_after_removal(tab_idx);
        Some(tab)
    }

    /// Adds a tab taken from another workspace as the last tab, numbered as this workspace's
    /// next tab with its panes numbered in layout order; returns its index.
    pub(crate) fn insert_moved_tab(&mut self, mut tab: Tab) -> usize {
        tab.number = self.next_public_tab_number;
        self.next_public_tab_number += 1;
        for pane_id in tab.layout.pane_ids() {
            if !self.public_pane_numbers.contains_key(&pane_id) {
                self.register_new_pane_with_number(pane_id, self.next_public_pane_number);
            }
        }
        self.tabs.push(tab);
        self.tabs.len() - 1
    }
}

#[cfg(test)]
impl Tab {
    /// The layout tree as text: pane IDs, split directions and ratios.
    pub(crate) fn layout_shape_for_test(&self) -> String {
        fn shape(node: &crate::layout::Node) -> String {
            match node {
                crate::layout::Node::Pane(pane_id) => pane_id.raw().to_string(),
                crate::layout::Node::Split {
                    direction,
                    ratio,
                    first,
                    second,
                } => format!("({direction:?} {ratio} {} {})", shape(first), shape(second)),
            }
        }
        shape(self.layout.root())
    }
}

#[cfg(test)]
mod tests {
    use ratatui::layout::Direction;

    use super::*;

    #[test]
    fn moved_tab_keeps_its_layout_and_takes_the_targets_numbers() {
        let mut source = Workspace::test_new("source");
        source.test_add_tab(Some("split"));
        source.switch_tab(1);
        let second = source.test_split(Direction::Horizontal);
        let first = source.tabs[1].root_pane;
        let layout_before = source.tabs[1].layout_shape_for_test();
        let mut target = Workspace::test_new("target");
        target.test_add_tab(None);

        let tab = source.take_tab_for_move(1).unwrap();
        assert_eq!(source.tabs.len(), 1);
        assert_eq!(source.active_tab, 0);
        assert_eq!(source.public_pane_number(first), None);
        assert_eq!(source.public_pane_number(second), None);

        let next_tab = target.next_public_tab_number;
        let next_pane = target.next_public_pane_number;
        let tab_idx = target.insert_moved_tab(tab);
        assert_eq!(tab_idx, 2);
        let moved = &target.tabs[tab_idx];
        assert_eq!(moved.number, next_tab);
        assert_eq!(moved.custom_name.as_deref(), Some("split"));
        assert_eq!(moved.layout_shape_for_test(), layout_before);
        assert_eq!(target.public_pane_number(first), Some(next_pane));
        assert_eq!(target.public_pane_number(second), Some(next_pane + 1));
        assert_eq!(target.active_tab, 0);
        source.assert_invariants_for_test();
        target.assert_invariants_for_test();
    }

    #[test]
    fn taking_the_only_tab_empties_the_workspace() {
        let mut source = Workspace::test_new("source");

        assert!(source.take_tab_for_move(1).is_none());
        assert!(source.take_tab_for_move(0).is_some());
        assert!(source.tabs.is_empty());
    }

    #[test]
    fn taking_a_tab_before_the_active_one_keeps_the_active_tab() {
        let mut source = Workspace::test_new("source");
        source.test_add_tab(Some("two"));
        source.test_add_tab(Some("three"));
        source.switch_tab(2);

        source.take_tab_for_move(0).unwrap();

        assert_eq!(source.active_tab, 1);
        assert_eq!(source.tabs[1].custom_name.as_deref(), Some("three"));
    }
}
