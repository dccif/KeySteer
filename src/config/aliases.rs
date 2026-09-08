//! Key alias compilation, binding normalization, and platform chord warnings.

use super::*;

/// User-defined key names shared by every platform, with optional
/// platform-specific overrides.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeyAliases {
    #[serde(default)]
    #[serde(flatten)]
    pub all: BTreeMap<String, String>,
    #[serde(default)]
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub windows: BTreeMap<String, String>,
    #[serde(default)]
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub macos: BTreeMap<String, String>,
    #[serde(default)]
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub linux: BTreeMap<String, String>,
}

impl Default for KeyAliases {
    fn default() -> Self {
        Self {
            all: BTreeMap::new(),
            windows: BTreeMap::from([("Primary".into(), "left_alt".into())]),
            macos: BTreeMap::new(),
            linux: BTreeMap::new(),
        }
    }
}

impl KeyAliases {
    pub(super) fn effective(&self) -> Result<BTreeMap<String, String>, ConfigError> {
        let mut effective = normalized_alias_scope("[key_aliases]", &self.all)?;
        let (label, platform) = if cfg!(target_os = "windows") {
            ("[key_aliases.windows]", &self.windows)
        } else if cfg!(target_os = "macos") {
            ("[key_aliases.macos]", &self.macos)
        } else if cfg!(target_os = "linux") {
            ("[key_aliases.linux]", &self.linux)
        } else {
            ("[key_aliases]", &self.all)
        };
        if !std::ptr::eq(platform, &self.all) {
            effective.extend(normalized_alias_scope(label, platform)?);
        }
        Ok(effective)
    }
}

/// Flag chords that will not behave as the user expects on this platform.
///
/// These are warnings, not errors: the user may genuinely want the binding, or
/// may have a layout where the problem does not apply. They exist because the
/// failure mode is otherwise baffling — the key appears bound and simply does
/// nothing, or types a character instead.
pub(super) fn platform_warning(chord: &KeyChord) -> Option<String> {
    let keys: Vec<&str> = chord.keys().iter().map(|k| k.as_str()).collect();
    let has = |name: &str| keys.contains(&name);
    let has_any = |names: &[&str]| names.iter().any(|n| has(n));

    let alt = has_any(&["alt", "left_alt", "right_alt"]);
    let ctrl = has_any(&["ctrl", "left_ctrl", "right_ctrl"]);
    let shift = has_any(&["shift", "left_shift", "right_shift"]);
    let cmd = has_any(&["win", "left_win", "right_win"]);
    let activation = chord.activation_key().as_str();
    let is_letter = activation.len() == 1
        && activation
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic());

    // macOS: Option+<letter> is a dead key or special character, so the OS
    // consumes it to compose text. This is why `alt+e` never fires.
    // Window's intentional Option+W launcher is matched by physical key code
    // before forwarding. Keep the existing advisory for other Option letters.
    if cfg!(target_os = "macos") && alt && !cmd && !ctrl && is_letter && activation != "w" {
        return Some(format!(
            "on macOS, Option+{} types a special character (Option+E is a \
             dead-key accent) and may not reach this program. Prefer \
             `primary+shift+{}`, which is Cmd+Shift here and Ctrl+Shift elsewhere",
            activation.to_uppercase(),
            activation
        ));
    }

    // macOS reserves Cmd+Space and Cmd+Tab system-wide. Cmd+Q is intentionally
    // allowed for grid-like mode exit bindings, which the event tap consumes.
    if cfg!(target_os = "macos") && cmd && !shift && !alt && matches!(activation, "space" | "tab") {
        return Some(format!(
            "on macOS, Cmd+{activation} is reserved by the system \
             (Spotlight / app switcher / quit) and will not reach this program"
        ));
    }

    // Linux/Windows terminals own Ctrl+Shift+C and Ctrl+Shift+V for clipboard.
    if !cfg!(target_os = "macos") && ctrl && shift && matches!(activation, "c" | "v") {
        return Some(format!(
            "Ctrl+Shift+{} is the clipboard shortcut in most terminals and \
             will be swallowed there",
            activation.to_uppercase()
        ));
    }

    // F21-F24 do not exist on Apple keyboards.
    if cfg!(target_os = "macos")
        && let Some(digits) = activation.strip_prefix('f')
        && let Ok(n) = digits.parse::<u32>()
        && (21..=24).contains(&n)
    {
        return Some(format!(
            "F{n} does not exist on macOS; use F1-F20 for a portable binding"
        ));
    }

    None
}

pub(super) fn normalized_alias_scope(
    label: &str,
    configured: &BTreeMap<String, String>,
) -> Result<BTreeMap<String, String>, ConfigError> {
    let mut normalized = BTreeMap::new();
    for (name, target) in configured {
        let name = normalize_alias_name(name);
        if name.is_empty() || name.contains('+') {
            return Err(ConfigError::Invalid(format!(
                "{label} alias {name:?} must name one key"
            )));
        }
        if normalized.insert(name.clone(), target.clone()).is_some() {
            return Err(ConfigError::Invalid(format!(
                "{label} alias {name:?} is defined more than once after normalization"
            )));
        }
    }
    Ok(normalized)
}

pub(super) fn compile_key_aliases(
    configured: &BTreeMap<String, String>,
) -> Result<BTreeMap<String, String>, ConfigError> {
    let raw = normalized_alias_scope("[key_aliases]", configured)?;

    fn resolve(
        name: &str,
        raw: &BTreeMap<String, String>,
        resolved: &mut BTreeMap<String, String>,
        visiting: &mut BTreeSet<String>,
    ) -> Result<String, ConfigError> {
        if let Some(target) = resolved.get(name) {
            return Ok(target.clone());
        }
        if !visiting.insert(name.to_string()) {
            return Err(ConfigError::Invalid(format!(
                "key alias cycle contains {name:?}"
            )));
        }
        let Some(target) = raw.get(name) else {
            return Err(ConfigError::Invalid(format!(
                "key alias {name:?} disappeared while resolving aliases"
            )));
        };
        if target.contains('+') {
            return Err(ConfigError::Invalid(format!(
                "key alias {name:?} must resolve to one key, not chord {target:?}"
            )));
        }
        let target_name = normalize_alias_name(target);
        let canonical = if raw.contains_key(&target_name) {
            resolve(&target_name, raw, resolved, visiting)?
        } else {
            normalize_builtin_key(target)
        };
        visiting.remove(name);
        if !Key::is_known(&canonical) {
            return Err(ConfigError::Invalid(format!(
                "key alias {name:?} resolves to unknown key {target:?}"
            )));
        }
        resolved.insert(name.to_string(), canonical.clone());
        Ok(canonical)
    }

    let mut resolved = BTreeMap::new();
    for name in raw.keys() {
        resolve(name, &raw, &mut resolved, &mut BTreeSet::new())?;
    }
    Ok(resolved)
}

pub(super) fn normalize_binding_keys(
    table: &mut Bindings,
    label: &str,
    aliases: &BTreeMap<String, String>,
) -> Result<(), ConfigError> {
    let source = std::mem::take(table);
    for (text, binding) in source {
        let names: Vec<&str> = text.split_ascii_whitespace().collect();
        let names = if names.len() > 1 {
            names
        } else {
            vec![text.as_str()]
        };
        for name in names {
            let uses_alias = name
                .split('+')
                .any(|part| aliases.contains_key(&normalize_alias_name(part)));
            let canonical = if uses_alias {
                KeyChord::parse_with_aliases(name, aliases)
                    .map_err(|error| {
                        ConfigError::Parse(format!(
                            "{label} binding {name:?} is not a valid key or chord: {error}"
                        ))
                    })?
                    .canonical()
            } else if text.split_ascii_whitespace().count() > 1 {
                let chord = KeyChord::parse(name).map_err(|error| {
                    ConfigError::Parse(format!(
                        "{label} whitespace alias {name:?} is not a single key: {error}"
                    ))
                })?;
                if chord.keys().len() != 1 {
                    return Err(ConfigError::Parse(format!(
                        "{label} whitespace aliases must be single keys; use `+` for a chord: {text:?}"
                    )));
                }
                chord.canonical()
            } else {
                name.to_string()
            };
            if table.insert(canonical.clone(), binding.clone()).is_some() {
                return Err(ConfigError::Parse(format!(
                    "{label} binding {text:?} resolves to duplicate chord {canonical:?}"
                )));
            }
        }
    }
    Ok(())
}

pub(super) fn normalize_key_list(
    keys: &mut [String],
    aliases: &BTreeMap<String, String>,
) -> Result<(), ConfigError> {
    for key in keys {
        normalize_key_if_aliased(key, aliases)?;
    }
    Ok(())
}

pub(super) fn normalize_key_if_aliased(
    key: &mut String,
    aliases: &BTreeMap<String, String>,
) -> Result<(), ConfigError> {
    if aliases.contains_key(&normalize_alias_name(key)) {
        *key = Key::new_with_aliases(&*key, aliases)
            .map_err(ConfigError::Parse)?
            .to_string();
    }
    Ok(())
}
