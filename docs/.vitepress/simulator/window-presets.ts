import type { LayoutNode, LayoutTree } from './window-layout.ts'
import { moveToSlot, treeSlots } from './window-layout.ts'
import type { DemoWindow } from './window.ts'

export type RegionTemplate = { kind: 'slot'; id: number } | { kind: 'split'; axis: 'x' | 'y'; ratio: number; first: RegionTemplate; second: RegionTemplate }
export interface TabTemplate { region: { x: number; y: number; width: number; height: number }; active: number }
export type WindowTemplate = { kind: 'layout'; data: RegionTemplate } | { kind: 'tabs'; data: TabTemplate }
export interface SavedWindowPreset { id: number; note: string; window_count: number; template: WindowTemplate }
export const WORKSPACE_FILE_NAME = 'workspace.ksw'
export const WORKSPACE_STORAGE_KEY = 'keysteer.simulator.workspace.v1'
export function regionTemplate(node: LayoutNode): RegionTemplate {
  return node.kind === 'slot' ? { kind: 'slot', id: node.id } : { kind: 'split', axis: node.axis, ratio: node.ratio, first: regionTemplate(node.first), second: regionTemplate(node.second) }
}
export function presetName(preset: SavedWindowPreset): string {
  return preset.note.trim() || `${preset.template.kind === 'tabs' ? 'Tabs' : 'Layout'} ${preset.id}`
}
function regionCount(node: RegionTemplate): number { return node.kind === 'slot' ? 1 : regionCount(node.first) + regionCount(node.second) }
export function instantiateLayout(layout: SavedWindowPreset, windows: DemoWindow[]): LayoutTree {
  if (layout.template.kind !== 'layout') throw new Error('Tab 模板需要手动选择窗口')
  function node(template: RegionTemplate): LayoutNode {
    return template.kind === 'slot' ? { ...template, window: null } : { ...template, first: node(template.first), second: node(template.second) }
  }
  const tree: LayoutTree = { root: node(layout.template.data), selected: 1, nextSlot: 1 }
  const slots = treeSlots(tree).sort((a, b) => a.id - b.id)
  tree.selected = slots[0].id; tree.nextSlot = slots.at(-1)!.id + 1
  const eligible = windows.filter((w, i) => w.resizable !== false && !w.fullscreen && windows.findIndex(v => v.id === w.id) === i)
  slots.forEach((slot, index) => { if (eligible[index]) moveToSlot(tree, eligible[index].id, slot.id) })
  return tree
}
export function readSavedPresets(text: string | null): SavedWindowPreset[] {
  if (text === null) return []
  if (text.length > 1024 * 1024) throw new Error('工作区预设库过大')
  return validateSavedPresets(JSON.parse(text))
}
function validateSavedPresets(data: unknown): SavedWindowPreset[] {
  if (!Array.isArray(data) || data.length > 99) throw new Error('工作区预设库格式无效')
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
    if (!Number.isInteger(value?.id) || value.id < 1 || value.id > 99 || ids.has(value.id) || typeof value.note !== 'string' || [...value.note].length > 80 || /[\x00-\x1f\x7f-\x9f]/.test(value.note)) throw new Error('预设备注或编号无效')
    ids.add(value.id)
    if (!Number.isInteger(value.window_count) || value.window_count < 0) throw new Error('窗口数无效')
    let template: WindowTemplate
    if (value.template?.kind === 'tabs') {
      const r = value.template.data?.region, active = value.template.data?.active
      if (value.window_count < 2 || value.window_count > 256 || !Number.isInteger(active) || active < 0 || active >= value.window_count
        || !r || ![r.x, r.y, r.width, r.height].every(Number.isFinite) || r.x < 0 || r.y < 0 || r.width <= 0 || r.height <= 0
        || r.x + r.width > 1 + 1e-9 || r.y + r.height > 1 + 1e-9) throw new Error('Tabs 预设无效')
      template = { kind: 'tabs', data: { region: { x: r.x, y: r.y, width: r.width, height: r.height }, active } }
    } else if (value.template?.kind === 'layout') {
      const tree = regions(value.template.data, new Set())
      if (value.window_count > regionCount(tree)) throw new Error('窗口数无效')
      template = { kind: 'layout', data: tree }
    } else throw new Error('预设类型无效')
    return { id: value.id, note: value.note, window_count: value.window_count, template }
  }).sort((a, b) => a.id - b.id)
}

const FILE_HEADER = [75, 83, 87, 79, 82, 75, 83, 80, 1]
/** Identical to app/preset_store/codec.rs; browser storage remains independent. */
export function encodeWorkspaceFile(layouts: SavedWindowPreset[]): Uint8Array {
  const checked = validateSavedPresets(layouts)
  const output = [...FILE_HEADER, checked.length], encoder = new TextEncoder()
  function integer(value: number, length: number): void { for (let i = 0; i < length; i++) output.push((value >>> (i * 8)) & 255) }
  function decimal(value: number): void { const bytes = new Uint8Array(8); new DataView(bytes.buffer).setFloat64(0, value, true); output.push(...bytes) }
  function tree(node: RegionTemplate): void {
    if (node.kind === 'slot') { output.push(0); integer(node.id, 4); return }
    output.push(node.axis === 'x' ? 1 : 2)
    const number = new Uint8Array(8); new DataView(number.buffer).setFloat64(0, node.ratio, true); output.push(...number)
    tree(node.first); tree(node.second)
  }
  for (const layout of checked) {
    output.push(layout.id); integer(layout.window_count, 2)
    const note = encoder.encode(layout.note); integer(note.length, 2); output.push(...note)
    output.push(layout.template.kind === 'tabs' ? 1 : 0)
    if (layout.template.kind === 'tabs') {
      const r = layout.template.data.region
      for (const value of [r.x, r.y, r.width, r.height]) decimal(value)
      integer(layout.template.data.active, 2)
    } else tree(layout.template.data)
  }
  return new Uint8Array(output)
}
export function decodeWorkspaceFile(bytes: Uint8Array): SavedWindowPreset[] {
  if (bytes.length > 1024 * 1024) throw new Error('工作区文件过大')
  let offset = 0
  function take(count: number): Uint8Array {
    if (offset + count > bytes.length) throw new Error('工作区文件不完整')
    const value = bytes.subarray(offset, offset + count); offset += count; return value
  }
  function integer(length: number): number { return take(length).reduce((value, byte, index) => value + byte * 2 ** (index * 8), 0) }
  if (!take(8).every((byte, index) => byte === FILE_HEADER[index])) throw new Error('不支持此工作区文件版本')
  const version = integer(1)
  if (version !== 1) throw new Error('不支持此工作区文件版本')
  const count = integer(1)
  if (count > 99) throw new Error('预设数量过多')
  function tree(depth: number, budget: { nodes: number }): RegionTemplate {
    if (depth > 32 || ++budget.nodes > 511) throw new Error('区域结构超出限制')
    const tag = integer(1)
    if (tag === 0) return { kind: 'slot', id: integer(4) }
    if (tag !== 1 && tag !== 2) throw new Error('区域结构无效')
    const number = take(8), ratio = new DataView(number.buffer, number.byteOffset, 8).getFloat64(0, true)
    if (!Number.isFinite(ratio) || ratio <= 0 || ratio >= 1) throw new Error('分割比例无效')
    return { kind: 'split', axis: tag === 1 ? 'x' : 'y', ratio, first: tree(depth + 1, budget), second: tree(depth + 1, budget) }
  }
  const layouts: SavedWindowPreset[] = [], decoder = new TextDecoder('utf-8', { fatal: true })
  for (let i = 0; i < count; i++) {
    const id = integer(1), window_count = integer(2), length = integer(2)
    if (length > 320) throw new Error('备注过长')
    const note = decoder.decode(take(length)), tag = integer(1)
    if (tag === 0) layouts.push({ id, window_count, note, template: { kind: 'layout', data: tree(0, { nodes: 0 }) } })
    else if (tag === 1) {
      const decimal = () => { const bytes = take(8); return new DataView(bytes.buffer, bytes.byteOffset, 8).getFloat64(0, true) }
      const region = { x: decimal(), y: decimal(), width: decimal(), height: decimal() }, active = integer(2)
      layouts.push({ id, window_count, note, template: { kind: 'tabs', data: { region, active } } })
    } else throw new Error('预设类型无效')
  }
  if (offset !== bytes.length) throw new Error('工作区文件含有多余数据')
  return validateSavedPresets(layouts)
}
