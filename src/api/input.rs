//! Keys, chords and mode identity.
//!
//! Key names are platform-neutral. `primary` resolves to Cmd on macOS and Ctrl
//! elsewhere, so one configuration file works everywhere, and the aliases match
//! what users already write (`Cmd`, `Super`, `Option`, `Return`, `PageUp`, …).

use std::borrow::Borrow;
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::str::FromStr;
use std::sync::Arc;

use serde::{Deserialize, Deserializer, Serialize, Serializer, de::Error as _};

thread_local! {
    static ACTIVE_KEY_ALIASES: RefCell<Option<BTreeMap<String, String>>> = const { RefCell::new(None) };
}

struct KeyAliasScope(Option<BTreeMap<String, String>>);

impl Drop for KeyAliasScope {
    fn drop(&mut self) {
        ACTIVE_KEY_ALIASES.with(|active| {
            active.replace(self.0.take());
        });
    }
}

pub(crate) fn with_key_aliases<T>(
    aliases: &BTreeMap<String, String>,
    operation: impl FnOnce() -> T,
) -> T {
    let previous = ACTIVE_KEY_ALIASES.with(|active| active.replace(Some(aliases.clone())));
    let _scope = KeyAliasScope(previous);
    operation()
}

/// The modifier `primary` stands for on this platform.
///
/// Cmd on macOS, Ctrl elsewhere. This is resolved at parse time so the rest of
/// the program only ever sees concrete modifiers.
pub const fn primary_modifier() -> &'static str {
    if cfg!(target_os = "macos") {
        "win"
    } else {
        "ctrl"
    }
}

/// A normalized key name (lowercase, `_`-separated).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Key(Arc<str>);

impl Borrow<str> for Key {
    fn borrow(&self) -> &str {
        self.as_str()
    }
}

impl Key {
    pub fn new(value: impl AsRef<str>) -> Result<Self, String> {
        let normalized = normalize_key(value.as_ref());
        if normalized.is_empty() {
            Err("key must not be empty".into())
        } else {
            Ok(Self(normalized.into()))
        }
    }

    pub(crate) fn new_with_aliases(
        value: impl AsRef<str>,
        aliases: &BTreeMap<String, String>,
    ) -> Result<Self, String> {
        with_key_aliases(aliases, || Self::new(value))
    }

    pub fn as_str(&self) -> &str {
        self.0.as_ref()
    }

    pub fn is_modifier(&self) -> bool {
        matches!(
            self.0.as_ref(),
            "alt"
                | "left_alt"
                | "right_alt"
                | "ctrl"
                | "left_ctrl"
                | "right_ctrl"
                | "shift"
                | "left_shift"
                | "right_shift"
                | "win"
                | "left_win"
                | "right_win"
        )
    }

    /// The single character this key types, if it is a character key.
    /// Used by grid/hint modes to build their input buffers.
    pub fn as_char(&self) -> Option<char> {
        let mut chars = self.0.chars();
        match (chars.next(), chars.next()) {
            (Some(c), None) => Some(c),
            _ => None,
        }
    }

    /// Whether `name` denotes a key this program recognises.
    ///
    /// Used when resolving configuration: a value must be a known key before
    /// it is treated as "send this keystroke", so that a typo like `gird` is
    /// reported rather than silently sent as four characters.
    pub fn is_known(name: &str) -> bool {
        let Ok(key) = Self::new(name) else {
            return false;
        };
        let name = key.as_str();
        // Single printable characters are always keys.
        if key.as_char().is_some_and(|c| !c.is_whitespace()) {
            return true;
        }
        if key.is_modifier() {
            return true;
        }
        // Function keys F1-F24 and numpad digits.
        if let Some(digits) = name.strip_prefix('f')
            && !digits.is_empty()
            && digits.chars().all(|c| c.is_ascii_digit())
            && digits.parse::<u32>().is_ok_and(|n| (1..=24).contains(&n))
        {
            return true;
        }
        if let Some(digit) = name.strip_prefix("numpad_")
            && digit.len() == 1
            && digit.chars().all(|c| c.is_ascii_digit())
        {
            return true;
        }
        matches!(
            name,
            "esc"
                | "enter"
                | "tab"
                | "space"
                | "backspace"
                | "delete"
                | "insert"
                | "home"
                | "end"
                | "page_up"
                | "page_down"
                | "arrow_up"
                | "arrow_down"
                | "arrow_left"
                | "arrow_right"
                | "caps_lock"
                | "num_lock"
                | "scroll_lock"
                | "print_screen"
                | "pause"
                | "menu"
                | "add"
                | "subtract"
                | "multiply"
                | "divide"
                | "decimal"
                | "fn"
        )
    }
}

fn normalize_key(value: &str) -> String {
    let alias = normalize_alias_name(value);
    if let Some(resolved) = ACTIVE_KEY_ALIASES.with(|active| {
        active
            .borrow()
            .as_ref()
            .and_then(|aliases| aliases.get(&alias).cloned())
    }) {
        return resolved;
    }
    normalize_builtin_key(value)
}

pub(crate) fn normalize_alias_name(value: &str) -> String {
    let trimmed = value.trim();
    if trimmed.chars().count() == 1 {
        trimmed.to_lowercase()
    } else {
        trimmed.to_lowercase().replace([' ', '-'], "_")
    }
}

pub(crate) fn normalize_builtin_key(value: &str) -> String {
    let trimmed = value.trim();
    // A single character is a literal key: `-` is the minus key, not a
    // separator, so it must not be rewritten below.
    if trimmed.chars().count() == 1 {
        return trimmed.to_lowercase();
    }
    let value = trimmed.to_lowercase().replace([' ', '-'], "_");
    match value.as_str() {
        // Platform-neutral modifier: the whole point of `primary`.
        "primary" | "mod" => primary_modifier().into(),
        "control" => "ctrl".into(),
        "command" | "cmd" | "meta" | "super" => "win".into(),
        "option" | "opt" => "alt".into(),
        "leftcontrol" | "leftcontrolkey" | "lctrl" | "left_control" => "left_ctrl".into(),
        "rightcontrol" | "rightcontrolkey" | "rctrl" | "right_control" => "right_ctrl".into(),
        "leftalt" | "lalt" | "left_option" | "left_opt" => "left_alt".into(),
        "rightalt" | "ralt" | "right_option" | "right_opt" => "right_alt".into(),
        "leftshift" | "lshift" => "left_shift".into(),
        "rightshift" | "rshift" => "right_shift".into(),
        "leftwin" | "lwin" | "left_command" | "left_cmd" | "left_super" | "left_meta" => {
            "left_win".into()
        }
        "rightwin" | "rwin" | "right_command" | "right_cmd" | "right_super" | "right_meta" => {
            "right_win".into()
        }
        "escape" => "esc".into(),
        "return" => "enter".into(),
        "del" => "delete".into(),
        "up" => "arrow_up".into(),
        "down" => "arrow_down".into(),
        "left" => "arrow_left".into(),
        "right" => "arrow_right".into(),
        "pageup" | "pgup" => "page_up".into(),
        "pagedown" | "pgdn" | "pgdown" => "page_down".into(),
        "capslock" => "caps_lock".into(),
        "numlock" => "num_lock".into(),
        "scrolllock" => "scroll_lock".into(),
        "printscreen" | "prtsc" => "print_screen".into(),
        "semicolon" => ";".into(),
        "apostrophe" | "quote" => "'".into(),
        "comma" => ",".into(),
        "period" | "dot" => ".".into(),
        "slash" => "/".into(),
        "backslash" => "\\".into(),
        "grave" | "backtick" => "`".into(),
        "minus" | "hyphen" => "-".into(),
        "equal" | "equals" => "=".into(),
        "leftbracket" | "left_bracket" => "[".into(),
        "rightbracket" | "right_bracket" => "]".into(),
        other => other.into(),
    }
}

impl fmt::Display for Key {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl FromStr for Key {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::new(s)
    }
}

impl Serialize for Key {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for Key {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Self::new(String::deserialize(deserializer)?).map_err(D::Error::custom)
    }
}

/// A set of keys that must be held simultaneously, e.g. `left_alt+g`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct KeyChord {
    keys: Vec<Key>,
    activation: Key,
}

impl KeyChord {
    pub fn parse(value: &str) -> Result<Self, String> {
        let keys = value
            .split('+')
            .map(Key::new)
            .collect::<Result<Vec<_>, _>>()?;
        if keys.is_empty() {
            return Err("chord must contain a key".into());
        }
        if keys.iter().collect::<BTreeSet<_>>().len() != keys.len() {
            return Err(format!("chord contains a duplicate key: {value}"));
        }
        if keys.len() > 1 && keys.iter().all(Key::is_modifier) {
            return Err(format!("chord must contain a non-modifier key: {value}"));
        }
        let activation = keys
            .iter()
            .rev()
            .find(|key| !key.is_modifier())
            .or_else(|| keys.last())
            .cloned()
            .ok_or_else(|| "chord must contain a key".to_string())?;
        Ok(Self { keys, activation })
    }

    pub(crate) fn parse_with_aliases(
        value: &str,
        aliases: &BTreeMap<String, String>,
    ) -> Result<Self, String> {
        with_key_aliases(aliases, || Self::parse(value))
    }

    pub fn keys(&self) -> &[Key] {
        &self.keys
    }

    /// The non-modifier key whose press completes the chord.
    pub fn activation_key(&self) -> &Key {
        &self.activation
    }

    /// Whether `key` is the physical key that completes this chord. Generic
    /// modifier bindings such as `shift` match either physical Shift key.
    pub fn activation_matches(&self, key: &Key) -> bool {
        modifier_equivalent_is_pressed(self.activation_key(), std::slice::from_ref(key))
    }

    /// Stable text form, used for comparison and for round-tripping through
    /// configuration.
    ///
    /// Modifiers come first in the conventional `ctrl alt shift win` order
    /// rather than alphabetically, so `send ctrl+alt+delete` survives a
    /// round trip and reads the way users write it.
    pub fn canonical(&self) -> String {
        fn modifier_rank(key: &Key) -> usize {
            match key.as_str() {
                "ctrl" | "left_ctrl" | "right_ctrl" => 0,
                "alt" | "left_alt" | "right_alt" => 1,
                "shift" | "left_shift" | "right_shift" => 2,
                "win" | "left_win" | "right_win" => 3,
                _ => 4,
            }
        }

        let (mut modifiers, mut keys): (Vec<_>, Vec<_>) =
            self.keys.iter().partition(|key| key.is_modifier());
        modifiers.sort_by(|a, b| modifier_rank(a).cmp(&modifier_rank(b)).then(a.cmp(b)));
        keys.sort_unstable();
        modifiers
            .into_iter()
            .chain(keys)
            .map(Key::as_str)
            .collect::<Vec<_>>()
            .join("+")
    }

    /// True when every key of the chord is currently held. Side-agnostic
    /// modifiers (`alt`) match either physical side.
    pub fn matches_pressed(&self, pressed: &[Key]) -> bool {
        self.keys
            .iter()
            .all(|key| modifier_equivalent_is_pressed(key, pressed))
    }
}

impl fmt::Display for KeyChord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.canonical())
    }
}

fn modifier_equivalent_is_pressed(key: &Key, pressed: &[Key]) -> bool {
    if pressed.contains(key) {
        return true;
    }
    let sides: [&str; 2] = match key.as_str() {
        "alt" => ["left_alt", "right_alt"],
        "ctrl" => ["left_ctrl", "right_ctrl"],
        "shift" => ["left_shift", "right_shift"],
        "win" => ["left_win", "right_win"],
        _ => return false,
    };
    pressed.iter().any(|k| sides.contains(&k.as_str()))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyState {
    Down,
    Up,
}

/// A keyboard event observed (or injected) by the platform layer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InputEvent {
    pub key: Key,
    pub state: KeyState,
    /// Auto-repeat from holding the key.
    pub repeat: bool,
    /// Synthesized by this process — must never be re-processed.
    pub injected: bool,
    pub timestamp_millis: u64,
}

/// Identifier of a mode. Built-in and plugin modes share this namespace,
/// so a plugin may register `my-plugin:zoom` and be activated like any
/// built-in mode.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ModeId(ModeName);

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
enum ModeName {
    Idle,
    Normal,
    Grid,
    RecursiveGrid,
    UiHint,
    Shared(Arc<str>),
}

impl ModeId {
    pub fn new(value: impl Into<String>) -> Result<Self, String> {
        Self::parse_borrowed(&value.into())
    }

    /// Parse a borrowed id without allocating for built-in modes.
    pub fn parse_borrowed(value: &str) -> Result<Self, String> {
        let valid = !value.is_empty()
            && value
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | ':' | '/'));
        if valid {
            let name = match value {
                "idle" => ModeName::Idle,
                "normal" => ModeName::Normal,
                "grid" => ModeName::Grid,
                "recursive_grid" => ModeName::RecursiveGrid,
                "ui_hint" => ModeName::UiHint,
                _ => ModeName::Shared(Arc::from(value)),
            };
            Ok(Self(name))
        } else {
            Err(format!("invalid mode id: {value:?}"))
        }
    }

    /// The ids of the five built-in modes.
    pub const BUILT_IN: [&'static str; 5] = ["idle", "normal", "grid", "recursive_grid", "ui_hint"];

    /// Whether `name` could name a mode in configuration.
    ///
    /// A bare word must be one of the built-ins; anything else has to be
    /// namespaced (`my-plugin:zoom`). This is what makes a typo like `gird` an
    /// error instead of a binding to a mode that will never exist.
    pub fn is_plausible(name: &str) -> bool {
        Self::BUILT_IN.contains(&name) || name.contains(':')
    }

    pub fn idle() -> Self {
        Self(ModeName::Idle)
    }
    pub fn normal() -> Self {
        Self(ModeName::Normal)
    }
    pub fn grid() -> Self {
        Self(ModeName::Grid)
    }
    pub fn recursive_grid() -> Self {
        Self(ModeName::RecursiveGrid)
    }
    pub fn ui_hint() -> Self {
        Self(ModeName::UiHint)
    }

    pub fn as_str(&self) -> &str {
        match &self.0 {
            ModeName::Idle => "idle",
            ModeName::Normal => "normal",
            ModeName::Grid => "grid",
            ModeName::RecursiveGrid => "recursive_grid",
            ModeName::UiHint => "ui_hint",
            ModeName::Shared(name) => name,
        }
    }
}

impl fmt::Display for ModeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for ModeId {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::new(s)
    }
}

impl Serialize for ModeId {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for ModeId {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Self::new(String::deserialize(deserializer)?).map_err(D::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generic_modifier_activation_matches_either_physical_side() {
        let shift = KeyChord::parse("shift").unwrap();
        assert!(shift.activation_matches(&Key::new("left_shift").unwrap()));
        assert!(shift.activation_matches(&Key::new("right_shift").unwrap()));
        assert!(!shift.activation_matches(&Key::new("left_ctrl").unwrap()));
    }

    #[test]
    fn normalizes_chords_and_finds_activation_key() {
        let chord = KeyChord::parse("LeftAlt + H").unwrap();
        assert_eq!(chord.canonical(), "left_alt+h");
        assert_eq!(chord.activation_key().as_str(), "h");
    }

    #[test]
    fn side_agnostic_modifier_matches_either_side() {
        let chord = KeyChord::parse("alt+g").unwrap();
        let pressed = [Key::new("right_alt").unwrap(), Key::new("g").unwrap()];
        assert!(chord.matches_pressed(&pressed));
    }

    #[test]
    fn explicit_modifier_side_does_not_match_the_other_side() {
        for family in ["alt", "ctrl", "shift", "win"] {
            let chord = KeyChord::parse(&format!("left_{family}+g")).unwrap();
            let right = [
                Key::new(format!("right_{family}")).unwrap(),
                Key::new("g").unwrap(),
            ];
            let left = [
                Key::new(format!("left_{family}")).unwrap(),
                Key::new("g").unwrap(),
            ];
            assert!(!chord.matches_pressed(&right), "{family}");
            assert!(chord.matches_pressed(&left), "{family}");
        }
    }

    #[test]
    fn generic_modifier_families_match_both_physical_sides() {
        for family in ["alt", "ctrl", "shift", "win"] {
            let chord = KeyChord::parse(&format!("{family}+g")).unwrap();
            for side in ["left", "right"] {
                let pressed = [
                    Key::new(format!("{side}_{family}")).unwrap(),
                    Key::new("g").unwrap(),
                ];
                assert!(chord.matches_pressed(&pressed), "{side}_{family}");
            }
        }
    }

    #[test]
    fn rejects_modifier_only_chords() {
        assert!(KeyChord::parse("ctrl+shift").is_err());
    }

    #[test]
    fn punctuation_keys_survive_normalization() {
        // `-` is a separator inside a name but a literal key on its own.
        for symbol in ["-", "=", "[", "]", ";", "'", "\\", ",", ".", "/", "`"] {
            assert_eq!(Key::new(symbol).unwrap().as_str(), symbol, "{symbol:?}");
        }
        assert_eq!(Key::new("left-alt").unwrap().as_str(), "left_alt");
    }

    #[test]
    fn single_character_keys_expose_their_char() {
        assert_eq!(Key::new("g").unwrap().as_char(), Some('g'));
        assert_eq!(Key::new("-").unwrap().as_char(), Some('-'));
        assert_eq!(Key::new("esc").unwrap().as_char(), None);
    }

    #[test]
    fn is_known_accepts_real_keys_and_rejects_typos() {
        for name in [
            "a",
            "5",
            "-",
            "esc",
            "enter",
            "home",
            "page_up",
            "f1",
            "f12",
            "f24",
            "numpad_7",
            "left_alt",
            "arrow_left",
        ] {
            assert!(Key::is_known(name), "{name:?} should be a known key");
        }
        // These must not be mistaken for keystrokes, or a typo in the config
        // would silently type text instead of reporting an error.
        for name in ["gird", "move_left", "grid", "ui_hint", "scrol_up"] {
            assert!(!Key::is_known(name), "{name:?} should not be a known key");
        }
        // F25 does not exist on any platform we target.
        assert!(!Key::is_known("f25"));
        assert!(!Key::is_known("f0"));
    }

    #[test]
    fn primary_resolves_to_the_platform_modifier() {
        let key = Key::new("primary").unwrap();
        if cfg!(target_os = "macos") {
            assert_eq!(key.as_str(), "win", "primary should be Cmd on macOS");
        } else {
            assert_eq!(key.as_str(), "ctrl", "primary should be Ctrl elsewhere");
        }
        // It must be a modifier, or chord validation would reject `primary+g`.
        assert!(key.is_modifier());
    }

    #[test]
    fn a_primary_chord_parses_and_keeps_its_activation_key() {
        let chord = KeyChord::parse("Primary+Shift+G").unwrap();
        assert_eq!(chord.activation_key().as_str(), "g");
        assert_eq!(chord.keys().len(), 3);
    }

    #[test]
    fn documented_modifier_aliases_all_resolve() {
        // The alias table users are told to expect.
        for (alias, expected) in [
            ("Cmd", "win"),
            ("Command", "win"),
            ("Super", "win"),
            ("Meta", "win"),
            ("Ctrl", "ctrl"),
            ("Control", "ctrl"),
            ("Alt", "alt"),
            ("Option", "alt"),
            ("Shift", "shift"),
        ] {
            assert_eq!(
                Key::new(alias).unwrap().as_str(),
                expected,
                "alias {alias:?}"
            );
        }
    }

    #[test]
    fn documented_named_keys_all_resolve() {
        for (name, expected) in [
            ("Space", "space"),
            ("Return", "enter"),
            ("Enter", "enter"),
            ("Escape", "esc"),
            ("Tab", "tab"),
            ("Delete", "delete"),
            ("Backspace", "backspace"),
            ("Up", "arrow_up"),
            ("Down", "arrow_down"),
            ("Left", "arrow_left"),
            ("Right", "arrow_right"),
            ("Home", "home"),
            ("End", "end"),
            ("PageUp", "page_up"),
            ("PageDown", "page_down"),
        ] {
            let key = Key::new(name).unwrap();
            assert_eq!(key.as_str(), expected, "named key {name:?}");
            assert!(Key::is_known(name), "{name:?} should be known");
        }
    }

    #[test]
    fn documented_symbol_keys_all_resolve() {
        for symbol in ["`", "-", "=", "[", "]", "\\", ";", "'", ",", ".", "/"] {
            assert!(Key::is_known(symbol), "symbol {symbol:?} should be known");
        }
        // Spelled-out forms map onto the same keys.
        for (name, expected) in [
            ("minus", "-"),
            ("equal", "="),
            ("left_bracket", "["),
            ("right_bracket", "]"),
            ("backslash", "\\"),
            ("semicolon", ";"),
            ("quote", "'"),
            ("comma", ","),
            ("period", "."),
            ("slash", "/"),
            ("grave", "`"),
        ] {
            assert_eq!(Key::new(name).unwrap().as_str(), expected, "{name:?}");
        }
    }
}
