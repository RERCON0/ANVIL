//! Closing commands keep consent and stable pane IDs separate from drawing.
use super::*;

impl AnvilApp {
    /// True means a dialog is pending; no process has been stopped.
    pub(super) fn confirm_sessions(&mut self, panes: Vec<PaneId>, whole_window: bool) -> bool {
        let requested: std::collections::HashSet<_> = panes.iter().copied().collect();
        let running = self
            .tabs
            .iter()
            .flat_map(|tab| tab.panes.iter())
            .filter(|(id, entry)| requested.contains(id) && !entry.exited && entry.live().is_some())
            .count();
        if running == 0 {
            return false;
        }
        if self.ui.dialog.is_none() {
            self.ui.dialog = Some(DialogState::CloseSessions { panes, whole_window, running });
        }
        true
    }

    pub(crate) fn request_window_close(&mut self) -> bool {
        if std::mem::take(&mut self.approved_close_window) {
            return true;
        }
        if self.ui.dialog.is_some() {
            return false;
        }
        let panes = self.tabs.iter().flat_map(|tab| tab.panes.keys().copied()).collect();
        !self.confirm_sessions(panes, true)
    }

    pub(super) fn finish_close(&mut self, panes: Vec<PaneId>, whole_window: bool) {
        if whole_window {
            self.approved_close_window = true;
            return;
        }
        let requested: std::collections::HashSet<_> = panes.into_iter().collect();
        // Close whole tabs together so Reopen keeps the original complete layout.
        for index in (0..self.tabs.len()).rev() {
            if self.tabs[index].panes.keys().all(|id| requested.contains(id)) {
                self.close_tab_unchecked(index);
            } else {
                let ids: Vec<_> = self.tabs[index].panes.keys().copied().filter(|id| requested.contains(id)).collect();
                for id in ids {
                    self.close_pane_in_unchecked(index, id);
                }
            }
        }
    }
}
