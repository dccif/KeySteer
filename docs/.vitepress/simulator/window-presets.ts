import type { LayoutNode, LayoutTree } from './window-layout.ts'
import { moveToSlot, treeSlots } from './window-layout.ts'
import type { DemoWindow } from './window.ts'

export type RegionTemplate = { kind: 'slot'; id: number } | { kind: 'split'; axis: 'x' | 'y'; ratio: number; first: RegionTemplate; second: RegionTemplate }
export interface SavedWindowLayout { id: number; note: string; window_count: number; regions: RegionTemplate }
export const LAYOUT_STORAGE_KEY = 'keysteer.simulator.layouts.v1'
export function regionTemplate(node: LayoutNode): RegionTemplate {
  return node.kind === 'slot' ? { kind: 'slot', id: node.id } : { kind: 'split', axis: node.axis, ratio: node.ratio, first: regionTemplate(node.first), second: regionTemplate(node.second) }
}
export function savedLayoutName(layout: SavedWindowLayout): string {
  if (layout.note.trim()) return layout.note
  const count = regionCount(layout.regions)
  return `Layout ${layout.id} · ${layout.window_count} windows${count === layout.window_count ? '' : ` / ${count} regions`}`
}
function regionCount(node: RegionTemplate): number { return node.kind === 'slot' ? 1 : regionCount(node.first) + regionCount(node.second) }
export function instantiateLayout(layout: SavedWindowLayout, windows: DemoWindow[]): LayoutTree {
  function node(template: RegionTemplate): LayoutNode {
    return template.kind === 'slot' ? { ...template, window: null } : { ...template, first: node(template.first), second: node(template.second) }
  }
  const tree: LayoutTree = { root: node(layout.regions), selected: 1, nextSlot: 1 }
  const slots = treeSlots(tree).sort((a, b) => a.id - b.id)
  tree.selected = slots[0].id; tree.nextSlot = slots.at(-1)!.id + 1
  const eligible = windows.filter((w, i) => w.resizable !== false && !w.fullscreen && windows.findIndex(v => v.id === w.id) === i)
  slots.forEach((slot, index) => { if (eligible[index]) moveToSlot(tree, eligible[index].id, slot.id) })
  return tree
}
export function readSavedLayouts(text: string | null): SavedWindowLayout[] {
  if (text === null) return []
  if (text.length > 1024 * 1024) throw new Error('布局库过大')
  return validateSavedLayouts(JSON.parse(text))
}
function validateSavedLayouts(data: unknown): SavedWindowLayout[] {
  if (!Array.isArray(data) || data.length > 99) throw new Error('布局库格式无效')
  const ids = new Set<number>()
  function regions(value: any, seen: Set<number>, depth = 0): RegionTemplate {
    if (!value || depth > 32) throw new Error('区域结构无效')
    if (value.kind === 'slot' && Number.isInteger(value.id) && value.id > 0 && value.id < 0xffffffff && !seen.has(value.id) && seen.size < 256) {
      seen.add(value.id); return { kind: 'slot', id: value.id }
    }
    if (value.kind === 'split' && ['x', 'y'].includes(value.axis) && Number.isFinite(value.ratio) && value.ratio > 0 && value.ratio < 1) {
      return { kind: 'split', axis: value.axis, ratio: value.ratio, first: regions(value.first, seen, depth + 1), second: regions(value.second, seen, depth + 1) }
    }
    throw new Error('区域结构无效')
  }
  return data.map((value: any) => {
    if (!Number.isInteger(value?.id) || value.id < 1 || value.id > 99 || ids.has(value.id) || typeof value.note !== 'string' || [...value.note].length > 80 || /[\x00-\x1f\x7f-\x9f]/.test(value.note)) throw new Error('布局备注或编号无效')
    ids.add(value.id)
    const tree = regions(value.regions, new Set())
    if (!Number.isInteger(value.window_count) || value.window_count < 0 || value.window_count > regionCount(tree)) throw new Error('窗口数无效')
    return { id: value.id, note: value.note, window_count: value.window_count, regions: tree }
  }).sort((a, b) => a.id - b.id)
}

const FILE_HEADER = [75, 83, 76, 65, 89, 79, 85, 84, 1]
/** Identical to app/layout_store/codec.rs; browser storage remains independent. */
export function encodeLayoutFile(layouts: SavedWindowLayout[]): Uint8Array {
  const checked = validateSavedLayouts(layouts)
  const output = [...FILE_HEADER, checked.length], encoder = new TextEncoder()
  function integer(value: number, length: number): void { for (let i = 0; i < length; i++) output.push((value >>> (i * 8)) & 255) }
  function tree(node: RegionTemplate): void {
    if (node.kind === 'slot') { output.push(0); integer(node.id, 4); return }
    output.push(node.axis === 'x' ? 1 : 2)
    const number = new Uint8Array(8); new DataView(number.buffer).setFloat64(0, node.ratio, true); output.push(...number)
    tree(node.first); tree(node.second)
  }
  for (const layout of checked) {
    output.push(layout.id); integer(layout.window_count, 2)
    const note = encoder.encode(layout.note); integer(note.length, 2); output.push(...note)
    tree(layout.regions)
  }
  return new Uint8Array(output)
}
export function decodeLayoutFile(bytes: Uint8Array): SavedWindowLayout[] {
  if (bytes.length > 1024 * 1024) throw new Error('布局文件过大')
  let offset = 0
  function take(count: number): Uint8Array {
    if (offset + count > bytes.length) throw new Error('布局文件不完整')
    const value = bytes.subarray(offset, offset + count); offset += count; return value
  }
  function integer(length: number): number { return take(length).reduce((value, byte, index) => value + byte * 2 ** (index * 8), 0) }
  if (!take(FILE_HEADER.length).every((byte, index) => byte === FILE_HEADER[index])) throw new Error('不支持此布局文件版本')
  const count = integer(1)
  if (count > 99) throw new Error('布局数量过多')
  function tree(depth: number, budget: { nodes: number }): RegionTemplate {
    if (depth > 32 || ++budget.nodes > 511) throw new Error('区域结构超出限制')
    const tag = integer(1)
    if (tag === 0) return { kind: 'slot', id: integer(4) }
    if (tag !== 1 && tag !== 2) throw new Error('区域结构无效')
    const number = take(8), ratio = new DataView(number.buffer, number.byteOffset, 8).getFloat64(0, true)
    if (!Number.isFinite(ratio) || ratio <= 0 || ratio >= 1) throw new Error('分割比例无效')
    return { kind: 'split', axis: tag === 1 ? 'x' : 'y', ratio, first: tree(depth + 1, budget), second: tree(depth + 1, budget) }
  }
  const layouts: SavedWindowLayout[] = [], decoder = new TextDecoder('utf-8', { fatal: true })
  for (let i = 0; i < count; i++) {
    const id = integer(1), window_count = integer(2), length = integer(2)
    if (length > 320) throw new Error('备注过长')
    const note = decoder.decode(take(length)), regions = tree(0, { nodes: 0 })
    layouts.push({ id, window_count, note, regions })
  }
  if (offset !== bytes.length) throw new Error('布局文件含有多余数据')
  return validateSavedLayouts(layouts)
}
