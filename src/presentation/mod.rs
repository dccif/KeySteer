#![forbid(unsafe_code)]
//! Central scene composition. Depends only on the public data/port vocabulary;
//! no native calls, mode instances, configuration documents or event routing.

pub(crate) mod dynamic;
pub(crate) mod grid;
pub(crate) mod hint;
pub(crate) mod key_help;
pub(crate) mod label_placement;
pub(crate) mod recursive_grid;
pub(crate) mod screen_selector;
pub(crate) mod window;

use crate::api::presentation::{Presenter, View};
use crate::api::{HostContext, OverlayScene};

pub struct Composer;
pub static COMPOSER: Composer = Composer;

pub(crate) fn label_scale(scale: f64) -> f64 {
    #[cfg(target_os = "windows")]
    {
        crate::api::overlay::normalized_label_scale(scale)
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = scale;
        1.0
    }
}

impl Presenter for Composer {
    fn compose(&self, view: View<'_>, context: &HostContext<'_>) -> OverlayScene {
        match view {
            View::Empty => OverlayScene::new(),
            View::Grid(view) => view.scene(context.palette),
            View::RecursiveGrid(view) => view.scene(context.palette),
            View::Window(view) => view.scene(context),
            View::ScreenSelector(view) => view.scene(context),
            View::Hints(view) => view.scene(context),
            View::HintSelection(view) => view.scene(context),
            View::Status(view) => view.scene(context),
        }
    }

    fn prepare_hints(
        &self,
        content: crate::api::presentation::HintContent<'_>,
        layers: &mut crate::api::presentation::VisualLayerPlan,
        workspace: &mut Option<Vec<(usize, crate::api::Rect)>>,
        context: &HostContext<'_>,
    ) {
        hint::prepare_hints(content, layers, workspace, context);
    }
}

/// Bound labels by Unicode scalar count without splitting UTF-8.
pub(crate) fn elide(text: &str, capacity: usize) -> String {
    let mut characters = text.chars();
    let mut value: String = characters.by_ref().take(capacity).collect();
    if characters.next().is_some() && capacity > 0 {
        value.pop();
        value.push('…');
    }
    value
}

pub(crate) fn text_units(text: &str) -> f64 {
    text.chars()
        .map(|c| if c.is_ascii() { 0.75 } else { 1.0 })
        .sum()
}

/// Caption budget in font-size units; reserve a full em for non-ASCII text.
pub(crate) fn elide_width(text: &str, units: f64) -> String {
    if text_units(text) <= units {
        return text.into();
    }
    let mut output = String::new();
    let mut used = 1.0; // ellipsis
    for c in text.chars() {
        used += if c.is_ascii() { 0.75 } else { 1.0 };
        if used > units {
            break;
        }
        output.push(c);
    }
    if units >= 1.0 {
        output.push('…');
    }
    output
}
