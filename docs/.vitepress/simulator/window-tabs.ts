/** Browser counterpart of the worker's persistent group model. */
import type { SimulatorState } from './state.ts'
import type { DemoWindow, WindowState } from './window.ts'
import type { SavedWindowPreset } from './window-presets.ts'

export interface DemoTabGroup { id: number; members: number[]; active: number }
export type DemoTabTarget = { kind: 'window' | 'group'; id: number }
interface TabCheckpoint { groups: DemoTabGroup[]; windows: DemoWindow[]; active: number | null }
export interface DemoTabs {
  groups: DemoTabGroup[]; target: DemoTabTarget | null; nextId: number
  history: TabCheckpoint[]; redo: TabCheckpoint[]
  restore: { layout: SavedWindowPreset; members: number[] } | null
}
const copy = <T>(value: T): T => JSON.parse(JSON.stringify(value)) as T
export function createDemoTabs(): DemoTabs { return { groups: [], target: null, nextId: 1, history: [], redo: [], restore: null } }
export function containingTab(w: WindowState, id: number): DemoTabGroup | undefined { return w.tabs.groups.find(g => g.members.includes(id)) }
export function currentTabGroup(w: WindowState): DemoTabGroup | undefined {
  const target = w.tabs.target
  return target?.kind === 'group' ? w.tabs.groups.find(g => g.id === target.id) : containingTab(w, target?.id ?? w.target ?? -1)
}
function checkpoint(w: WindowState): TabCheckpoint { return { groups: copy(w.tabs.groups), windows: copy(w.windows), active: w.target } }
function remember(w: WindowState, before: TabCheckpoint): void {
  if (JSON.stringify(before.groups.map(g => [g.id, g.members])) === JSON.stringify(w.tabs.groups.map(g => [g.id, g.members]))) return
  if (w.tabs.history.length === 32) w.tabs.history.shift()
  w.tabs.history.push(before); w.tabs.redo = []
}
function removeMember(group: DemoTabGroup, id: number): void {
  const index = group.members.indexOf(id)
  if (index < 0) return
  group.members.splice(index, 1)
  if (group.active === id && group.members.length) group.active = group.members[Math.min(index, group.members.length - 1)]
}
export function pruneTabGroups(w: WindowState): void {
  for (const group of w.tabs.groups) {
    for (const id of [...group.members]) if (!w.windows.some(v => v.id === id && !v.fullscreen)) removeMember(group, id)
  }
  w.tabs.groups = w.tabs.groups.filter(g => g.members.length >= 2)
  const target = w.tabs.target
  if (target?.kind === 'group' && !w.tabs.groups.some(g => g.id === target.id) || target?.kind === 'window' && !w.windows.some(v => v.id === target.id)) w.tabs.target = null
}
/** Copy only geometry/state; identities and application content stay with each window. */
export function tabFrame(window: DemoWindow) {
  return { x: window.x, y: window.y, width: window.width, height: window.height, screen: window.screen,
    minimized: window.minimized, restored: window.restored && { ...window.restored } }
}
export function activeTabWindow(w: WindowState, id: number): DemoWindow | undefined {
  return w.windows.find(v => v.id === (containingTab(w, id)?.active ?? id))
}
export function activateTab(state: SimulatorState, id: number): void {
  const w = state.window, window = w.windows.find(v => v.id === id)
  if (!window) return
  const group = containingTab(w, id)
  if (group && group.active !== id) {
    const previous = activeTabWindow(w, id)
    if (previous) Object.assign(window, tabFrame(previous))
    group.active = id
  }
  if (window.minimized) window.minimized = false
  w.target = id; w.screen = window.screen
  w.recent = [id, ...w.recent.filter(v => v !== id)]
  state.pointer = { x: (window.x + window.width / 2) / 10, y: (window.y + window.height / 2) / 6.5 }
}
function align(w: WindowState, id: number): void {
  const source = w.windows.find(v => v.id === id), group = containingTab(w, id)
  if (!source || !group) return
  const members = group.members.map(id => w.windows.find(v => v.id === id)!)
  if (members.some(v => !v || v.fullscreen || v.resizable === false)) throw new Error('只支持普通可缩放窗口')
  const width = Math.max(source.width, ...members.map(v => v.minWidth ?? 100)), height = Math.max(source.height, ...members.map(v => v.minHeight ?? 80))
  if (width > 1000 || height > 650) throw new Error('成员最小尺寸无法放入当前屏幕')
  Object.assign(activeTabWindow(w, id)!, { screen: source.screen, width, height, x: Math.max(0, Math.min(1000 - width, source.x)), y: Math.max(0, Math.min(650 - height, source.y)), minimized: false, restored: undefined })
}
function restoreCheckpoint(w: WindowState, before: TabCheckpoint): void {
  w.windows = w.windows.map(v => copy(before.windows.find(old => old.id === v.id) ?? v))
  w.tabs.groups = copy(before.groups); w.target = before.active; w.tabs.target = null
  pruneTabGroups(w)
}
export function chooseTabTarget(state: SimulatorState, selected: DemoTabTarget, record = true): boolean {
  const w = state.window, oldGroup = selected.kind === 'group' ? w.tabs.groups.find(g => g.id === selected.id) : undefined
  const incoming = selected.kind === 'window' ? [selected.id] : oldGroup?.members.slice()
  if (!incoming?.length || incoming.some(id => !w.windows.some(v => v.id === id && v.resizable !== false && !v.fullscreen))) { state.lastEvent = '窗口或标签组不可用'; return false }
  const active = oldGroup?.active ?? selected.id
  const before = checkpoint(w), previousTarget = copy(w.tabs.target)
  if (!w.tabs.target) {
    const existing = selected.kind === 'window' ? containingTab(w, selected.id) : oldGroup
    w.tabs.target = existing ? { kind: 'group', id: existing.id } : selected
    activateTab(state, active); state.lastEvent = '继续输入窗口编号加入，结束本轮后开始下一组'; return true
  }
  const current = w.tabs.target
  let group = current.kind === 'group' ? w.tabs.groups.find(g => g.id === current.id) : undefined
  const base = group?.members.slice() ?? [current.id], anchor = group?.active ?? current.id
  if (incoming.every(id => base.includes(id))) { activateTab(state, active); return true }
  if (new Set([...base, ...incoming]).size > 256) { state.lastEvent = '每组最多 256 个窗口'; return false }
  if (!group) {
    let id = 1
    while (w.tabs.groups.some(g => g.id === id)) id++
    w.tabs.nextId = Math.max(w.tabs.nextId, id + 1)
    group = { id, members: base, active: base[0] }; w.tabs.groups.push(group)
  }
  for (const other of w.tabs.groups) if (other.id !== group.id) for (const id of incoming) removeMember(other, id)
  for (const id of incoming) if (!group.members.includes(id)) group.members.push(id)
  w.tabs.target = { kind: 'group', id: group.id }; group.active = active
  pruneTabGroups(w)
  try { align(w, anchor); activateTab(state, active) }
  catch (error) { restoreCheckpoint(w, before); w.tabs.target = previousTarget; state.lastEvent = String(error); return false }
  if (record) remember(w, before)
  state.lastEvent = `正在组合 ~${group.id}：${group.members.map(id => w.numbers[id]).join('、')}`
  return true
}
export function autoGroupTabs(state: SimulatorState): void {
  const w = state.window, before = checkpoint(w), active = w.target
  w.tabs.target = null
  const apps = new Map<string, number[]>()
  for (const window of w.windows) {
    if ((w.screens !== 'all' && window.screen !== w.screen) || (!w.includeMinimized && window.minimized) || window.fullscreen || window.resizable === false || containingTab(w, window.id)) continue
    const key = `${window.screen}:${window.app.toLowerCase()}`
    apps.set(key, [...apps.get(key) ?? [], window.id])
  }
  for (const members of apps.values()) {
    if (members.length < 2) continue
    w.tabs.target = null
    for (const id of members) if (!chooseTabTarget(state, { kind: 'window', id }, false)) {
      restoreCheckpoint(w, before); w.tabs.target = null; return
    }
  }
  w.tabs.target = null
  if (active !== null) activateTab(state, active)
  remember(w, before)
  state.lastEvent = '已整理同应用窗口 · 输入编号开始分组'
}
export function tabAction(state: SimulatorState, action: string): boolean {
  const w = state.window, tabs = w.tabs
  if (tabs.restore) {
    if (action === 'window_tab_remove' || action === 'window_undo') tabs.restore.members.pop()
    else if (action === 'window_tab_end') tabs.restore.members = []
    else if (action === 'window_tab_dissolve') tabs.restore = null
    else state.lastEvent = '请先选择模板所需的窗口'
    return true
  }
  if (action === 'window_tab_end') { tabs.target = null; state.lastEvent = '输入窗口编号开始下一组'; return true }
  if (action === 'window_undo' || action === 'window_redo') {
    const redo = action === 'window_redo', source = redo ? tabs.redo : tabs.history, desired = source.pop()
    if (!desired) { state.lastEvent = redo ? '没有可重做的分组操作' : '没有可撤销的分组操作'; return true }
    const destination = redo ? tabs.history : tabs.redo
    if (destination.length === 32) destination.shift()
    destination.push(checkpoint(w)); restoreCheckpoint(w, desired)
    state.lastEvent = redo ? '已重做分组操作' : '已撤销分组操作'; return true
  }
  const group = currentTabGroup(w)
  if (!group) { state.lastEvent = '请先选择标签组'; return true }
  if (action === 'window_save_layout') { w.noteOpen = true; return true }
  const before = checkpoint(w)
  if (action === 'window_tab_dissolve') { tabs.groups = tabs.groups.filter(g => g.id !== group.id); tabs.target = null }
  else if (action === 'window_tab_remove') {
    removeMember(group, group.active); tabs.target = { kind: 'group', id: group.id }; activateTab(state, group.active)
  } else if (['window_tab_next', 'window_tab_previous', 'window_tab_move_left', 'window_tab_move_right'].includes(action)) {
    const index = group.members.indexOf(group.active), backwards = action.endsWith('previous') || action.endsWith('left'), next = (index + (backwards ? group.members.length - 1 : 1)) % group.members.length
    if (action.includes('_move_')) [group.members[index], group.members[next]] = [group.members[next], group.members[index]]
    else { activateTab(state, group.members[next]); return true }
  } else return false
  pruneTabGroups(w); remember(w, before)
  state.lastEvent = '标签组已更新'; return true
}
export function selectTemplateMember(state: SimulatorState, id: number): void {
  const w = state.window, restore = w.tabs.restore
  if (!restore || restore.layout.template.kind !== 'tabs' || !w.windows.some(v => v.id === id && v.resizable !== false && !v.fullscreen)) { state.lastEvent = '请选择可缩放窗口'; return }
  if (restore.members.includes(id) || restore.members.length >= restore.layout.window_count) { state.lastEvent = '窗口已选择，可移除后重新选择'; return }
  restore.members.push(id)
  if (restore.members.length !== restore.layout.window_count) return
  const before = checkpoint(w), members = restore.members.slice(), template = restore.layout.template.data
  for (const group of w.tabs.groups) for (const id of members) removeMember(group, id)
  pruneTabGroups(w); w.tabs.target = null
  try {
    for (const id of members) if (!chooseTabTarget(state, { kind: 'window', id }, false)) throw new Error(state.lastEvent)
    const first = w.windows.find(v => v.id === members[template.active])!
    containingTab(w, first.id)!.active = first.id
    Object.assign(first, { x: template.region.x * 1000, y: template.region.y * 650, width: template.region.width * 1000, height: template.region.height * 650, screen: w.screen })
    align(w, first.id); activateTab(state, members[template.active])
  } catch (error) { restoreCheckpoint(w, before); state.lastEvent = String(error); return }
  w.tabs.restore = null; w.tabs.target = null; remember(w, before)
  state.lastEvent = '已恢复 Tab 模板'
}
export function tabDetail(w: WindowState): string {
  if (w.tabs.restore) return `恢复 Tabs · ${w.tabs.restore.members.length} / ${w.tabs.restore.layout.window_count} 个窗口`
  const target = w.tabs.target
  return target?.kind === 'group' ? `Tabs · 正在组合 ~${target.id}` : target ? `Tabs · 起点 ${w.numbers[target.id]}` : 'Tabs · 输入窗口编号开始分组'
}
