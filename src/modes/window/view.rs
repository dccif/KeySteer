//! Read-only window presentation data; no scene construction.
use super::*;
use crate::api::presentation::{View, WindowView};

impl WindowMode {
    pub(super) fn detail(&self) -> String {
        if self.library_open {
            let mut detail = format!(
                "Restore\nType a layout number · page {} / {}",
                self.library_page + 1,
                self.saved_layouts
                    .len()
                    .div_ceil(super::presets::PAGE_SIZE)
                    .max(1)
            );
            if self.saved_layouts.is_empty() {
                detail.push_str("\nNo saved layouts · save a layout in the editor");
            }
            for layout in self
                .saved_layouts
                .iter()
                .skip(self.library_page * super::presets::PAGE_SIZE)
                .take(super::presets::PAGE_SIZE)
            {
                detail.push_str(&format!("\n{}   {}", layout.id, layout.name()));
            }
            if !self.number.display.is_empty() {
                detail.push_str(&format!("\nInput: {}", self.number.display));
            }
            if let Some(status) = &self.status {
                detail.push_str(&format!("\n{status}"));
            }
            return detail;
        }
        let state = match self.edit.as_ref().map(|e| &e.model) {
            Some(EditModel::Quick(quick)) => format!("Quick · {}", quick.caption()),
            Some(EditModel::Tree(tree)) => format!("Edit · area `{}", tree.selected),
            None => if self.size { "Resize" } else { "Move" }.into(),
        };
        let mut detail = state;
        if let Some(window) = &self.target {
            let app = window
                .app
                .rsplit(['/', '\\'])
                .next()
                .unwrap_or(&window.app)
                .trim_end_matches(".exe");
            detail.push_str(&format!("\n{app}\n{}", window.title));
        }
        if !self.number.display.is_empty() {
            detail.push_str("\nInput: ");
            detail.push_str(&self.number.display);
            if self.number.pending() {
                detail.push('…');
            }
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
        if self.library_open {
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
