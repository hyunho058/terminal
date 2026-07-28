//! The per-card menu's item set (sticky-tab-title D4/R3.6) — the app's half of the
//! hamburger the renderer draws and hit-tests.
//!
//! The renderer knows only *how many* items a menu has and which position was pressed
//! (D10/R3.2); which action a position means is decided here, because the app is what
//! chose the items in the first place. `Reset to auto` appears only for a tab that has a
//! user title, so a menu carries 2 or 3 items and position 2 means different things on
//! different tabs — deriving the labels and the actions from ONE list is what keeps a
//! press from ever firing the wrong one.

/// One entry in an open card menu.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuItem {
    /// Open the rename field on this tab (R3.6).
    Rename,
    /// Clear the user title, reverting the card to its live auto label (R3.6/D5).
    /// Offered only when there is a user title to clear.
    ResetToAuto,
    /// Close this tab (R3.6). On the last tab this exits the app, exactly as Cmd+W does.
    Close,
}

impl MenuItem {
    /// The label drawn for this item.
    pub fn label(self) -> &'static str {
        match self {
            MenuItem::Rename => "Rename",
            MenuItem::ResetToAuto => "Reset to auto",
            MenuItem::Close => "Close",
        }
    }
}

/// The items for a tab, in draw order (R3.6). `has_user_title` decides whether the
/// reset entry is offered — there is nothing to reset on a tab that was never named.
pub fn items_for(has_user_title: bool) -> Vec<MenuItem> {
    if has_user_title {
        vec![MenuItem::Rename, MenuItem::ResetToAuto, MenuItem::Close]
    } else {
        vec![MenuItem::Rename, MenuItem::Close]
    }
}

/// The labels for [`items_for`], for the renderer's `CardMenu`.
pub fn labels(items: &[MenuItem]) -> Vec<String> {
    items.iter().map(|i| i.label().to_string()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reset_is_offered_only_when_there_is_a_title_to_reset() {
        // R3.6/D4 — a never-named tab has nothing to reset, so the entry is absent and
        // the menu is 2 items rather than 3.
        assert_eq!(items_for(false), vec![MenuItem::Rename, MenuItem::Close]);
        assert_eq!(
            items_for(true),
            vec![MenuItem::Rename, MenuItem::ResetToAuto, MenuItem::Close]
        );
    }

    #[test]
    fn position_two_means_different_things_on_different_tabs() {
        // The reason item identity is resolved through this list and never hard-coded by
        // index: on an unnamed tab index 1 is Close, on a named tab it is Reset.
        assert_eq!(items_for(false)[1], MenuItem::Close);
        assert_eq!(items_for(true)[1], MenuItem::ResetToAuto);
        assert_eq!(items_for(true)[2], MenuItem::Close);
    }

    #[test]
    fn labels_match_the_items_one_for_one() {
        // The renderer sizes and hit-tests the popup from the label count, so a mismatch
        // here would make a press land on the wrong row.
        let items = items_for(true);
        let labels = labels(&items);
        assert_eq!(labels.len(), items.len());
        assert_eq!(labels, vec!["Rename", "Reset to auto", "Close"]);
    }
}
