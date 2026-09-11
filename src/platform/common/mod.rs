//! Infrastructure shared by native backends.

pub(crate) mod app_info;
pub(crate) mod character_candidates;
pub(crate) mod disposition_mailbox;
pub(crate) mod partial_batcher;
pub(crate) mod scan_mailbox;
pub(crate) mod spatial_index;
pub(crate) mod update;
pub(crate) mod window_geometry;
pub(crate) mod window_placement;
pub(crate) mod window_session;
mod window_tab_model;
mod window_tabs;
#[cfg(all(test, target_os = "windows"))]
pub(crate) use window_tabs::Grouped as WindowGroupsProbe;
