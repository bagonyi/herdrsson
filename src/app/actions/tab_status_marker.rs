//! Fork: leaves the Tab Status plugin's marker out of notifications.

/// Status markers the Tab Status plugin puts in front of tab names. A
/// notification already names the status, and the plugin swaps the marker only
/// after the notification is made, so it would show the old one.
const TAB_STATUS_MARKERS: [&str; 3] = ["🟢 ", "⏳ ", "🟠 "];

pub(super) fn strip(label: &str) -> &str {
    TAB_STATUS_MARKERS
        .iter()
        .find_map(|marker| label.strip_prefix(marker))
        .unwrap_or(label)
}

#[cfg(test)]
mod tests {
    use crate::app::actions::notification_context;
    use crate::workspace::Workspace;

    #[test]
    fn notification_context_leaves_out_tab_status_marker() {
        let mut ws = Workspace::test_new("workflow");
        let marked = ws.test_add_tab(Some("⏳ orchestrator"));
        let unmarked = ws.test_add_tab(Some("🚀 deploy"));
        let flagged = ws.test_add_tab(Some("🟠 🚩 review"));
        let pane = ws.tabs[marked].root_pane;
        let other = ws.tabs[unmarked].root_pane;
        let flagged = ws.tabs[flagged].root_pane;

        assert_eq!(
            notification_context(&ws, "workflow", 0, pane),
            "workflow · 1 · orchestrator"
        );
        assert_eq!(
            notification_context(&ws, "workflow", 0, other),
            "workflow · 1 · 🚀 deploy"
        );
        // The plugin's flag is part of the tab's name, so it stays.
        assert_eq!(
            notification_context(&ws, "workflow", 0, flagged),
            "workflow · 1 · 🚩 review"
        );
    }
}
