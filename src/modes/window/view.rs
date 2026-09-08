//! Read-only window presentation data; no scene construction.
use super::*;
use crate::api::presentation::{View, WindowView};

impl WindowMode {
    pub(super) fn detail(&self) -> String {
        let state = match self.edit.as_ref().map(|e| &e.model) {
            Some(EditModel::Quick(quick)) => format!("Quick · {}", quick.caption()),
            Some(EditModel::Tree(tree)) => format!("Edit · area `{}", tree.selected),
            None => if self.size { "size" } else { "move" }.into(),
        };
        let title = self
            .target
            .as_ref()
            .map_or("No target", |w| w.title.as_str());
        let mut detail = format!("{state} · {}", title.chars().take(40).collect::<String>());
        if self.number.pending() {
            detail.push_str(&format!(
                "\nNumber: {}{}…",
                if self.number.slot { "`" } else { "" },
                self.number.prefix
            ));
        }
        if let Some(source) = self.swap_source.and_then(|id| self.numbers.get(&id)) {
            detail.push_str(&format!("\nWindow {source} → window number / `area"));
        }
        if let Some(status) = &self.status {
            detail.push('\n');
            detail.extend(status.chars().take(140));
        }
        detail
    }

    pub(super) fn view(&self) -> View<'_> {
        if self.temporary {
            return View::Empty;
        }
        let tree = self
            .edit
            .as_ref()
            .filter(|edit| edit.ready)
            .and_then(|edit| match &edit.model {
                EditModel::Tree(tree) => Some(tree),
                _ => None,
            });
        View::Window(WindowView {
            ui: &self.settings.ui,
            border_width: self.settings.border_width,
            target: self.target.as_ref(),
            screen: self.screen,
            inventory: &self.inventory,
            visible: &self.visible,
            numbers: &self.numbers,
            tree,
            gap: self.settings.gap * self.edit.as_ref().map_or(1.0, |edit| edit.gap_scale),
        })
    }
}
