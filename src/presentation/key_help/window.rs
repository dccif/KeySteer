//! Semantic sections for Window help. Keys always come from effective bindings.
use std::collections::BTreeMap;

type Entry = (String, String);

pub(super) struct Sections {
    pub left: Vec<Entry>,
    pub right: Vec<Entry>,
    pub modes: Vec<Entry>,
    pub exit: String,
    pub exit_label: String,
}

impl Sections {
    pub fn entries(self, columns: usize) -> Vec<Entry> {
        let mut left = self.left;
        if columns == 2 {
            let rows = left.len().max(self.right.len());
            left.resize(rows, (String::new(), String::new()));
            left.extend(self.right);
            left.resize(rows * 2, (String::new(), String::new()));
        } else {
            left.extend(self.right);
        }
        left
    }
}

fn take(actions: &mut BTreeMap<String, Vec<String>>, name: &str, entries: &mut Vec<Entry>) {
    if let Some(keys) = actions.remove(name) {
        entries.push((
            keys.join(" / "),
            super::window_action_label(name).unwrap_or(name).into(),
        ));
    }
}

/// Collapse a complete directional family only when its modifier prefix is
/// shared. A partial or mixed-modifier binding keeps its individual meaning.
fn family(
    actions: &mut BTreeMap<String, Vec<String>>,
    names: &[&str],
    caption: &str,
    entries: &mut Vec<Entry>,
) {
    let keys: Option<Vec<_>> = names
        .iter()
        .map(|name| {
            actions
                .get(*name)
                .filter(|keys| keys.len() == 1)
                .map(|keys| &keys[0])
        })
        .collect();
    if let Some(keys) = keys {
        let parts: Vec<_> = keys
            .iter()
            .map(|key| key.rsplit_once('+').unwrap_or(("", key)))
            .collect();
        if parts.iter().all(|(prefix, _)| *prefix == parts[0].0) {
            let leaves = parts
                .iter()
                .map(|(_, key)| *key)
                .collect::<Vec<_>>()
                .join("/");
            let keys = if parts[0].0.is_empty() {
                leaves
            } else {
                format!("{}+{leaves}", parts[0].0)
            };
            for name in names {
                actions.remove(*name);
            }
            entries.push((keys, caption.into()));
            return;
        }
    }
    for name in names {
        take(actions, name, entries);
    }
}

fn pair(
    actions: &mut BTreeMap<String, Vec<String>>,
    names: [&str; 2],
    caption: &str,
    entries: &mut Vec<Entry>,
) {
    if names.iter().all(|name| actions.contains_key(*name)) {
        let keys = names
            .into_iter()
            .flat_map(|name| actions.remove(name).unwrap_or_default())
            .collect::<Vec<_>>();
        entries.push((keys.join(" / "), caption.into()));
    } else {
        for name in names {
            take(actions, name, entries);
        }
    }
}

// Only abbreviate Shift when the effective bindings really have that relation.
fn audio_scopes<const N: usize>(
    actions: &mut BTreeMap<String, Vec<String>>,
    app: [&str; N],
    system: [&str; N],
    label: &str,
    arrows: &str,
    entries: &mut Vec<Entry>,
) {
    let complete = app
        .iter()
        .chain(system.iter())
        .all(|name| actions.contains_key(*name));
    let shifted = app.iter().zip(system).all(|(a, b)| {
        matches!((actions.get(*a), actions.get(b)), (Some(a), Some(b)) if a.len() == 1 && b.len() == 1 && b[0] == format!("SHIFT+{}", a[0]))
    });
    let mut local = Vec::new();
    let mut global = Vec::new();
    family(actions, &app, &format!("App {label} {arrows}"), &mut local);
    family(
        actions,
        &system,
        &format!("System {label} {arrows}"),
        &mut global,
    );
    if complete && local.len() == 1 && global.len() == 1 {
        let keys = if shifted {
            local[0].0.clone()
        } else {
            format!("{} / {}", local[0].0, global[0].0)
        };
        let caption = if shifted {
            format!("{label} {arrows} · Shift: system")
        } else {
            format!("App / system {label} {arrows}")
        };
        entries.push((keys, caption));
    } else {
        entries.extend(local);
        entries.extend(global);
    }
}

fn section(entries: &mut Vec<Entry>, title: &str, mut content: Vec<Entry>) {
    if !content.is_empty() {
        entries.push((String::new(), title.into()));
        entries.append(&mut content);
    }
}

pub(super) fn sections(
    mut actions: BTreeMap<String, Vec<String>>,
    mode: &str,
    return_target: Option<&str>,
    resizing: bool,
) -> Sections {
    let destination = return_target
        .filter(|target| actions.contains_key(*target))
        .unwrap_or("idle");
    let exit = actions.remove(destination).unwrap_or_default().join(" / ");
    let exit_label = format!(
        "{} \u{2192} {}",
        if destination == "idle" {
            "Exit"
        } else {
            "Back"
        },
        super::window_action_label(destination).unwrap_or(destination)
    );
    let mut operations = Vec::new();
    if mode == "window_tab" {
        family(
            &mut actions,
            &["move_left", "move_down", "move_up", "move_right"],
            "Move / switch tab",
            &mut operations,
        );
    }
    for (names, caption) in [
        (
            &["window_left", "window_down", "window_up", "window_right"][..],
            if resizing {
                "Resize ←↓↑→"
            } else {
                "Move ←↓↑→"
            },
        ),
        (
            &[
                "window_layout_left",
                "window_layout_down",
                "window_layout_up",
                "window_layout_right",
            ][..],
            if mode == "window_editor" {
                "Select ←↓↑→"
            } else {
                "Layout ←↓↑→"
            },
        ),
        (
            &[
                "window_split_left",
                "window_split_down",
                "window_split_up",
                "window_split_right",
            ][..],
            "Split ←↓↑→",
        ),
        (
            &["window_ratio_left", "window_ratio_right"][..],
            "Width − / +",
        ),
        (
            &["window_ratio_up", "window_ratio_down"][..],
            "Height − / +",
        ),
        (
            &["window_tab_move_left", "window_tab_move_right"][..],
            "Move tab ← / →",
        ),
    ] {
        family(&mut actions, names, caption, &mut operations);
    }
    pair(
        &mut actions,
        ["window_tab_next", "window_tab_previous"],
        "Next / previous tab",
        &mut operations,
    );
    for name in [
        "window_size",
        "window_center",
        "window_screen_next",
        "window_screen_previous",
        "size_cycle",
        "window_close",
        "window_remove_region",
        "window_save_layout",
        "Area number",
        "window_area_number",
        "window_tab_end",
        "window_tab_group",
        "window_number_end",
        "window_tab_remove",
        "window_tab_dissolve",
        "window_delete",
        "window_confirm",
        "Previous page",
        "Next page",
    ] {
        take(&mut actions, name, &mut operations);
    }
    let mut modes = Vec::new();
    for name in [
        "window_quick",
        "window_editor",
        "window_restore",
        "window_tab",
    ] {
        take(&mut actions, name, &mut modes);
    }
    for (_, caption) in &mut modes {
        *caption = match caption.as_str() {
            "Quick layout" => "Quick",
            "Edit layout tree" => "Edit",
            "Restore layout" => "Restore",
            "Group windows as tabs" => "Tabs",
            "Delete layouts" => "Delete",
            _ => caption.as_str(),
        }
        .into();
    }
    let mut common = Vec::new();
    pair(
        &mut actions,
        ["window_select", "window_select_previous"],
        "Next / previous window",
        &mut common,
    );
    audio_scopes(
        &mut actions,
        ["window_volume_down", "window_volume_up"],
        ["window_system_volume_down", "window_system_volume_up"],
        "Volume",
        "− / +",
        &mut common,
    );
    audio_scopes(
        &mut actions,
        ["window_audio_previous", "window_audio_next"],
        ["window_system_audio_previous", "window_system_audio_next"],
        "Output",
        "← / →",
        &mut common,
    );
    audio_scopes(
        &mut actions,
        ["window_volume_mute"],
        ["window_system_volume_mute"],
        "Mute",
        "/ unmute",
        &mut common,
    );
    let undo = actions.get("window_undo").filter(|keys| keys.len() == 1);
    let redo = actions.get("window_redo").filter(|keys| keys.len() == 1);
    if let (Some(undo), Some(redo)) = (undo, redo) {
        common.push((format!("{} / {}", undo[0], redo[0]), "Undo / redo".into()));
        actions.remove("window_undo");
        actions.remove("window_redo");
    }
    for name in [
        "window_select",
        "window_select_previous",
        "window_undo",
        "window_redo",
        "window_reset_initial",
        "idle",
        "window",
        "normal",
        "key_help",
    ] {
        take(&mut actions, name, &mut common);
    }
    let mut left = Vec::new();
    let title = "ACTIONS";
    section(&mut left, title, operations);
    let other = actions
        .into_iter()
        .map(|(action, keys)| {
            (
                keys.join(" / "),
                super::window_action_label(&action)
                    .unwrap_or(&action)
                    .into(),
            )
        })
        .collect();
    section(&mut left, "OTHER ACTIONS", other);
    let mut right = Vec::new();
    section(&mut right, "COMMON", common);
    Sections {
        left,
        right,
        modes,
        exit,
        exit_label,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pairs_navigation_even_with_different_modifiers_and_multiple_bindings() {
        for (next, previous, mode, caption) in [
            (
                "window_select",
                "window_select_previous",
                "window",
                "Next / previous window",
            ),
            (
                "window_tab_next",
                "window_tab_previous",
                "window_tab",
                "Next / previous tab",
            ),
        ] {
            let mut actions = BTreeMap::new();
            actions.insert(next.into(), vec!["TAB".into(), "F9".into()]);
            actions.insert(previous.into(), vec!["SHIFT+TAB".into()]);
            let plan = sections(actions.clone(), mode, None, false);
            assert!(
                plan.left
                    .iter()
                    .chain(&plan.right)
                    .any(|e| e == &("TAB / F9 / SHIFT+TAB".into(), caption.into()))
            );
            actions.remove(previous);
            let plan = sections(actions, mode, None, false);
            assert!(
                plan.left
                    .iter()
                    .chain(&plan.right)
                    .any(|(key, _)| key == "TAB / F9")
            );
        }
    }

    #[test]
    fn groups_actions_independently_of_configured_key_order() {
        let actions = [
            ("window_left", "B"),
            ("window_down", "N"),
            ("window_up", "M"),
            ("window_right", "V"),
            ("window_quick", "F9"),
            ("window_editor", "F8"),
            ("window_restore", "F7"),
            ("window_tab", "F6"),
            ("window_undo", "U"),
            ("idle", "ESCAPE"),
            ("send:custom", "F1"),
        ]
        .into_iter()
        .map(|(action, key)| (action.into(), vec![key.into()]))
        .collect();
        let plan = sections(actions, "window", None, false);
        assert_eq!(plan.exit, "ESCAPE");
        assert_eq!(plan.left[1], ("B/N/M/V".into(), "Move ←↓↑→".into()));
        assert!(
            plan.left
                .iter()
                .any(|(key, action)| key == "F1" && action == "send:custom")
        );
        assert_eq!(
            plan.modes
                .iter()
                .map(|(key, _)| key.as_str())
                .collect::<Vec<_>>(),
            ["F9", "F8", "F7", "F6"]
        );
    }

    #[test]
    fn mixed_modifiers_and_partial_families_keep_exact_individual_bindings() {
        let actions = [
            ("window_ratio_left", "CTRL+H"),
            ("window_ratio_right", "F9"),
            ("window_split_up", "SHIFT+K"),
        ]
        .into_iter()
        .map(|(action, key)| (action.into(), vec![key.into()]))
        .collect();
        let plan = sections(actions, "window_editor", None, false);
        assert!(
            plan.left
                .contains(&("F9".into(), "Grow region width".into()))
        );
        assert!(
            plan.left
                .contains(&("CTRL+H".into(), "Shrink region width".into()))
        );
        assert!(plan.left.contains(&("SHIFT+K".into(), "Split up".into())));
    }
}
