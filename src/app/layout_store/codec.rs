//! Versioned compact preorder encoding, decoded with explicit size/depth bounds.
use crate::api::window_layout::Axis;
use crate::api::window_presets::{MAX_NOTE_CHARS, MAX_PRESETS, RegionTemplate, SavedLayout};
const HEADER: &[u8; 9] = b"KSLAYOUT\x01";

pub(super) fn encode(layouts: &[SavedLayout]) -> Result<Vec<u8>, String> {
    if layouts.len() > MAX_PRESETS {
        return Err("Too many saved layouts".into());
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
        tree(&layout.regions, &mut bytes);
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
            .ok_or("Saved layouts file is truncated")?;
        self.bytes = tail;
        Ok(head)
    }
    fn fixed<const N: usize>(&mut self) -> Result<[u8; N], String> {
        self.take(N)?
            .try_into()
            .map_err(|_| "Invalid saved layouts field".into())
    }
    fn byte(&mut self) -> Result<u8, String> {
        Ok(self.fixed::<1>()?[0])
    }
    fn tree(&mut self, depth: usize, nodes: &mut usize) -> Result<RegionTemplate, String> {
        *nodes += 1;
        if depth > 32 || *nodes > 511 {
            return Err("Saved layout exceeds geometry limits".into());
        }
        match self.byte()? {
            0 => Ok(RegionTemplate::Slot {
                id: u32::from_le_bytes(self.fixed()?),
            }),
            tag @ (1 | 2) => {
                let ratio = f64::from_le_bytes(self.fixed()?);
                if !ratio.is_finite() || ratio <= 0.0 || ratio >= 1.0 {
                    return Err("Saved layout has an invalid divider".into());
                }
                Ok(RegionTemplate::Split {
                    axis: if tag == 1 { Axis::X } else { Axis::Y },
                    ratio,
                    first: Box::new(self.tree(depth + 1, nodes)?),
                    second: Box::new(self.tree(depth + 1, nodes)?),
                })
            }
            _ => Err("Saved layout has an invalid region tag".into()),
        }
    }
}
pub(super) fn decode(bytes: &[u8]) -> Result<Vec<SavedLayout>, String> {
    let mut reader = Reader { bytes };
    if reader.take(HEADER.len())? != HEADER {
        return Err("Unsupported saved layouts file".into());
    }
    let count = reader.byte()? as usize;
    if count > MAX_PRESETS {
        return Err("Too many saved layouts".into());
    }
    let mut layouts = Vec::with_capacity(count);
    let mut ids = std::collections::BTreeSet::new();
    for _ in 0..count {
        let id = reader.byte()? as u32;
        let window_count = u16::from_le_bytes(reader.fixed()?) as usize;
        let length = u16::from_le_bytes(reader.fixed()?) as usize;
        if length > MAX_NOTE_CHARS * 4 {
            return Err("Saved layout note is too long".into());
        }
        let note = std::str::from_utf8(reader.take(length)?)
            .map_err(|_| "Saved layout note is not valid UTF-8")?
            .to_string();
        let regions = reader.tree(0, &mut 0)?;
        let layout = SavedLayout {
            id,
            window_count,
            note,
            regions,
        };
        layout.validate()?;
        if !ids.insert(id) {
            return Err("Duplicate saved layout number".into());
        }
        layouts.push(layout);
    }
    if !reader.bytes.is_empty() {
        return Err("Saved layouts file has unexpected trailing data".into());
    }
    layouts.sort_by_key(|layout| layout.id);
    Ok(layouts)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn shared_browser_fixture_round_trips_byte_for_byte() {
        let fixture = include_bytes!("../../../tests/fixtures/window-layouts-v1.kslayout");
        let layouts = decode(fixture).unwrap();
        assert_eq!(layouts.len(), 2);
        assert_eq!(layouts[0].note, "Code · 阅读");
        assert_eq!(layouts[1].id, 12);
        assert_eq!(encode(&layouts).unwrap(), fixture);
    }
    #[test]
    fn compact_roundtrip_rejects_every_truncation_and_unknown_version() {
        let layouts = vec![SavedLayout {
            id: 1,
            note: "写代码 🦀".into(),
            window_count: 1,
            regions: RegionTemplate::Split {
                axis: Axis::X,
                ratio: 1.0 / 3.0,
                first: Box::new(RegionTemplate::Slot { id: 1 }),
                second: Box::new(RegionTemplate::Slot { id: 2 }),
            },
        }];
        let bytes = encode(&layouts).unwrap();
        assert_eq!(decode(&bytes).unwrap(), layouts);
        assert!(bytes.len() < 60);
        for len in 0..bytes.len() {
            assert!(decode(&bytes[..len]).is_err(), "truncation {len}");
        }
        let mut bad = bytes.clone();
        bad[8] = 2;
        assert!(decode(&bad).is_err());
        let mut bad = bytes;
        bad.push(0);
        assert!(decode(&bad).is_err());
    }
}
