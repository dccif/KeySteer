use super::{AppOverride, Bindings, LabelUi};
use crate::api::window::WindowAction as W;
use crate::api::{Binding, LifecycleAction, ModeId, TargetingLifecycle};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum SplitRatio {
    Fraction(String),
    Decimal(f64),
}

// Shared schema, not binding inheritance. Each mode owns a separate value.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct WindowModeConfig {
    pub enabled: bool,
    pub inherits: Vec<String>,
    pub temporary_mode: Option<String>,
    pub temporary_mode_keys: Vec<String>,
    pub number_timeout_ms: u64,
    pub border_width: f64,
    pub ui: LabelUi,
    pub lifecycle: TargetingLifecycle,
    pub bindings: Bindings,
    pub app_configs: Vec<AppOverride>,
}
impl Default for WindowModeConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            inherits: vec!["hotkeys".into()],
            temporary_mode: Some("normal".into()),
            temporary_mode_keys: vec!["primary".into()],
            number_timeout_ms: 250,
            border_width: 3.0,
            ui: LabelUi {
                font_size: 28,
                ..Default::default()
            },
            lifecycle: TargetingLifecycle::default(),
            bindings: Bindings::new(),
            app_configs: Vec::new(),
        }
    }
}
macro_rules! window_config {
    ($name:ident { $($field:ident: $ty:ty),* $(,)? }) => {
        #[derive(Debug, Clone, PartialEq, Serialize)]
        pub struct $name {
            #[serde(flatten)]
            pub common: WindowModeConfig,
            $(pub $field: $ty,)*
        }
        // Deserialize a flat document with this mode's own defaults. A nested
        // serde(flatten) default would use WindowModeConfig's empty bindings.
        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                #[derive(Deserialize)]
                #[serde(default, deny_unknown_fields)]
                struct Document {
                    enabled: bool, inherits: Vec<String>, temporary_mode: Option<String>,
                    temporary_mode_keys: Vec<String>, number_timeout_ms: u64, border_width: f64,
                    ui: LabelUi,
                    #[serde(deserialize_with = "read_lifecycle")]
                    lifecycle: TargetingLifecycle, bindings: Bindings,
                    app_configs: Vec<AppOverride>, $($field: $ty,)*
                }
                fn read_lifecycle<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<TargetingLifecycle, D::Error> {
                    #[derive(Deserialize)]
                    #[serde(deny_unknown_fields)]
                    struct Partial { after_finish: Option<LifecycleAction>, after_click: Option<LifecycleAction> }
                    let partial = Partial::deserialize(deserializer)?;
                    let defaults = $name::default().common.lifecycle;
                    Ok(TargetingLifecycle { after_finish: partial.after_finish.unwrap_or(defaults.after_finish), after_click: partial.after_click.unwrap_or(defaults.after_click) })
                }
                impl Default for Document {
                    fn default() -> Self {
                        let mode = $name::default(); let common = mode.common;
                        Self { enabled: common.enabled, inherits: common.inherits, temporary_mode: common.temporary_mode,
                            temporary_mode_keys: common.temporary_mode_keys, number_timeout_ms: common.number_timeout_ms,
                            border_width: common.border_width, ui: common.ui, lifecycle: common.lifecycle,
                            bindings: common.bindings, app_configs: common.app_configs, $($field: mode.$field,)* }
                    }
                }
                let doc = Document::deserialize(deserializer)?;
                Ok(Self { common: WindowModeConfig { enabled: doc.enabled, inherits: doc.inherits,
                    temporary_mode: doc.temporary_mode, temporary_mode_keys: doc.temporary_mode_keys,
                    number_timeout_ms: doc.number_timeout_ms, border_width: doc.border_width, ui: doc.ui,
                    lifecycle: doc.lifecycle, bindings: doc.bindings, app_configs: doc.app_configs }, $($field: doc.$field,)* })
            }
        }
        impl std::ops::Deref for $name { type Target = WindowModeConfig; fn deref(&self) -> &Self::Target { &self.common } }
        impl std::ops::DerefMut for $name { fn deref_mut(&mut self) -> &mut Self::Target { &mut self.common } }
    }
}
window_config!(Window {
    move_step: f64,
    move_speed: f64,
    resize_step: f64,
    resize_speed: f64
});
window_config!(WindowQuick { split_ratios: Vec<SplitRatio>, gap: f64 });
window_config!(WindowEditor {
    resize_step: f64,
    resize_speed: f64,
    gap: f64
});
window_config!(WindowRestore { gap: f64 });
window_config!(WindowDelete {});

fn common(back: ModeId, actions: &[(&str, W)], modes: &[(&str, ModeId)]) -> WindowModeConfig {
    let mut common = WindowModeConfig::default();
    common.bindings.extend(
        actions
            .iter()
            .map(|(key, action)| ((*key).into(), Binding::Window(*action))),
    );
    common.bindings.extend(
        modes
            .iter()
            .cloned()
            .chain([("q", back), ("primary+q", ModeId::idle())])
            .map(|(key, mode)| (key.into(), Binding::Mode(mode))),
    );
    common
}
impl Default for Window {
    fn default() -> Self {
        Self {
            common: common(
                ModeId::idle(),
                &[
                    ("h", W::Left),
                    ("j", W::Down),
                    ("k", W::Up),
                    ("l", W::Right),
                    ("s", W::Size),
                    ("d", W::NextScreen),
                    ("f", W::CycleState),
                    ("c", W::Center),
                    ("tab", W::Select),
                    ("z", W::Undo),
                ],
                &[
                    ("a", ModeId::window_quick()),
                    ("e", ModeId::window_editor()),
                    ("r", ModeId::window_restore()),
                ],
            ),
            move_step: 20.0,
            move_speed: 600.0,
            resize_step: 20.0,
            resize_speed: 500.0,
        }
    }
}
impl Default for WindowQuick {
    fn default() -> Self {
        use crate::api::Direction::*;
        Self {
            common: common(
                ModeId::window(),
                &[
                    ("h", W::Navigate(Left)),
                    ("j", W::Navigate(Down)),
                    ("k", W::Navigate(Up)),
                    ("l", W::Navigate(Right)),
                    ("tab", W::Select),
                    ("z", W::Undo),
                ],
                &[
                    ("e", ModeId::window_editor()),
                    ("r", ModeId::window_restore()),
                ],
            ),
            split_ratios: ["1/4", "1/3", "1/2", "2/3", "3/4"]
                .map(|v| SplitRatio::Fraction(v.into()))
                .to_vec(),
            gap: 8.0,
        }
    }
}
impl Default for WindowEditor {
    fn default() -> Self {
        use crate::api::Direction::*;
        Self {
            common: common(
                ModeId::window(),
                &[
                    ("h", W::Navigate(Left)),
                    ("j", W::Navigate(Down)),
                    ("k", W::Navigate(Up)),
                    ("l", W::Navigate(Right)),
                    ("shift+h", W::Split(Left)),
                    ("shift+j", W::Split(Down)),
                    ("shift+k", W::Split(Up)),
                    ("shift+l", W::Split(Right)),
                    ("ctrl+h", W::Ratio(Left)),
                    ("ctrl+j", W::Ratio(Down)),
                    ("ctrl+k", W::Ratio(Up)),
                    ("ctrl+l", W::Ratio(Right)),
                    ("tab", W::Select),
                    ("z", W::Undo),
                    ("x", W::RemoveRegion),
                    ("ctrl+s", W::SaveLayout),
                ],
                &[("r", ModeId::window_restore())],
            ),
            resize_step: 20.0,
            resize_speed: 500.0,
            gap: 8.0,
        }
    }
}
impl Default for WindowRestore {
    fn default() -> Self {
        let mut common = common(
            ModeId::window(),
            &[("enter", W::Confirm)],
            &[("x", ModeId::window_delete())],
        );
        common.lifecycle.after_finish = LifecycleAction::Mode(ModeId::window_editor());
        Self { common, gap: 8.0 }
    }
}
impl Default for WindowDelete {
    fn default() -> Self {
        Self {
            common: common(ModeId::window_restore(), &[("enter", W::Confirm)], &[]),
        }
    }
}

impl WindowQuick {
    pub(crate) fn parsed_split_ratios(&self) -> Result<Vec<f64>, &'static str> {
        const ERROR: &str = "window_quick.split_ratios must be a non-empty array of fractions such as \"1/4\" or decimal numbers, each finite, greater than 0 and less than 1";
        let mut values = Vec::with_capacity(self.split_ratios.len());
        for ratio in &self.split_ratios {
            let value = match ratio {
                SplitRatio::Decimal(value) => *value,
                SplitRatio::Fraction(text) => {
                    let (numerator, denominator) = text.trim().split_once('/').ok_or(ERROR)?;
                    let numerator = numerator.trim().parse::<u32>().map_err(|_| ERROR)?;
                    let denominator = denominator.trim().parse::<u32>().map_err(|_| ERROR)?;
                    if numerator == 0 || numerator >= denominator {
                        return Err(ERROR);
                    }
                    f64::from(numerator) / f64::from(denominator)
                }
            };
            if !value.is_finite() || value <= 0.0 || value >= 1.0 {
                return Err(ERROR);
            }
            values.push(value);
        }
        if values.is_empty() {
            return Err(ERROR);
        }
        values.sort_unstable_by(f64::total_cmp);
        values.dedup();
        Ok(values)
    }
}
