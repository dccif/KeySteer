use super::{AppOverride, Bindings, LabelUi};
use crate::api::window::WindowAction as W;
use crate::api::{Binding, ModeId};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum SplitRatio {
    Fraction(String),
    Decimal(f64),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Window {
    pub enabled: bool,
    pub exit_mode: crate::api::lifecycle::LifecycleAction,
    pub inherits: Vec<String>,
    pub temporary_mode: Option<String>,
    pub temporary_mode_keys: Vec<String>,
    /// Legacy configuration compatibility; no double-tap gesture is scheduled.
    pub double_tap_ms: u64,
    pub number_timeout_ms: u64,
    pub split_ratios: Vec<SplitRatio>,
    pub move_step: f64,
    pub move_speed: f64,
    pub resize_step: f64,
    pub resize_speed: f64,
    pub gap: f64,
    pub layout_keys: String,
    pub border_width: f64,
    pub ui: LabelUi,
    pub bindings: Bindings,
    pub app_configs: Vec<AppOverride>,
}

impl Window {
    pub(crate) fn parsed_split_ratios(&self) -> Result<Vec<f64>, &'static str> {
        const ERROR: &str = "window.split_ratios must be a non-empty array of fractions such as \"1/4\" or decimal numbers, each finite, greater than 0 and less than 1";
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

impl Default for Window {
    fn default() -> Self {
        Self {
            enabled: true,
            exit_mode: crate::api::lifecycle::LifecycleAction::Return,
            inherits: vec!["hotkeys".into()],
            temporary_mode: Some("normal".into()),
            temporary_mode_keys: vec!["primary".into()],
            double_tap_ms: 300,
            number_timeout_ms: 250,
            split_ratios: ["1/4", "1/3", "1/2", "2/3", "3/4"]
                .map(|value| SplitRatio::Fraction(value.into()))
                .to_vec(),
            move_step: 20.0,
            move_speed: 600.0,
            resize_step: 20.0,
            resize_speed: 500.0,
            gap: 8.0,
            layout_keys: "123456789qwe".into(),
            border_width: 3.0,
            ui: LabelUi {
                font_size: 28,
                ..LabelUi::default()
            },
            bindings: [
                ("s", W::Size),
                ("a", W::Layout),
                ("e", W::Edit),
                ("d", W::NextScreen),
                ("f", W::Maximize),
                ("c", W::Center),
                ("tab", W::Select),
                ("z", W::Undo),
                ("x", W::RemoveRegion),
                ("r", W::SavedLayouts),
                ("ctrl+s", W::SaveLayout),
                ("q", W::Cancel),
                ("primary+q", W::Exit),
            ]
            .into_iter()
            .map(|(key, action)| (key.into(), Binding::Window(action)))
            .chain([("alt+w".into(), Binding::Mode(ModeId::window()))])
            .collect(),
            app_configs: Vec::new(),
        }
    }
}
