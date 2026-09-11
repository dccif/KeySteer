import type { SimulatorMode, SimulatorState } from './state'
import { automaticTree, fitTree, importTree, layoutRect, moveToSlot, navigateSlot, quickCaption, quickRect, quickStep, resizeRegionBy, removeSlot, retainTreeWindows, splitSlot, treeSlots } from './window-layout.ts'
import type { LayoutDirection, LayoutTree, QuickPlacement } from './window-layout.ts'
import { instantiateLayout, regionTemplate, presetName } from './window-presets.ts'
import { parseSplitRatios } from './window-ratios.ts'
import type { SavedWindowPreset, WindowTemplate } from './window-presets.ts'
import { activateTab, autoGroupTabs, chooseTabTarget, containingTab, createDemoTabs, currentTabGroup, pruneTabGroups, selectTemplateMember, activeTabWindow, tabFrame, tabAction, tabDetail } from './window-tabs.ts'
import type { DemoTabs } from './window-tabs.ts'

export interface WindowRect { x: number; y: number; width: number; height: number }
export interface DemoWindow extends WindowRect {
  id: number
  title: string
  app: string
  screen: number
  minimized?: boolean
  restored?: WindowRect
  minWidth?: number
  minHeight?: number
  resizable?: boolean
  fullscreen?: boolean
}
export type WindowMode = 'window' | 'window_quick' | 'window_editor' | 'window_restore' | 'window_tab'
export function isWindowMode(mode: string): mode is WindowMode { return ['window', 'window_quick', 'window_editor', 'window_restore', 'window_tab'].includes(mode) }

export interface WindowState {
  screens: 'current' | 'all'
  includeMinimized: boolean
  tabs: DemoTabs
  mode: WindowMode
  deletingPresets: boolean
  deleteSelection: SavedWindowPreset | null
  presets: SavedWindowPreset[]
  library: boolean
  libraryPage: number
  libraryIndex: Map<string, { exact?: number; longer: boolean }>
  noteOpen: boolean
  editingPresetId: number | null
  savedEditSnapshot: string
  recent: number[]
  windows: DemoWindow[]
  target: number | null
  screen: number
  size: boolean
  temporary: boolean
  panel: 'none' | 'quick' | 'tree'
  ratios: readonly number[]
  quick: QuickPlacement
  tree: LayoutTree | null
  trees: Record<number, LayoutTree>
  editBefore: DemoWindow[] | null
  editHistory: Array<{ quick: QuickPlacement; tree: LayoutTree | null; windows: DemoWindow[] }>
  editRedo: WindowState['editHistory']
  initial: DemoWindow[]
  changedWindows: number[]
  redo: WindowState['history']
  numbers: Record<number, number>
  nextNumber: number
  numberDisplay: string
  numberPrefix: string
  numberSlot: boolean
  numberDeadline: number | null
  windowIndex: Map<string, { exact?: number; longer: boolean }>
  slotIndex: Map<string, { exact?: number; longer: boolean }>
  swapSource: number | null
  previous: SimulatorMode
  gesture: boolean
  group: number
  history: Array<{ group: number; windows: DemoWindow[] }>
}

// Coordinates represent a 1000 × 650 logical-pixel demo workspace per screen.
export const WINDOW_AREA = { width: 1000, height: 650 }
export const WINDOW_MOTION = new Set(['window_left', 'window_down', 'window_up', 'window_right', 'window_ratio_left', 'window_ratio_down', 'window_ratio_up', 'window_ratio_right'])


export function createWindowState(): WindowState {
  return {
    screens: 'current', includeMinimized: false,
    tabs: createDemoTabs(),
    mode: 'window', deletingPresets: false, deleteSelection: null, presets: [], library: false, libraryPage: 0, libraryIndex: new Map(), noteOpen: false, recent: [], editingPresetId: null, savedEditSnapshot: '',
    windows: [
      { id: 1, title: '项目笔记', app: 'Notes', screen: 0, x: 120, y: 95, width: 430, height: 330 },
      { id: 2, title: 'KeySteer 文档', app: 'Browser', screen: 0, x: 420, y: 180, width: 450, height: 330 },
      { id: 3, title: '文件', app: 'Files', screen: 0, x: 55, y: 340, width: 350, height: 250 },
      { id: 4, title: '终端', app: 'Terminal', screen: 1, x: 180, y: 130, width: 590, height: 380 },
    ], target: null, screen: 0, size: false, temporary: false, panel: 'none',
    ratios: [.25, 1/3, .5, 2/3, .75, 1], quick: { horizontal: null, vertical: null }, tree: null, trees: {}, editBefore: null, editHistory: [], editRedo: [], initial: [], changedWindows: [], redo: [],
    numbers: {}, nextNumber: 1, numberDisplay: '', numberPrefix: '', numberSlot: false, numberDeadline: null, windowIndex: new Map(), slotIndex: new Map(), swapSource: null,
    previous: 'normal', gesture: false, group: 0, history: [],
  }
}

export function windowTarget(state: WindowState): DemoWindow | undefined {
  return state.target === null ? undefined : activeTabWindow(state, state.target)
}

export function setDemoWindowCount(state: SimulatorState, count: number): void {
  count = Math.max(1, Math.min(30, Math.floor(count)))
  const w = state.window, screen = w.screen
  w.tabs = createDemoTabs()
  w.windows = Array.from({ length: count }, (_, i) => ({ id: i + 1, title: `示例窗口 ${i + 1}`, app: i % 3 === 0 ? 'Notes' : i % 3 === 1 ? 'Browser' : 'Files', screen,
    x: 35 + i % 6 * 112, y: 70 + Math.floor(i / 6) * 86, width: 300, height: 170 }))
  w.windows.push({ id: count + 1, title: '另一屏幕', app: 'Terminal', screen: (screen + 1) % 2, x: 180, y: 130, width: 590, height: 380 })
  state.mode = w.previous; enterWindow(state)
  if (w.target === null) selectWindow(state, 1)
  state.lastEvent = `${count} 个示例窗口 · 仅歧义编号等待第二位`
}

export function enterWindow(state: SimulatorState, settings: Record<string, any> = {}): void {
  const w = state.window
  if (isWindowMode(state.mode)) return
  w.previous = state.mode
  w.screens = settings.screens === 'all' ? 'all' : 'current'; w.includeMinimized = settings.include_minimized === true
  w.size = false; w.temporary = false; w.history = []; w.redo = []; w.editRedo = []; w.initial = snapshot(w); w.changedWindows = []; w.gesture = false
  w.library = false; w.noteOpen = false
  w.editingPresetId = null; w.savedEditSnapshot = ''
  const x = state.pointer.x * WINDOW_AREA.width / 100, y = state.pointer.y * WINDOW_AREA.height / 100
  w.target = [...w.windows].reverse().find(v => (!containingTab(w, v.id) || containingTab(w, v.id)?.active === v.id) && !v.minimized && v.screen === w.screen && x >= v.x && x <= v.x + v.width && y >= v.y && y <= v.y + v.height)?.id ?? null
  w.panel = 'none'; w.tree = null; w.trees = {}; w.editBefore = null; w.editHistory = []
  w.numbers = {}; w.nextNumber = 1
  cancelWindowNumber(w); w.swapSource = null
  // The locked window gets the first stable number as the native acquire
  // result arrives before the asynchronous inventory.
  if (w.target !== null) w.numbers[w.target] ??= w.nextNumber++
  refreshWindowNumbers(w)
  state.mode = 'window'; w.mode = 'window'; state.lastEvent = w.target == null ? 'Tab 切换到示例窗口' : '已锁定示例窗口'
}

export function leaveWindow(state: SimulatorState): void {
  const w = state.window
  w.tabs.target = null; w.tabs.restore = null
  if (w.panel !== 'none') endWindowEdit(state, true)
  w.library = false; w.noteOpen = false; w.deleteSelection = null
  w.history = []; w.redo = []; w.editRedo = []; w.initial = []; w.changedWindows = []; w.panel = 'none'; w.gesture = false; w.temporary = false
  cancelWindowNumber(w); w.trees = {}
}

export function switchWindowMode(state: SimulatorState, mode: WindowMode, settings: Record<string, any> = {}): void {
  if (settings.enabled === false) return
  if (!isWindowMode(state.mode)) enterWindow(state, settings)
  const w = state.window
  if (w.mode === 'window_tab' && mode !== 'window_tab') { w.tabs.target = null; w.tabs.restore = null }
  const scope = settings.screens === 'all' ? 'all' : 'current', includeMinimized = settings.include_minimized === true
  if (w.screens !== scope || w.includeMinimized !== includeMinimized) { w.numbers = {}; w.nextNumber = 1 }
  w.screens = scope; w.includeMinimized = includeMinimized
  refreshWindowNumbers(w)
  const library = mode === 'window_restore'
  if (!library && !(mode === 'window_editor' && w.panel === 'tree') && w.panel !== 'none') endWindowEdit(state, true)
  if (mode === 'window_quick') w.ratios = [...parseSplitRatios(settings.split_ratios), 1]
  state.mode = mode; w.mode = mode; w.library = library
  w.noteOpen = false; w.deleteSelection = null; w.deletingPresets = false; w.temporary = false; w.gesture = false
  cancelWindowNumber(w); w.swapSource = null
  if (library) { w.libraryPage = 0; w.libraryIndex = numberIndex(availableWindowPresets(w).map(p => p.id)) }
  else if (mode === 'window_quick' && w.panel === 'none') startWindowEdit(state, false, settings)
  else if (mode === 'window_editor' && w.panel === 'none') startWindowEdit(state, true, settings)
  else if (mode === 'window_tab' && !w.tabs.restore) autoGroupTabs(state)
  state.lastEvent = `进入 ${mode}`
}

export function temporaryWindow(state: WindowState, active: boolean, settings: Record<string, any> = {}, now = Date.now()): void {
  if (state.temporary === active) return
  state.temporary = active; state.gesture = false
  state.numberDeadline = !active && (state.library ? state.libraryIndex : state.numberSlot ? state.slotIndex : state.windowIndex).get(state.numberPrefix)?.longer
    ? now + settingsNumber(settings, 'number_timeout_ms', 250) : null
}

function settingsNumber(settings: Record<string, any>, key: string, fallback: number): number {
  const value = Number(settings[key] ?? fallback)
  return Number.isFinite(value) ? Math.max(0, value) : fallback
}

function snapshot(state: WindowState): DemoWindow[] {
  return state.windows.map(w => ({ ...w, ...tabFrame(activeTabWindow(state, w.id) ?? w) }))
}

function changed(state: WindowState, before: DemoWindow[]): void {
  const current = snapshot(state)
  const changed = before.filter(old => {
    const after = current.find(w => w.id === old.id)
    return after && JSON.stringify(old) !== JSON.stringify(after)
  })
  if (!changed.length) return
  for (const old of changed) {
    if (!state.initial.some(w => w.id === old.id)) state.initial.push(copy(old))
    if (!state.changedWindows.includes(old.id)) state.changedWindows.push(old.id)
  }
  state.redo = []
  const last = state.history.at(-1)
  if (last?.group === state.group) {
    changed.forEach(old => { if (!last.windows.some(w => w.id === old.id)) last.windows.push(old) })
    return
  }
  if (state.history.length === 32) state.history.shift()
  state.history.push({ group: state.group, windows: changed })
}

function inset(rect: WindowRect, gap: number): WindowRect {
  const half = Math.min(Math.max(0, gap / 2), Math.min(rect.width, rect.height) / 4)
  return { x: rect.x + half, y: rect.y + half, width: rect.width - 2 * half, height: rect.height - 2 * half }
}

export function tileWindows(count: number, gap: number): WindowRect[] {
  if (count <= 0) return []
  const { width, height } = WINDOW_AREA
  let bestRows = 1, bestScore = Infinity
  for (let rows = 1; rows <= count; rows++) {
    let score = 0
    for (let row = 0; row < rows; row++) {
      const columns = Math.floor(count / rows) + Number(row < count % rows)
      score += Math.abs(Math.log((width / columns) / (height * columns / count) / 1.6)) * columns
    }
    if (score < bestScore) { bestRows = rows; bestScore = score }
  }
  const areas: WindowRect[] = []
  let y = 0
  for (let row = 0; row < bestRows; row++) {
    const columns = Math.floor(count / bestRows) + Number(row < count % bestRows)
    const h = height * columns / count
    for (let col = 0; col < columns; col++) areas.push(inset({ x: col * width / columns, y, width: width / columns, height: h }, gap))
    y += h
  }
  return areas
}

function followPointer(state: SimulatorState, before: WindowRect, after: WindowRect): void {
  state.pointer.x = Math.min(100, Math.max(0, (after.x + state.pointer.x * WINDOW_AREA.width / 100 - before.x) / WINDOW_AREA.width * 100))
  state.pointer.y = Math.min(100, Math.max(0, (after.y + state.pointer.y * WINDOW_AREA.height / 100 - before.y) / WINDOW_AREA.height * 100))
}

const copy = <T>(value: T): T => JSON.parse(JSON.stringify(value)) as T

function numberIndex(values: number[]): WindowState['windowIndex'] {
  const index: WindowState['windowIndex'] = new Map()
  for (const value of values) {
    const label = String(value)
    for (let length = 1; length <= label.length; length++) {
      const key = label.slice(0, length), prefix = index.get(key) ?? { longer: false }
      if (length === label.length) prefix.exact = value; else prefix.longer = true
      index.set(key, prefix)
    }
  }
  return index
}
export function eligibleWindow(w: WindowState, window: DemoWindow): boolean {
  return (w.includeMinimized || !window.minimized) && (w.screens === 'all' || window.screen === w.screen)
}
export function refreshWindowNumbers(w: WindowState): void {
  pruneTabGroups(w)
  const windows = snapshot(w).filter(v => eligibleWindow(w, v))
  // Minimized/out-of-scope windows keep their reservation until they close.
  for (const id of Object.keys(w.numbers)) if (!w.windows.some(v => v.id === Number(id))) delete w.numbers[Number(id)]
  const first = new Map<string, number>()
  for (const window of windows) if (w.numbers[window.id] !== undefined) {
    const app = window.app.toLowerCase()
    first.set(app, Math.min(first.get(app) ?? Infinity, w.numbers[window.id]))
  }
  windows.sort((a, b) => (first.get(a.app.toLowerCase()) ?? Infinity) - (first.get(b.app.toLowerCase()) ?? Infinity) || a.app.toLowerCase().localeCompare(b.app.toLowerCase()))
  windows.forEach(v => { w.numbers[v.id] ??= w.nextNumber++ })
  w.windowIndex = numberIndex(windows.map(v => w.numbers[v.id]))
  if (w.tree) retainTreeWindows(w.tree, w.windows.map(v => v.id))
  w.slotIndex = numberIndex(w.mode === 'window_tab' ? w.tabs.groups.filter(g => windows.some(v => v.id === g.active)).map(g => g.id) : w.tree ? treeSlots(w.tree).map(s => s.id) : [])
}
export function cancelWindowNumber(w: WindowState): boolean {
  const pending = Boolean(w.numberPrefix || w.numberSlot)
  w.numberDisplay = ''; w.numberPrefix = ''; w.numberSlot = false; w.numberDeadline = null
  return pending
}
export function finishWindowNumber(state: SimulatorState, settings: Record<string, any> = {}): void {
  const w = state.window, slot = w.numberSlot
  if (w.library) { const id = w.libraryIndex.get(w.numberPrefix)?.exact; cancelWindowNumber(w); if (id !== undefined) restoreWindowPreset(state, id, settings); return }
  const value = (slot ? w.slotIndex : w.windowIndex).get(w.numberPrefix)?.exact
  const display = w.numberDisplay
  cancelWindowNumber(w)
  w.numberDisplay = display
  if (value !== undefined) chooseWindowNumber(state, value, settings, slot)
  else if (w.mode === 'window_tab' && display) state.lastEvent = '编号无效，当前组合保持不变'
}
export function windowSelectionKey(state: SimulatorState, key: string, settings: Record<string, any>, now = Date.now()): boolean {
  const w = state.window
  if (!isWindowMode(state.mode) || w.temporary) return false
  if (w.library) {
    if (key === 'page_down') { w.libraryPage = Math.min(w.libraryPage + 1, Math.floor(Math.max(0, availableWindowPresets(w).length - 1) / 6)); return true }
    if (key === 'page_up') { w.libraryPage = Math.max(0, w.libraryPage - 1); return true }
    if (!/^\d$/.test(key)) return false
    w.deleteSelection = null
    if (w.numberPrefix && !w.libraryIndex.has(w.numberPrefix + key)) finishWindowNumber(state, settings)
    if (!w.library) return true
    w.numberPrefix += key; w.numberDisplay = w.numberPrefix
    const prefix = w.libraryIndex.get(w.numberPrefix)
    if (!prefix) cancelWindowNumber(w)
    else if (prefix.longer) w.numberDeadline = now + settingsNumber(settings, 'number_timeout_ms', 250)
    else finishWindowNumber(state, settings)
    return true
  }
  if (key === '`' && state.mode === 'window_editor') {
    if (w.panel === 'quick') endWindowEdit(state, true)
    if (w.panel === 'none') startWindowEdit(state, true, settings)
    cancelWindowNumber(w); w.numberSlot = true; w.numberDisplay = '`'; return true
  }
  if (!/^\d$/.test(key)) return false

  let index = w.numberSlot ? w.slotIndex : w.windowIndex
  if (w.numberPrefix && !index.has(w.numberPrefix + key)) { finishWindowNumber(state, settings); index = w.windowIndex }
  w.numberPrefix += key
  w.numberDisplay = (w.numberSlot ? w.mode === 'window_tab' ? '~' : '`' : '') + w.numberPrefix
  const prefix = index.get(w.numberPrefix)
  if (!prefix) { cancelWindowNumber(w); if (w.mode === 'window_tab') state.lastEvent = '编号无效，当前组合保持不变' }
  else if (prefix.longer) w.numberDeadline = now + settingsNumber(settings, 'number_timeout_ms', 250)
  else finishWindowNumber(state, settings)
  return true
}
function restoreWindows(w: WindowState, before: DemoWindow[]): void {
  // Closed windows stay closed; new windows are not removed by rollback.
  for (const old of before) {
    const current = activeTabWindow(w, old.id)
    if (current) {
      Object.assign(current, tabFrame(old))
      if (old.minimized === undefined) delete current.minimized
      if (old.restored === undefined) delete current.restored
    }
  }
}
function layoutWindows(w: WindowState): DemoWindow[] {
  return w.windows.filter(v => !containingTab(w, v.id) || containingTab(w, v.id)?.members[0] === v.id).map(v => {
    const members = containingTab(w, v.id)?.members.map(id => w.windows.find(v => v.id === id)!) ?? [v]
    return { ...v, ...tabFrame(activeTabWindow(w, v.id) ?? v), minWidth: Math.max(...members.map(v => v.minWidth ?? 100)), minHeight: Math.max(...members.map(v => v.minHeight ?? 80)) + (members.length > 1 ? 30 : 0) }
  })
}
function startWindowEdit(state: SimulatorState, tree: boolean, settings: Record<string, any>, automatic = true): void {
  const w = state.window, target = windowTarget(w)
  if (!target && !tree) { state.lastEvent = '请先选择窗口'; return }
  if (!tree && (target?.resizable === false || target?.fullscreen)) { state.lastEvent = '此窗口不可调整尺寸'; return }
  w.screen = target?.screen ?? w.screen; w.editBefore = snapshot(w); w.editHistory = []; w.editRedo = []; w.swapSource = null
  w.editingPresetId = null; w.savedEditSnapshot = ''
  const windows = layoutWindows(w).filter(v => (w.includeMinimized || !v.minimized) && v.screen === w.screen && v.resizable !== false && !v.fullscreen).sort((a, b) => w.numbers[a.id] - w.numbers[b.id])
  if (tree) {
    let cached = w.trees[w.screen] ? copy(w.trees[w.screen]) : null
    if (cached) {
      retainTreeWindows(cached, windows.map(v => v.id))
      const slots = treeSlots(cached)
      const same = windows.every(v => slots.some(s => s.window === v.id)) && slots.every(slot => {
        if (slot.window === null) return true
        const current = windows.find(v => v.id === slot.window), expected = layoutRect(slot.rect, WINDOW_AREA, settingsNumber(settings, 'gap', 0))
        return current && (['x', 'y', 'width', 'height'] as const).every(k => Math.abs(current[k] - expected[k]) < 2)
      })
      if (!same) cached = null
    }
    w.tree = !automatic ? importTree(windows, (target ? containingTab(w, target.id)?.members[0] ?? target.id : null), WINDOW_AREA) : cached ?? automaticTree(windows, (target ? containingTab(w, target.id)?.members[0] ?? target.id : null), WINDOW_AREA, settingsNumber(settings, 'gap', 0)) ?? importTree(windows, (target ? containingTab(w, target.id)?.members[0] ?? target.id : null), WINDOW_AREA)
    w.panel = 'tree'
    if (automatic && fitTree(w.tree, windows, WINDOW_AREA, settingsNumber(settings, 'gap', 0))) {
      const extra = new Map<number, LayoutTree>()
      if (w.screens === 'all') for (const screen of new Set(w.windows.map(v => v.screen))) {
        if (screen === w.screen) continue
        const members = layoutWindows(w).filter(v => v.screen === screen && eligibleWindow(w, v) && v.resizable !== false && !v.fullscreen)
        if (!members.length) continue
        const layout = automaticTree(members, null, WINDOW_AREA, settingsNumber(settings, 'gap', 0))
        if (!layout || !fitTree(layout, members, WINDOW_AREA, settingsNumber(settings, 'gap', 0))) { state.lastEvent = '窗口最小尺寸无法放入此布局'; return }
        extra.set(screen, layout)
      }
      pushEdit(w, editSnapshot(w)); applyTree(w, settings)
      const main = w.tree
      for (const [screen, tree] of extra) { w.tree = tree; applyTree(w, settings); w.trees[screen] = copy(tree) }
      w.tree = main; state.lastEvent = '已自动布局 · Shift 切分 · Ctrl 移动分割线 · X 删除区域'
    } else if (automatic) state.lastEvent = '窗口最小尺寸无法放入此布局，可删除分区后调整'
  } else { w.quick = { horizontal: null, vertical: null }; w.tree = null; w.panel = 'quick'; state.lastEvent = '按方向选择半屏，再调整比例' }
  refreshWindowNumbers(w)
}
function endWindowEdit(state: SimulatorState, commit: boolean): void {
  const w = state.window
  if (w.editBefore) {
    if (commit) { w.group++; changed(w, w.editBefore); if (w.tree) w.trees[w.screen] = copy(w.tree) }
    else restoreWindows(w, w.editBefore)
  }
  w.panel = 'none'; w.tree = null; w.editBefore = null; w.editHistory = []; w.editRedo = []; w.swapSource = null
  cancelWindowNumber(w); refreshWindowNumbers(w)
  state.lastEvent = commit ? '已保留本轮布局' : '已恢复进入编辑前的布局'
}
function applyTree(w: WindowState, settings: Record<string, any>): void {
  if (!w.tree) return
  for (const slot of treeSlots(w.tree)) {
    const window = slot.window === null ? undefined : activeTabWindow(w, slot.window)
    if (window) { Object.assign(window, layoutRect(slot.rect, WINDOW_AREA, settingsNumber(settings, 'gap', 0)), { restored: undefined, minimized: false }) }
  }
}
function editSnapshot(w: WindowState): WindowState['editHistory'][number] {
  return { quick: copy(w.quick), tree: copy(w.tree), windows: snapshot(w) }
}
function pushEdit(w: WindowState, before: WindowState['editHistory'][number]): void {
  if (w.editHistory.length === 32) w.editHistory.shift()
  w.editHistory.push(before)
  w.editRedo = []; w.redo = []
}
function selectWindow(state: SimulatorState, id: number): void {
  const w = state.window, next = w.windows.find(v => v.id === id)
  if (!next) return
  const previousScreen = w.screen
  activateTab(state, id)
  if (w.panel === 'tree') w.screen = previousScreen
  w.target = id
  w.recent = [id, ...w.recent.filter(v => v !== id)]
  if (w.panel !== 'tree') w.screen = next.screen
  state.pointer.x = (next.x + next.width / 2) / WINDOW_AREA.width * 100
  state.pointer.y = (next.y + next.height / 2) / WINDOW_AREA.height * 100
  state.lastEvent = `切换到 ${next.app} · ${next.title}`
  refreshWindowNumbers(w)
}
export function chooseWindowNumber(state: SimulatorState, number: number, settings: Record<string, any> = {}, slot = false): void {
  const w = state.window

  const target = snapshot(w).find(v => eligibleWindow(w, v) && w.numbers[v.id] === number)
  if (w.mode === 'window_tab') {
    if (w.tabs.restore) { if (!slot && target) selectTemplateMember(state, target.id); else state.lastEvent = '请选择单个窗口编号' }
    else if (slot) chooseTabTarget(state, { kind: 'group', id: number })
    else if (target) chooseTabTarget(state, { kind: 'window', id: target.id })
    else state.lastEvent = '窗口编号无效'
    refreshWindowNumbers(w); return
  }
  if (w.panel === 'tree' && w.tree) {
    const before = editSnapshot(w), tree = w.tree
    let changed = false
    if (slot) {
      if (w.swapSource !== null) { changed = moveToSlot(tree, w.swapSource, number); w.swapSource = null }
      else {
        const region = treeSlots(tree).find(s => s.id === number)
        if (region) {
          tree.selected = number
          if (region.window !== null) selectWindow(state, region.window)
          else { state.pointer.x = (region.rect.x + region.rect.width / 2) * 100; state.pointer.y = (region.rect.y + region.rect.height / 2) * 100 }
        }
      }
    } else if (target) {
      const representative = containingTab(w, target.id)?.members[0] ?? target.id
      const destination = treeSlots(tree).find(s => s.window === representative)
      if (w.swapSource !== null) {
        if (destination && w.swapSource !== representative) changed = moveToSlot(tree, w.swapSource, destination.id)
        w.swapSource = null
      } else {
        selectWindow(state, target.id)
        if (destination) { tree.selected = destination.id; w.swapSource = representative }
      }
    }
    if (changed) {
      if (!fitTree(tree, layoutWindows(w), WINDOW_AREA, settingsNumber(settings, 'gap', 0))) { w.tree = before.tree; state.lastEvent = '窗口最小尺寸无法放入目标区域'; return }
      applyTree(w, settings); pushEdit(w, before); state.lastEvent = '已调整窗口占用'
    }
  } else if (!slot && target) {
    const quick = w.mode === 'window_quick'
    if (quick) endWindowEdit(state, true)
    selectWindow(state, target.id)
    if (quick) startWindowEdit(state, false, settings)
  }
}
function currentPreset(w: WindowState): { template: WindowTemplate; window_count: number } | null {
  if (w.mode === 'window_tab') {
    const group = currentTabGroup(w), active = group && w.windows.find(v => v.id === group.active)
    return group && active ? { window_count: group.members.length, template: { kind: 'tabs', data: {
      active: group.members.indexOf(group.active), region: { x: active.x / 1000, y: active.y / 650, width: active.width / 1000, height: active.height / 650 },
    } } } : null
  }
  return w.tree && w.panel === 'tree' ? { window_count: treeSlots(w.tree).filter(s => s.window !== null).length,
    template: { kind: 'layout', data: regionTemplate(w.tree.root) } } : null
}
export function saveWindowPreset(state: SimulatorState, note: string): void {
  const w = state.window, content = currentPreset(w)
  if (!content) return
  note = note.trim()
  if ([...note].length > 80 || /[\x00-\x1f\x7f-\x9f]/.test(note)) throw new Error('名称最多 80 个字符，不能包含换行')
  const editing = w.presets.find(p => p.id === w.editingPresetId && p.template.kind === content.template.kind)
  const id = editing?.id ?? Array.from({ length: 99 }, (_, i) => i + 1).find(id => !w.presets.some(p => p.id === id))
  if (!id) throw new Error('工作区预设库已满')
  const preset: SavedWindowPreset = { id, note, ...content }
  w.presets = [...w.presets.filter(p => p.id !== id), preset].sort((a, b) => a.id - b.id)
  w.editingPresetId = id; w.savedEditSnapshot = currentLayoutSnapshot(w)
  w.noteOpen = false; state.lastEvent = `已保存 ${presetName(preset)}`
}
export function availableWindowPresets(w: WindowState): SavedWindowPreset[] { return w.presets }
export function restoreWindowPreset(state: SimulatorState, id: number, settings: Record<string, any> = {}): void {
  const w = state.window, preset = availableWindowPresets(w).find(p => p.id === id)
  if (!preset) return
  if (w.deletingPresets) { w.deleteSelection = copy(preset); cancelWindowNumber(w); return }
  if (preset.template.kind === 'tabs') {
    if (w.panel !== 'none') endWindowEdit(state, true)
    w.tabs.restore = { layout: copy(preset), members: [] }; w.tabs.target = null
    w.editingPresetId = id; w.savedEditSnapshot = presetSnapshot(preset)
    switchWindowMode(state, 'window_tab', settings); refreshWindowNumbers(w)
    state.lastEvent = `按顺序选择 ${preset.window_count} 个窗口，选满后自动恢复`; return
  }
  const recent = [...new Set([...(w.target === null ? [] : [w.target]), ...w.recent, ...w.windows.map(v => v.id).reverse()])]
  const targets = layoutWindows(w)
  const windows = recent.map(id => targets.find(v => v.id === id)).filter((v): v is DemoWindow => !!v && v.screen === w.screen && eligibleWindow(w, v))
  const tree = instantiateLayout(preset, windows)
  if (!fitTree(tree, windows, WINDOW_AREA, settingsNumber(settings, 'gap', 0))) { state.lastEvent = '窗口最小尺寸无法放入此布局'; return }
  const extra = new Map<number, LayoutTree>()
  if (w.screens === 'all') for (const screen of new Set(targets.map(v => v.screen))) {
    if (screen === w.screen) continue
    const members = targets.filter(v => v.screen === screen && eligibleWindow(w, v))
    if (!members.length) continue
    const layout = instantiateLayout(preset, members)
    if (!fitTree(layout, members, WINDOW_AREA, settingsNumber(settings, 'gap', 0))) { state.lastEvent = '窗口最小尺寸无法放入此布局'; return }
    extra.set(screen, layout)
  }
  if (w.panel !== 'none') endWindowEdit(state, true)
  cancelWindowNumber(w); w.editBefore = snapshot(w); w.editHistory = []; w.editRedo = []; w.swapSource = null
  w.tree = tree; w.panel = 'tree'
  pushEdit(w, editSnapshot(w)); applyTree(w, settings)
  for (const [screen, layout] of extra) { w.tree = layout; applyTree(w, settings); w.trees[screen] = copy(layout) }
  w.tree = tree; refreshWindowNumbers(w)
  w.editingPresetId = id; w.savedEditSnapshot = currentLayoutSnapshot(w)
  const target = settings.lifecycle?.after_finish ?? 'window_editor'
  if (isWindowMode(target)) switchWindowMode(state, target)
  else if (target !== 'keep') { leaveWindow(state); state.mode = target === 'return' ? w.previous : target }
  state.lastEvent = `已恢复 ${presetName(preset)}`
}
export function deleteWindowPreset(state: SimulatorState): void {
  const w = state.window, expected = w.deleteSelection
  if (!expected) return
  w.deleteSelection = null
  if (JSON.stringify(w.presets.find(p => p.id === expected.id)) !== JSON.stringify(expected)) { state.lastEvent = '布局已改变，请重新选择'; return }
  w.presets = w.presets.filter(p => p.id !== expected.id)
  w.libraryIndex = numberIndex(availableWindowPresets(w).map(p => p.id))
  w.libraryPage = Math.min(w.libraryPage, Math.max(0, Math.ceil(availableWindowPresets(w).length / 6) - 1))
  state.lastEvent = `已删除 ${presetName(expected)}`
}

function presetSnapshot(preset: { window_count: number; template: WindowTemplate }): string {
  const template = preset.template
  if (template.kind === 'tabs') {
    const { x, y, width, height } = template.data.region
    return JSON.stringify([preset.window_count, template.kind, template.data.active, x, y, width, height])
  }
  return JSON.stringify([preset.window_count, template.kind, template.data])
}
function currentLayoutSnapshot(w: WindowState): string { const preset = currentPreset(w); return preset ? presetSnapshot(preset) : '' }
export function hasWindowPresetChanges(w: WindowState): boolean { return currentPreset(w) !== null && currentLayoutSnapshot(w) !== w.savedEditSnapshot }
export function replaceWindowPresets(state: SimulatorState, layouts: SavedWindowPreset[]): void {
  if (state.window.panel !== 'none') endWindowEdit(state, true)
  state.window.noteOpen = false
  state.window.presets = layouts; state.window.libraryIndex = numberIndex(availableWindowPresets(state.window).map(p => p.id)); state.window.libraryPage = 0
  state.window.editingPresetId = null; state.window.savedEditSnapshot = ''; cancelWindowNumber(state.window)
}
export function windowDetail(w: WindowState): string {
  if (w.mode === 'window_tab') return tabDetail(w)
  if (w.library) return w.deletingPresets ? 'Delete layouts' : 'Restore'
  const phase = w.panel === 'quick' ? `Quick · ${quickCaption(w.quick, w.ratios)}` : w.panel === 'tree' ? `Edit · 区域 \`${w.tree?.selected}` : w.mode === 'window_quick' ? 'Quick' : w.mode === 'window_editor' ? 'Edit' : w.size ? 'Resize' : 'Move'
  return phase
}
export function windowInputStatus(w: WindowState): string {
  if (w.tabs.restore) return `已选：${w.tabs.restore.members.map(id => w.numbers[id]).join('、')} · 选满后自动套用`
  if (w.deleteSelection) return `确认删除 ${presetName(w.deleteSelection)}？按 Enter 确认`
  if (w.numberDisplay) return `输入：${w.numberDisplay}${w.numberPrefix || w.numberSlot ? '…' : ''}`
  if (w.swapSource !== null) return `窗口 ${w.numbers[w.swapSource]} → 窗口编号 / 反引号＋区域编号`
  return ''
}
export function windowActionAvailable(w: WindowState, action: string): boolean {
  if ((!action.startsWith('window_') && action !== 'size_cycle') || isWindowMode(action)) return true
  if (w.library) return action === 'window_delete' || action === 'window_confirm' && (!!w.numberPrefix || !!w.deleteSelection)
  if (action === 'window_delete') return false
  if (w.mode === 'window_tab') return action.startsWith('window_tab_') || ['window_number_end', 'window_undo', 'window_redo', 'window_save_layout'].includes(action)
  if (action.startsWith('window_tab_') || action === 'window_number_end') return false
  if (action === 'window_save_layout' || action === 'window_remove_region') return w.mode === 'window_editor' && w.panel === 'tree'
  if (/^window_(layout_|split_|ratio_)/.test(action)) return w.mode === 'window_editor' || w.mode === 'window_quick' && action.startsWith('window_layout_')
  if (w.mode === 'window') return !['window_save_layout', 'window_remove_region'].includes(action)
  return ['window_select', 'window_select_previous', 'window_confirm', 'window_undo', 'window_redo', 'window_reset_initial'].includes(action)
}
/** Dispatch configured actions, keeping Normal movement separate from window motion. */
export function applyWindowAction(state: SimulatorState, action: string, settings: Record<string, any> = {}, now = Date.now(), seconds?: number): boolean {
  if (state.mode === 'window_tab' && !state.window.temporary) {
    action = ({ move_left: 'window_tab_move_left', move_right: 'window_tab_move_right', move_up: 'window_tab_previous', move_down: 'window_tab_next' } as Record<string, string>)[action] ?? action
  }
  if (isWindowMode(action)) {
    if (settings.enabled === false) return true
    if (state.mode === action) { leaveWindow(state); state.mode = 'idle' }
    else switchWindowMode(state, action, settings)
    return true
  }
  if ((!action.startsWith('window_') && action !== 'size_cycle')) return false
  if (!isWindowMode(state.mode)) return true
  const w = state.window, target = windowTarget(w)
  if (w.temporary || !windowActionAvailable(w, action)) return true
  if (w.library) {
    if (action === 'window_delete') { w.deletingPresets = !w.deletingPresets; w.deleteSelection = null; cancelWindowNumber(w); state.lastEvent = w.deletingPresets ? '输入编号选择要删除的预设' : '输入预设编号恢复' }
    if (action === 'window_confirm') { if (w.deleteSelection) deleteWindowPreset(state); else finishWindowNumber(state, settings) }
    return true
  }
  if (w.mode === 'window_tab') {
    if (['window_tab_end', 'window_tab_group', 'window_number_end'].includes(action)) {
      finishWindowNumber(state, settings)
      if (action === 'window_number_end') return true
      cancelWindowNumber(w)
      if (action === 'window_tab_group') {
        if (w.tabs.restore) state.lastEvent = '请选择单个窗口编号'
        else { w.numberSlot = true; w.numberDisplay = '~'; state.lastEvent = '选择标签组编号' }
        return true
      }
    } else cancelWindowNumber(w)
    const applied = tabAction(state, action); refreshWindowNumbers(w); return applied
  }
  if (action === 'window_save_layout') { w.noteOpen = true; return true }
  const layoutAction = /^window_(layout|split|ratio)_(left|down|up|right)$/.exec(action)
  if (layoutAction) { w.swapSource = null; cancelWindowNumber(w)
    const before = editSnapshot(w), direction = layoutAction[2] as LayoutDirection
    if (w.panel === 'quick' && target) {
      quickStep(w.quick, direction, w.ratios)
      const rect = layoutRect(quickRect(w.quick, w.ratios), WINDOW_AREA, settingsNumber(settings, 'gap', 0))
      const cx = rect.x + rect.width / 2, cy = rect.y + rect.height / 2
      rect.width = Math.max(rect.width, target.minWidth ?? 100); rect.height = Math.max(rect.height, target.minHeight ?? 80)
      rect.x = Math.max(0, Math.min(WINDOW_AREA.width - rect.width, cx - rect.width / 2))
      rect.y = Math.max(0, Math.min(WINDOW_AREA.height - rect.height, cy - rect.height / 2))
      Object.assign(target, rect, { restored: undefined })
      if (JSON.stringify(before.quick) !== JSON.stringify(w.quick)) pushEdit(w, before)
      state.lastEvent = quickCaption(w.quick, w.ratios)
    } else if (w.panel === 'tree' && w.tree) {
      if (layoutAction[1] === 'layout') { navigateSlot(w.tree, direction); return true }
      if (layoutAction[1] === 'split') splitSlot(w.tree, direction); else resizeRegionBy(w.tree, direction, seconds === undefined ? settingsNumber(settings, 'resize_step', 20) : seconds * settingsNumber(settings, 'resize_speed', 500), WINDOW_AREA, layoutWindows(w), settingsNumber(settings, 'gap', 0))
      if (!fitTree(w.tree, layoutWindows(w), WINDOW_AREA, settingsNumber(settings, 'gap', 0))) { w.tree = before.tree; state.lastEvent = '窗口最小尺寸不允许此次切分'; return true }
      if (JSON.stringify(before.tree) !== JSON.stringify(w.tree)) { applyTree(w, settings); if (layoutAction[1] !== 'ratio' || !w.gesture) pushEdit(w, before); w.gesture = layoutAction[1] === 'ratio'; refreshWindowNumbers(w) }
    }
    return true
  }
  if (WINDOW_MOTION.has(action)) {

    if (w.panel !== 'none') return true
    if (!target) return true
    if (!w.gesture) { w.group++; w.gesture = true }
    const before = snapshot(w), old = { ...target }
    if (target.restored) { Object.assign(target, target.restored); target.restored = undefined; target.minimized = false }
    const amount = seconds === undefined ? settingsNumber(settings, w.size ? 'resize_step' : 'move_step', 20)
      : seconds * settingsNumber(settings, w.size ? 'resize_speed' : 'move_speed', w.size ? 500 : 600)
    const dx = action === 'window_left' ? -amount : action === 'window_right' ? amount : 0
    const dy = action === 'window_up' ? -amount : action === 'window_down' ? amount : 0
    if (w.size) {
      const cx = target.x + target.width / 2, cy = target.y + target.height / 2
      const maxWidth = 2 * Math.min(cx, WINDOW_AREA.width - cx), maxHeight = 2 * Math.min(cy, WINDOW_AREA.height - cy)
      if (dx && maxWidth >= 100) target.width = Math.min(maxWidth, Math.max(100, target.width + dx))
      if (dy && maxHeight >= 80) target.height = Math.min(maxHeight, Math.max(80, target.height - dy))
      target.x = cx - target.width / 2; target.y = cy - target.height / 2
    } else {
      target.x = Math.max(0, Math.min(WINDOW_AREA.width - target.width, target.x + dx))
      target.y = Math.max(0, Math.min(WINDOW_AREA.height - target.height, target.y + dy))
    }
    changed(w, before); followPointer(state, old, target)
    state.lastEvent = `${w.size ? '缩放' : '移动'} ${target.title}`
    return true
  }
  w.gesture = false; w.group++
  if (action === 'window_confirm' && (w.numberPrefix || w.numberSlot)) { finishWindowNumber(state, settings); return true }
  cancelWindowNumber(w); w.swapSource = null
  switch (action) {
    case 'window_remove_region': {
      if (!w.tree) return true
      const before = editSnapshot(w)
      if (!removeSlot(w.tree)) { state.lastEvent = '至少保留一个区域'; return true }
      if (!fitTree(w.tree, layoutWindows(w), WINDOW_AREA, settingsNumber(settings, 'gap', 0))) { w.tree = before.tree; return true }
      applyTree(w, settings); pushEdit(w, before); refreshWindowNumbers(w); state.lastEvent = '已删除区域'; return true
    }
    case 'window_size': w.size = !w.size; w.panel = 'none'; break
    case 'window_tile': {
      if (!target) break
      if (w.panel !== 'none') endWindowEdit(state, true)
      const before = snapshot(w)
      let count = 0
      for (const screen of new Set(w.windows.map(v => v.screen))) {
        if (w.screens !== 'all' && screen !== target.screen) continue
        const candidates = layoutWindows(w).filter(v => eligibleWindow(w, v) && v.screen === screen && v.resizable !== false && !v.fullscreen).sort((a, b) => Number(b.id === target.id) - Number(a.id === target.id))
        const areas = tileWindows(candidates.length, settingsNumber(settings, 'gap', 0))
        candidates.forEach((v, i) => {
          const area = areas[i]
          area.width = Math.max(area.width, v.minWidth ?? 100); area.height = Math.max(area.height, v.minHeight ?? 80)
          area.x = Math.max(0, Math.min(WINDOW_AREA.width - area.width, area.x)); area.y = Math.max(0, Math.min(WINDOW_AREA.height - area.height, area.y))
          Object.assign(activeTabWindow(w, v.id)!, area, { restored: undefined, minimized: false })
        })
        count += candidates.length; delete w.trees[screen]
      }
      changed(w, before); w.panel = 'none'; state.lastEvent = `屏幕 ${target.screen + 1}：已平铺 ${count} 个窗口`; return true
    }
    case 'window_select': case 'window_select_previous': {
      if (!w.windows.length) break
      const group = w.panel === 'tree' ? undefined : containingTab(w, w.target ?? -1)
      const candidates = (group ? group.members.map(id => snapshot(w).find(v => v.id === id)!).filter(Boolean) : snapshot(w)).filter(v => eligibleWindow(w, v))
      if (!candidates.length) break
      const index = candidates.findIndex(v => v.id === (group?.active ?? w.target))
      const next = candidates[(index + (action === 'window_select_previous' ? candidates.length - 1 : 1)) % candidates.length]
      const quick = w.mode === 'window_quick'
      if (quick) endWindowEdit(state, true)
      selectWindow(state, next.id)
      if (quick) startWindowEdit(state, false, settings)
      if (w.tree) w.tree.selected = treeSlots(w.tree).find(s => s.window === (containingTab(w, next.id)?.members[0] ?? next.id))?.id ?? w.tree.selected
      return true
    }
    case 'window_close': {
      const id = w.target === null ? null : activeTabWindow(w, w.target)?.id ?? w.target
      w.windows = w.windows.filter(window => window.id !== id)
      refreshWindowNumbers(w)
      w.target = null
      state.lastEvent = '已关闭目标窗口'; return true
    }
    case 'window_confirm': return true
    case 'window_reset_initial': {
      const panel = w.panel
      if (panel !== 'none') endWindowEdit(state, true)
      const before = snapshot(w)
      restoreWindows(w, w.initial.filter(window => w.changedWindows.includes(window.id)))
      w.group++; changed(w, before); w.trees = {}; refreshWindowNumbers(w)
      w.screen = windowTarget(w)?.screen ?? w.screen
      if (panel !== 'none') startWindowEdit(state, panel === 'tree', settings, false)
      state.lastEvent = '已恢复本次窗口会话的原始状态'; return true
    }
    case 'window_undo': case 'window_redo': {
      const redo = action === 'window_redo', source = redo ? w.editRedo : w.editHistory
      if (w.panel !== 'none' && source.length) {
        const previous = source.pop()!, destination = redo ? w.editHistory : w.editRedo
        if (destination.length === 32) destination.shift()
        destination.push(editSnapshot(w))
        w.quick = previous.quick; w.tree = previous.tree; restoreWindows(w, previous.windows); refreshWindowNumbers(w)
        state.lastEvent = redo ? '已重做一步编辑' : '已撤销一步编辑'; return true
      }
      if (!redo && w.panel !== 'none' && w.editRedo.length) { state.lastEvent = '本轮没有可撤销的编辑'; return true }
      if (redo && w.panel !== 'none' && w.editHistory.length) { state.lastEvent = '没有可重做的编辑'; return true }
      const panel = w.panel
      if (panel !== 'none') endWindowEdit(state, true)
      const stack = redo ? w.redo : w.history, previous = stack.pop()
      if (previous) {
        const inverse = snapshot(w).filter(window => previous.windows.some(old => old.id === window.id))
        restoreWindows(w, previous.windows)
        const destination = redo ? w.history : w.redo
        if (destination.length === 32) destination.shift()
        destination.push({ group: previous.group, windows: inverse })
      }
      w.trees = {}; refreshWindowNumbers(w); w.screen = windowTarget(w)?.screen ?? w.screen
      if (panel !== 'none') startWindowEdit(state, panel === 'tree', settings, false)
      state.lastEvent = previous ? redo ? '已重做一步窗口调整' : '已撤销一步窗口调整' : redo ? '没有可重做的调整' : '没有可撤销的调整'; return true
    }
    case 'window_center': case 'size_cycle': case 'window_screen_next': case 'window_screen_previous': {
      if (!target) break
      const before = snapshot(w), old = { ...target }
      if (action === 'size_cycle') {
        if (target.minimized) { target.minimized = false; Object.assign(target, target.restored); target.restored = undefined }
        else if (target.restored) {
          target.minimized = true
          for (const id of containingTab(w, target.id)?.members ?? []) {
            const member = w.windows.find(window => window.id === id)
            if (member) member.minimized = true
          }
        }
        else { target.restored = { x: target.x, y: target.y, width: target.width, height: target.height }; Object.assign(target, { x: 0, y: 0, ...WINDOW_AREA }) }
      } else {
        if (target.restored) { Object.assign(target, target.restored); target.restored = undefined; target.minimized = false }
        if (action === 'window_center') { target.x = (WINDOW_AREA.width - target.width) / 2; target.y = (WINDOW_AREA.height - target.height) / 2 }
        else { target.screen = (target.screen + 1) % 2; w.screen = target.screen; refreshWindowNumbers(w) }
      }
      changed(w, before); if (!target.minimized) followPointer(state, old, target); refreshWindowNumbers(w); break
    }
    default: return false
  }
  state.lastEvent = action
  return true
}
