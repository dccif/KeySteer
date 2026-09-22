#![cfg(test)]

use super::*;
use crate::api::window::{WindowId, WindowInfo};
use crate::api::window_tabs::{TabGroup, TabGroupId, TabState};
use crate::api::{Appearance, Point, Screen};
use std::collections::BTreeMap;
#[test]
fn window_scenes_match_pre_optimization_baseline() {
    let mut results = Vec::new();
    for dpi in [1.0, 2.0] {
        for appearance in [Appearance::Light, Appearance::Dark] {
            for enabled in [false, true] {
                for count in [1usize, 5, 16] {
                    for position in 0..3 {
                        for kind in 0..3 {
                            for grouped in [false, true] {
                                let mut config = crate::config::Config::parse(&format!("[window.card]\nguide_line_enabled = {enabled}\nposition_mode = \"{}\"\nposition = {}\n", if position == 0 {"window"} else {"screen"}, if position == 2 {r#"["50%", "50%", "50%", "50%"]"#} else {r#"["100%", "0%", "0%", "0%"]"#})).unwrap();
                                config.window.card.text_width = 200.0;
                                let light = config.palette(Appearance::Light);
                                let dark = config.palette(Appearance::Dark);
                                let styles = crate::api::style::WindowStyles::new(
                                    &config.window.ui,
                                    &config.window.card,
                                    &light,
                                    &dark,
                                    crate::presentation::window::RENDERERS,
                                );
                                let screens = [
                                    Screen {
                                        name: None,
                                        bounds: Rect::new(0.0, 0.0, 1920.0, 1080.0),
                                        work_area: Rect::new(0.0, 30.0, 1920.0, 1010.0),
                                        scale: dpi,
                                        is_primary: true,
                                    },
                                    Screen {
                                        name: None,
                                        bounds: Rect::new(-1600.0, 0.0, 1600.0, 900.0),
                                        work_area: Rect::new(-1600.0, 25.0, 1600.0, 850.0),
                                        scale: 1.5,
                                        is_primary: false,
                                    },
                                ];
                                let inventory: BTreeMap<_, _> = (0..count)
                                    .map(|i| {
                                        let screen = i % 2;
                                        let area = screens[screen].work_area;
                                        let id = WindowId(i as u64 + 1);
                                        (
                                            id,
                                            WindowInfo {
                                                id,
                                                title: format!(
                                                    "Title {i} 长标题 with uneven lengths {}",
                                                    "x".repeat(i % 7)
                                                ),
                                                app: format!("App {}", i % 3),
                                                bounds: Rect::new(
                                                    area.x + 57.0 + (i % 4) as f64 * 170.0,
                                                    area.y + 83.0 + (i % 3) as f64 * 113.0,
                                                    400.0 + (i % 3) as f64 * 25.0,
                                                    280.0,
                                                ),
                                                screen,
                                                resizable: true,
                                                minimized: false,
                                                maximized: false,
                                                fullscreen: false,
                                            },
                                        )
                                    })
                                    .collect();
                                let visible: Vec<_> = inventory.keys().copied().collect();
                                let numbers = visible
                                    .iter()
                                    .enumerate()
                                    .map(|(i, id)| (*id, i as u32 + 1))
                                    .collect();
                                let mut tabs = TabState::default();
                                if grouped && count >= 5 {
                                    tabs.groups.push(TabGroup {
                                        id: TabGroupId(1),
                                        members: vec![WindowId(1), WindowId(3), WindowId(5)],
                                        active: WindowId(1),
                                    });
                                }
                                let windows: Vec<_> = inventory
                                    .values()
                                    .filter(|w| w.screen == 0)
                                    .cloned()
                                    .collect();
                                let tree = crate::api::window_layout::LayoutTree::import(
                                    &windows,
                                    Some(WindowId(1)),
                                    screens[0].work_area,
                                );
                                let ctx = HostContext {
                                    presenter: &crate::presentation::COMPOSER,
                                    screens: &screens,
                                    cursor: Point::new(211.0, 155.0),
                                    focused_app: None,
                                    palette: if appearance == Appearance::Light {
                                        &light
                                    } else {
                                        &dark
                                    },
                                };
                                let view = WindowView {
                                    text_cache: None,
                                    configurable_position: kind != 2,
                                    tabs: &tabs,
                                    group_input: grouped,
                                    styles: &styles,
                                    border_width: 3.0,
                                    target: inventory.get(&WindowId(1)),
                                    screen: 0,
                                    inventory: &inventory,
                                    visible: &visible,
                                    numbers: &numbers,
                                    tree: (kind != 0).then_some(&tree),
                                    gap: 0.0,
                                };
                                let mut scene = view.scene(&ctx);
                                for stage in 0..3 {
                                    scene.sort_in_place();
                                    if stage > 0 {
                                        crate::presentation::label_placement::avoid_overlaps(
                                            &mut scene,
                                            &screens[0],
                                            &[Rect::new(400.0, 770.0, 1100.0, 240.0)],
                                        );
                                        scene.sort_in_place();
                                    }
                                    let mut json = serde_json::to_value(&scene).unwrap();
                                    json.as_object_mut().unwrap().remove("window_annotations");
                                    for label in json["labels"].as_array_mut().unwrap() {
                                        let o = label.as_object_mut().unwrap();
                                        o.remove("placement");
                                        o.remove("connector");
                                    }
                                    results.push(serde_json::json!({"key":format!("{dpi}/{appearance:?}/{enabled}/{count}/{position}/{kind}/{grouped}/{stage}"),"scene":json}));
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    let fixture = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/window-scenes-c7bff963.txt"
    ))
    .expect("the independently captured c7bff963 scene fixture must be installed");
    let expected: Vec<_> = fixture
        .lines()
        .filter(|line| !line.starts_with('#'))
        .collect();
    assert_eq!(results.len(), expected.len());
    for (result, expected) in results.iter().zip(expected) {
        let (key, digest) = expected.split_once(' ').unwrap();
        assert_eq!(result["key"], key);
        assert_eq!(
            scene_digest(&result["scene"]),
            u64::from_str_radix(digest, 16).unwrap(),
            "scene differs from c7bff963: {key}"
        );
    }
}
// Ignore serialization-only metadata and sub-micro-pixel float round trips.
// The fixture is captured from the baseline, never regenerated from this renderer.
fn scene_digest(value: &serde_json::Value) -> u64 {
    fn feed(hash: &mut u64, bytes: &[u8]) {
        for byte in bytes {
            *hash = (*hash ^ u64::from(*byte)).wrapping_mul(0x100000001b3);
        }
    }
    fn walk(hash: &mut u64, value: &serde_json::Value) {
        use serde_json::Value;
        match value {
            Value::Null => feed(hash, b"n"),
            Value::Bool(v) => feed(hash, if *v { b"t" } else { b"f" }),
            Value::Number(v) => {
                feed(hash, b"d");
                feed(
                    hash,
                    &((v.as_f64().unwrap() * 1_000_000.0).round() as i64).to_le_bytes(),
                );
            }
            Value::String(v) => {
                feed(hash, b"s");
                feed(hash, v.as_bytes());
                feed(hash, &[0]);
            }
            Value::Array(v) => {
                feed(hash, b"[");
                for item in v {
                    walk(hash, item);
                }
                feed(hash, b"]");
            }
            Value::Object(v) => {
                feed(hash, b"{");
                for (key, value) in v {
                    walk(hash, &Value::String(key.clone()));
                    walk(hash, value);
                }
                feed(hash, b"}");
            }
        }
    }
    let mut hash = 0xcbf29ce484222325;
    walk(&mut hash, value);
    hash
}
