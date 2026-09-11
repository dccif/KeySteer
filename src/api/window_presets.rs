//! Durable region geometry: native window identities are never persisted.
use super::window::{WindowId, WindowInfo};
use super::window_layout::{Axis, LayoutNode, LayoutTree};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

pub const MAX_PRESETS: usize = 99;
pub const MAX_NOTE_CHARS: usize = 80;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum RegionTemplate {
    Slot {
        id: u32,
    },
    Split {
        axis: Axis,
        ratio: f64,
        first: Box<Self>,
        second: Box<Self>,
    },
}
impl RegionTemplate {
    pub fn from_tree(tree: &LayoutTree) -> Self {
        fn copy(node: &LayoutNode) -> RegionTemplate {
            match node {
                LayoutNode::Slot { id, .. } => RegionTemplate::Slot { id: *id },
                LayoutNode::Split {
                    axis,
                    ratio,
                    first,
                    second,
                } => RegionTemplate::Split {
                    axis: *axis,
                    ratio: *ratio,
                    first: Box::new(copy(first)),
                    second: Box::new(copy(second)),
                },
            }
        }
        copy(&tree.root)
    }
    pub fn validate(&self) -> Result<usize, String> {
        fn visit(
            node: &RegionTemplate,
            depth: usize,
            ids: &mut BTreeSet<u32>,
        ) -> Result<(), String> {
            if depth > 32 {
                return Err("Saved layout is too deeply nested".into());
            }
            match node {
                RegionTemplate::Slot { id } => {
                    if *id == 0 || *id == u32::MAX || !ids.insert(*id) || ids.len() > 256 {
                        return Err("Saved layout has invalid region numbers".into());
                    }
                }
                RegionTemplate::Split {
                    ratio,
                    first,
                    second,
                    ..
                } => {
                    if !ratio.is_finite() || *ratio <= 0.0 || *ratio >= 1.0 {
                        return Err("Saved layout has an invalid divider".into());
                    }
                    visit(first, depth + 1, ids)?;
                    visit(second, depth + 1, ids)?;
                }
            }
            Ok(())
        }
        let mut ids = BTreeSet::new();
        visit(self, 0, &mut ids)?;
        Ok(ids.len())
    }
    /// Caller supplies windows in most-recently-used order. Extra windows are
    /// deliberately absent from the resulting layout's placement requests.
    pub fn instantiate(&self, windows: &[WindowInfo]) -> Result<LayoutTree, String> {
        self.validate()?;
        fn copy(node: &RegionTemplate) -> LayoutNode {
            match node {
                RegionTemplate::Slot { id } => LayoutNode::Slot {
                    id: *id,
                    window: None,
                },
                RegionTemplate::Split {
                    axis,
                    ratio,
                    first,
                    second,
                } => LayoutNode::Split {
                    axis: *axis,
                    ratio: *ratio,
                    first: Box::new(copy(first)),
                    second: Box::new(copy(second)),
                },
            }
        }
        let mut tree = LayoutTree::from_saved_root(copy(self));
        let mut slots = tree.slots();
        slots.sort_by_key(|s| s.id);
        let mut used = BTreeSet::<WindowId>::new();
        for (slot, window) in slots.iter().zip(
            windows
                .iter()
                .filter(|w| w.resizable && !w.fullscreen && used.insert(w.id)),
        ) {
            tree.move_window(window.id, slot.id);
        }
        tree.selected = slots[0].id;
        Ok(tree)
    }
}

/// A typed workspace item. Native identities never cross this persistence boundary.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    content = "data",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum WindowTemplate {
    Layout(RegionTemplate),
    Tabs(TabTemplate),
}
impl WindowTemplate {
    pub fn kind_name(&self) -> &'static str {
        match self {
            Self::Layout(_) => "Layout",
            Self::Tabs(_) => "Tabs",
        }
    }
    pub fn validate(&self, window_count: usize) -> Result<(), String> {
        match self {
            Self::Layout(regions) if window_count > regions.validate()? => {
                Err("Saved preset has an invalid window count".into())
            }
            Self::Layout(_) => Ok(()),
            Self::Tabs(tabs) => tabs.validate(window_count),
        }
    }
}
impl From<RegionTemplate> for WindowTemplate {
    fn from(value: RegionTemplate) -> Self {
        Self::Layout(value)
    }
}
impl From<TabTemplate> for WindowTemplate {
    fn from(value: TabTemplate) -> Self {
        Self::Tabs(value)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SavedPreset {
    pub id: u32,
    #[serde(default)]
    pub note: String,
    pub window_count: usize,
    pub template: WindowTemplate,
}
impl SavedPreset {
    pub fn name(&self) -> String {
        let note = self.note.trim();
        if note.is_empty() {
            format!("{} {}", self.template.kind_name(), self.id)
        } else {
            note.to_string()
        }
    }
    pub fn instantiate_layout(&self, windows: &[WindowInfo]) -> Result<LayoutTree, String> {
        match &self.template {
            WindowTemplate::Layout(regions) => regions.instantiate(windows),
            WindowTemplate::Tabs(_) => Err("Select windows in order to restore Tabs".into()),
        }
    }
    pub fn validate(&self) -> Result<(), String> {
        if self.id == 0
            || self.id as usize > MAX_PRESETS
            || self.note.chars().count() > MAX_NOTE_CHARS
            || self.note.chars().any(char::is_control)
        {
            return Err("Saved preset has invalid metadata".into());
        }
        self.template.validate(self.window_count)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TabTemplate {
    pub region: crate::api::Rect,
    /// Position in the manually selected, ordered member list, never an identity.
    pub active: usize,
}
impl TabTemplate {
    pub fn validate(&self, count: usize) -> Result<(), String> {
        let r = self.region;
        if !(2..=256).contains(&count)
            || self.active >= count
            || ![r.x, r.y, r.width, r.height].iter().all(|v| v.is_finite())
            || r.x < 0.0
            || r.y < 0.0
            || r.width <= 0.0
            || r.height <= 0.0
            || r.right() > 1.0 + 1e-9
            || r.bottom() > 1.0 + 1e-9
        {
            return Err("Tab template has invalid geometry, member count or active tab".into());
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct PresetLibraryRequest {
    pub session: u64,
    pub operation: PresetLibraryOperation,
}
#[derive(Clone, Debug, PartialEq)]
pub enum PresetLibraryOperation {
    List,
    Delete {
        expected: SavedPreset,
    },
    Save {
        template: WindowTemplate,
        window_count: usize,
    },
}
#[derive(Clone, Debug, PartialEq)]
pub struct PresetLibraryResult {
    pub session: u64,
    pub presets: Vec<SavedPreset>,
    pub saved: Option<u32>,
    pub message: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TextPrompt {
    /// Inline input bar in desktop coordinates, supplied by the host.
    pub bounds: crate::api::Rect,
    pub id: u64,
    pub title: String,
    pub message: String,
    pub placeholder: String,
    pub max_chars: usize,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::Rect;
    fn regions(count: u32) -> RegionTemplate {
        fn build(start: u32, end: u32) -> RegionTemplate {
            if start == end {
                return RegionTemplate::Slot { id: start };
            }
            let mid = (start + end) / 2;
            RegionTemplate::Split {
                axis: Axis::X,
                ratio: 0.5,
                first: Box::new(build(start, mid)),
                second: Box::new(build(mid + 1, end)),
            }
        }
        build(1, count)
    }
    fn windows(count: u64) -> Vec<WindowInfo> {
        (1..=count)
            .rev()
            .map(|id| WindowInfo {
                id: WindowId(id),
                title: "private title".into(),
                app: "private app".into(),
                bounds: Rect::new(0.0, 0.0, 400.0, 300.0),
                screen: 0,
                resizable: true,
                maximized: false,
                minimized: false,
                fullscreen: false,
            })
            .collect()
    }
    #[test]
    fn restoring_more_regions_keeps_empty_slots_and_less_regions_leaves_extra_windows_out() {
        let tree = regions(9).instantiate(&windows(4)).unwrap();
        let slots = tree.slots();
        assert_eq!(slots.len(), 9);
        assert_eq!(
            slots.iter().map(|s| s.window).collect::<Vec<_>>(),
            vec![
                Some(WindowId(4)),
                Some(WindowId(3)),
                Some(WindowId(2)),
                Some(WindowId(1)),
                None,
                None,
                None,
                None,
                None
            ]
        );
        let tree = regions(4).instantiate(&windows(9)).unwrap();
        assert_eq!(
            tree.slots()
                .iter()
                .filter_map(|s| s.window)
                .collect::<Vec<_>>(),
            vec![WindowId(9), WindowId(8), WindowId(7), WindowId(6)]
        );
    }
    #[test]
    fn save_names_and_round_trip_contain_no_window_identity() {
        let tree = regions(9).instantiate(&windows(4)).unwrap();
        let mut layout = SavedPreset {
            id: 1,
            note: String::new(),
            window_count: 4,
            template: WindowTemplate::Layout(RegionTemplate::from_tree(&tree)),
        };
        assert_eq!(layout.name(), "Layout 1");
        layout.window_count = 9;
        assert_eq!(layout.name(), "Layout 1");
        layout.note = "写代码 🦀".into();
        assert_eq!(layout.name(), "写代码 🦀");
        let json = serde_json::to_string(&layout).unwrap();
        assert!(
            !json.contains("private")
                && !json.contains("title")
                && !json.contains("app")
                && !json.contains("\"window\":")
        );
        assert_eq!(serde_json::from_str::<SavedPreset>(&json).unwrap(), layout);
    }
    #[test]
    fn restore_filters_fixed_fullscreen_and_duplicate_windows() {
        let mut windows = windows(5);
        windows[0].resizable = false;
        windows[1].fullscreen = true;
        windows.push(windows[2].clone());
        assert_eq!(
            regions(9)
                .instantiate(&windows)
                .unwrap()
                .slots()
                .iter()
                .filter_map(|s| s.window)
                .collect::<Vec<_>>(),
            vec![WindowId(3), WindowId(2), WindowId(1)]
        );
        assert!(
            regions(4)
                .instantiate(&[])
                .unwrap()
                .slots()
                .iter()
                .all(|s| s.window.is_none())
        );
    }
    #[test]
    fn corrupt_geometry_and_metadata_are_rejected() {
        let invalid = RegionTemplate::Split {
            axis: Axis::Y,
            ratio: f64::NAN,
            first: Box::new(regions(1)),
            second: Box::new(regions(1)),
        };
        assert!(invalid.validate().is_err());
        let mut invalid = invalid;
        if let RegionTemplate::Split { ratio, .. } = &mut invalid {
            *ratio = 0.5;
        }
        assert!(invalid.validate().is_err());
        assert!(
            SavedPreset {
                id: 1,
                note: "bad\nnote".into(),
                window_count: 1,
                template: WindowTemplate::Layout(regions(1))
            }
            .validate()
            .is_err()
        );
        assert!(
            SavedPreset {
                id: 1,
                note: String::new(),
                window_count: 2,
                template: WindowTemplate::Layout(regions(1))
            }
            .validate()
            .is_err()
        );
    }
}
