#![forbid(unsafe_code)]
//! Central scene composition. Depends only on the public data/port vocabulary;
//! no native calls, mode instances, configuration documents or event routing.

pub(crate) mod dynamic;
pub(crate) mod grid;
pub(crate) mod hint;
pub(crate) mod key_help;
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
