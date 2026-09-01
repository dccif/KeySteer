//! Platform-independent targeting lifecycle policy.

use serde::{Deserialize, Deserializer, Serialize, Serializer, de::Error as _};

use super::{ButtonAction, ModeId, MouseButton};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LifecycleAction {
    Keep,
    Finish,
    Restart,
    Return,
    Mode(ModeId),
    Click {
        button: MouseButton,
        action: ButtonAction,
    },
}

impl LifecycleAction {
    pub fn parse(text: &str) -> Result<Self, String> {
        Ok(match text.trim() {
            "keep" => Self::Keep,
            "finish" => Self::Finish,
            "restart" => Self::Restart,
            "return" => Self::Return,
            "left_click" => Self::Click {
                button: MouseButton::Left,
                action: ButtonAction::Click,
            },
            "right_click" => Self::Click {
                button: MouseButton::Right,
                action: ButtonAction::Click,
            },
            "middle_click" => Self::Click {
                button: MouseButton::Middle,
                action: ButtonAction::Click,
            },
            "double_click" => Self::Click {
                button: MouseButton::Left,
                action: ButtonAction::DoubleClick,
            },
            mode => Self::Mode(ModeId::new(mode)?),
        })
    }

    pub fn canonical(&self) -> &str {
        match self {
            Self::Keep => "keep",
            Self::Finish => "finish",
            Self::Restart => "restart",
            Self::Return => "return",
            Self::Mode(mode) => mode.as_str(),
            Self::Click {
                button: MouseButton::Left,
                action: ButtonAction::Click,
            } => "left_click",
            Self::Click {
                button: MouseButton::Right,
                action: ButtonAction::Click,
            } => "right_click",
            Self::Click {
                button: MouseButton::Middle,
                action: ButtonAction::Click,
            } => "middle_click",
            Self::Click {
                button: MouseButton::Left,
                action: ButtonAction::DoubleClick,
            } => "double_click",
            Self::Click { .. } => "unsupported_click",
        }
    }
}

impl Serialize for LifecycleAction {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.canonical())
    }
}

impl<'de> Deserialize<'de> for LifecycleAction {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let text = String::deserialize(deserializer)?;
        Self::parse(&text).map_err(D::Error::custom)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct TargetingLifecycle {
    pub after_finish: LifecycleAction,
    pub after_click: LifecycleAction,
}

impl Default for TargetingLifecycle {
    fn default() -> Self {
        Self {
            after_finish: LifecycleAction::Keep,
            after_click: LifecycleAction::Keep,
        }
    }
}
