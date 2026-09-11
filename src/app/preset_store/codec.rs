//! Versioned compact preorder encoding, decoded with explicit size/depth bounds.
use crate::api::window_layout::Axis;
use crate::api::window_presets::{
    MAX_NOTE_CHARS, MAX_PRESETS, RegionTemplate, SavedPreset, TabTemplate, WindowTemplate,
};
const HEADER: &[u8; 9] = b"KSWORKSP\x01";

pub(super) fn encode(layouts: &[SavedPreset]) -> Result<Vec<u8>, String> {
    if layouts.len() > MAX_PRESETS {
        return Err("Too many saved presets".into());
    }
    fn tree(node: &RegionTemplate, bytes: &mut Vec<u8>) {
        match node {
            RegionTemplate::Slot { id } => {
                bytes.push(0);
                bytes.extend(id.to_le_bytes());
            }
            RegionTemplate::Split {
                axis,
                ratio,
                first,
                second,
            } => {
                bytes.push(if *axis == Axis::X { 1 } else { 2 });
                bytes.extend(ratio.to_le_bytes());
                tree(first, bytes);
                tree(second, bytes);
            }
        }
    }
    let mut bytes = HEADER.to_vec();
    bytes.push(layouts.len() as u8);
    for layout in layouts {
        layout.validate()?;
        bytes.push(layout.id as u8);
        bytes.extend((layout.window_count as u16).to_le_bytes());
        bytes.extend((layout.note.len() as u16).to_le_bytes());
        bytes.extend(layout.note.as_bytes());
        match &layout.template {
            WindowTemplate::Layout(regions) => {
                bytes.push(0);
                tree(regions, &mut bytes);
            }
            WindowTemplate::Tabs(tabs) => {
                bytes.push(1);
                for coordinate in [
                    tabs.region.x,
                    tabs.region.y,
                    tabs.region.width,
                    tabs.region.height,
                ] {
                    bytes.extend(coordinate.to_le_bytes());
                }
                bytes.extend((tabs.active as u16).to_le_bytes());
            }
        }
    }
    Ok(bytes)
}
struct Reader<'a> {
    bytes: &'a [u8],
}
impl<'a> Reader<'a> {
    fn take(&mut self, count: usize) -> Result<&'a [u8], String> {
        let (head, tail) = self
            .bytes
            .split_at_checked(count)
            .ok_or("Workspace file is truncated")?;
        self.bytes = tail;
        Ok(head)
    }
    fn fixed<const N: usize>(&mut self) -> Result<[u8; N], String> {
        self.take(N)?
            .try_into()
            .map_err(|_| "Invalid saved presets field".into())
    }
    fn byte(&mut self) -> Result<u8, String> {
        Ok(self.fixed::<1>()?[0])
    }
    fn tree(&mut self, depth: usize, nodes: &mut usize) -> Result<RegionTemplate, String> {
        *nodes += 1;
        if depth > 32 || *nodes > 511 {
            return Err("Preset exceeds geometry limits".into());
        }
        match self.byte()? {
            0 => Ok(RegionTemplate::Slot {
                id: u32::from_le_bytes(self.fixed()?),
            }),
            tag @ (1 | 2) => {
                let ratio = f64::from_le_bytes(self.fixed()?);
                if !ratio.is_finite() || ratio <= 0.0 || ratio >= 1.0 {
                    return Err("Preset has an invalid divider".into());
                }
                Ok(RegionTemplate::Split {
                    axis: if tag == 1 { Axis::X } else { Axis::Y },
                    ratio,
                    first: Box::new(self.tree(depth + 1, nodes)?),
                    second: Box::new(self.tree(depth + 1, nodes)?),
                })
            }
            _ => Err("Preset has an invalid region tag".into()),
        }
    }
}
pub(super) fn decode(bytes: &[u8]) -> Result<Vec<SavedPreset>, String> {
    let mut reader = Reader { bytes };
    if reader.take(8)? != &HEADER[..8] {
        return Err("Unsupported saved presets file".into());
    }
    let version = reader.byte()?;
    if version != 1 {
        return Err("Unsupported saved presets file".into());
    }
    let count = reader.byte()? as usize;
    if count > MAX_PRESETS {
        return Err("Too many saved presets".into());
    }
    let mut layouts = Vec::with_capacity(count);
    let mut ids = std::collections::BTreeSet::new();
    for _ in 0..count {
        let id = reader.byte()? as u32;
        let window_count = u16::from_le_bytes(reader.fixed()?) as usize;
        let length = u16::from_le_bytes(reader.fixed()?) as usize;
        if length > MAX_NOTE_CHARS * 4 {
            return Err("Preset note is too long".into());
        }
        let note = std::str::from_utf8(reader.take(length)?)
            .map_err(|_| "Preset note is not valid UTF-8")?
            .to_string();
        let tag = reader.byte()?;
        let template = match tag {
            0 => WindowTemplate::Layout(reader.tree(0, &mut 0)?),
            1 => {
                let x = f64::from_le_bytes(reader.fixed()?);
                let y = f64::from_le_bytes(reader.fixed()?);
                let width = f64::from_le_bytes(reader.fixed()?);
                let height = f64::from_le_bytes(reader.fixed()?);
                let active = u16::from_le_bytes(reader.fixed()?) as usize;
                WindowTemplate::Tabs(TabTemplate {
                    region: crate::api::Rect::new(x, y, width, height),
                    active,
                })
            }
            _ => return Err("Preset has an invalid record type".into()),
        };
        let layout = SavedPreset {
            id,
            window_count,
            note,
            template,
        };
        layout.validate()?;
        if !ids.insert(id) {
            return Err("Duplicate saved layout number".into());
        }
        layouts.push(layout);
    }
    if !reader.bytes.is_empty() {
        return Err("Workspace file has unexpected trailing data".into());
    }
    layouts.sort_by_key(|layout| layout.id);
    Ok(layouts)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn shared_typed_browser_fixture_roundtrips_and_rejects_truncation() {
        let bytes = include_bytes!("../../../tests/fixtures/workspace.ksw");
        let layouts = decode(bytes).unwrap();
        assert_eq!(layouts.len(), 3);
        let template = layouts.iter().find(|p| p.id == 3).unwrap();
        assert_eq!(template.window_count, 3);
        assert_eq!(
            match &template.template {
                WindowTemplate::Tabs(tabs) => tabs.active,
                _ => panic!("expected Tabs"),
            },
            2
        );
        assert_eq!(encode(&layouts).unwrap(), bytes);
        for length in 0..bytes.len() {
            assert!(decode(&bytes[..length]).is_err());
        }
        let mut invalid = template.clone();
        if let WindowTemplate::Tabs(tabs) = &mut invalid.template {
            tabs.active = 3;
        }
        assert!(encode(&[invalid]).is_err());
    }
    #[test]
    fn compact_roundtrip_rejects_every_truncation_and_unknown_version() {
        let layouts = vec![SavedPreset {
            id: 1,
            note: "写代码 🦀".into(),
            window_count: 1,
            template: WindowTemplate::Layout(RegionTemplate::Split {
                axis: Axis::X,
                ratio: 1.0 / 3.0,
                first: Box::new(RegionTemplate::Slot { id: 1 }),
                second: Box::new(RegionTemplate::Slot { id: 2 }),
            }),
        }];
        let bytes = encode(&layouts).unwrap();
        assert_eq!(decode(&bytes).unwrap(), layouts);
        assert!(bytes.len() < 60);
        for len in 0..bytes.len() {
            assert!(decode(&bytes[..len]).is_err(), "truncation {len}");
        }
        let mut bad = bytes.clone();
        bad[8] = 3;
        assert!(decode(&bad).is_err());
        let mut bad = bytes;
        bad.push(0);
        assert!(decode(&bad).is_err());
    }
}
