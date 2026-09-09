import type { SimulatorMode, SimulatorState } from './state'
import { automaticTree, fitTree, importTree, layoutRect, moveToSlot, navigateSlot, quickCaption, quickRect, quickStep, resizeSplitBy, removeSlot, retainTreeWindows, splitSlot, treeSlots } from './window-layout.ts'
import type { LayoutDirection, LayoutTree, QuickPlacement } from './window-layout.ts'
import { instantiateLayout, regionTemplate, savedLayoutName } from './window-presets.ts'
import type { SavedWindowLayout } from './window-presets.ts'

export interface WindowRect { x: number; y: number; width: number; height: number }
export interface DemoWindow extends WindowRect {
  id: number
  title: string
  app: string
  screen: number
  restored?: WindowRect
  minWidth?: number
  minHeight?: number
  resizable?: boolean
  fullscreen?: boolean
}
export interface WindowState {
  presets: SavedWindowLayout[]
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
  quick: QuickPlacement
  tree: LayoutTree | null
  trees: Record<number, LayoutTree>
  editBefore: DemoWindow[] | null
  editHistory: Array<{ quick: QuickPlacement; tree: LayoutTree | null; windows: DemoWindow[] }>
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
    presets: [], library: false, libraryPage: 0, libraryIndex: new Map(), noteOpen: false, recent: [], editingPresetId: null, savedEditSnapshot: '',
    windows: [
      { id: 1, title: '项目笔记', app: 'Notes', screen: 0, x: 120, y: 95, width: 430, height: 330 },
      { id: 2, title: 'KeySteer 文档', app: 'Browser', screen: 0, x: 420, y: 180, width: 450, height: 330 },
      { id: 3, title: '文件', app: 'Files', screen: 0, x: 55, y: 340, width: 350, height: 250 },
      { id: 4, title: '终端', app: 'Terminal', screen: 1, x: 180, y: 130, width: 590, height: 380 },
    ], target: null, screen: 0, size: false, temporary: false, panel: 'none',
    quick: { horizontal: null, vertical: null }, tree: null, trees: {}, editBefore: null, editHistory: [],
    numbers: {}, nextNumber: 1, numberDisplay: '', numberPrefix: '', numberSlot: false, numberDeadline: null, windowIndex: new Map(), slotIndex: new Map(), swapSource: null,
    previous: 'normal', gesture: false, group: 0, history: [],
  }
}

export function windowTarget(state: WindowState): DemoWindow | undefined {
  return state.windows.find(w => w.id === state.target)
}

export function setDemoWindowCount(state: SimulatorState, count: number): void {
  count = Math.max(1, Math.min(30, Math.floor(count)))
  const w = state.window, screen = w.screen
  w.windows = Array.from({ length: count }, (_, i) => ({ id: i + 1, title: `示例窗口 ${i + 1}`, app: i % 3 === 0 ? 'Notes' : i % 3 === 1 ? 'Browser' : 'Files', screen,
    x: 35 + i % 6 * 112, y: 70 + Math.floor(i / 6) * 86, width: 300, height: 170 }))
  w.windows.push({ id: count + 1, title: '另一屏幕', app: 'Terminal', screen: (screen + 1) % 2, x: 180, y: 130, width: 590, height: 380 })
  state.mode = w.previous; enterWindow(state)
  if (w.target === null) selectWindow(state, 1)
  state.lastEvent = `${count} 个示例窗口 · 仅歧义编号等待第二位`
}

export function enterWindow(state: SimulatorState): void {
  const w = state.window
  if (state.mode === 'window') return
  w.previous = state.mode
  w.size = false; w.temporary = false; w.history = []; w.gesture = false
  w.library = false; w.noteOpen = false
  w.editingPresetId = null; w.savedEditSnapshot = ''
  const x = state.pointer.x * WINDOW_AREA.width / 100, y = state.pointer.y * WINDOW_AREA.height / 100
  w.target = [...w.windows].reverse().find(v => v.screen === w.screen && x >= v.x && x <= v.x + v.width && y >= v.y && y <= v.y + v.height)?.id ?? null
  w.panel = 'none'; w.tree = null; w.trees = {}; w.editBefore = null; w.editHistory = []
  w.numbers = {}; w.nextNumber = 1; cancelWindowNumber(w); w.swapSource = null
  // The locked window gets the first stable number as the native acquire
  // result arrives before the asynchronous inventory.
  if (w.target !== null) w.numbers[w.target] = w.nextNumber++
  refreshWindowNumbers(w)
  state.mode = 'window'; state.lastEvent = w.target == null ? 'Tab 切换到示例窗口' : '已锁定示例窗口'
}

function exitWindow(state: SimulatorState, settings: Record<string, any> = {}): void {
  state.window.library = false; state.window.noteOpen = false
  if (state.window.panel !== 'none') endWindowEdit(state, true)
  state.mode = !settings.exit_mode || settings.exit_mode === 'return' ? state.window.previous : settings.exit_mode
  state.window.history = []; state.window.panel = 'none'
  state.window.gesture = false; state.window.temporary = false
  cancelWindowNumber(state.window); state.window.trees = {}
  state.lastEvent = '保留窗口调整，返回上一模式'
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
  return state.windows.map(w => ({ ...w, restored: w.restored && { ...w.restored } }))
}

function changed(state: WindowState, before: DemoWindow[]): void {
  if (JSON.stringify(before) === JSON.stringify(state.windows)) return
  if (state.history.at(-1)?.group === state.group) return
  if (state.history.length === 32) state.history.shift()
  state.history.push({ group: state.group, windows: before })
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
export function refreshWindowNumbers(w: WindowState): void {
  const windows = w.windows.filter(v => v.screen === w.screen)
  windows.forEach(v => { w.numbers[v.id] ??= w.nextNumber++ })
  w.windowIndex = numberIndex(windows.map(v => w.numbers[v.id]))
  if (w.tree) retainTreeWindows(w.tree, w.windows.map(v => v.id))
  w.slotIndex = numberIndex(w.tree ? treeSlots(w.tree).map(s => s.id) : [])
}
export function cancelWindowNumber(w: WindowState): boolean {
  const pending = Boolean(w.numberPrefix || w.numberSlot)
  w.numberDisplay = ''; w.numberPrefix = ''; w.numberSlot = false; w.numberDeadline = null
  return pending
}
export function finishWindowNumber(state: SimulatorState, settings: Record<string, any> = {}): void {
  const w = state.window, slot = w.numberSlot
  if (w.library) { const id = w.libraryIndex.get(w.numberPrefix)?.exact; cancelWindowNumber(w); if (id !== undefined) restoreWindowLayout(state, id, settings); return }
  const value = (slot ? w.slotIndex : w.windowIndex).get(w.numberPrefix)?.exact
  const display = w.numberDisplay
  cancelWindowNumber(w)
  w.numberDisplay = display
  if (value !== undefined) chooseWindowNumber(state, value, settings, slot)
}
export function windowSelectionKey(state: SimulatorState, key: string, settings: Record<string, any>, now = Date.now()): boolean {
  const w = state.window
  if (state.mode !== 'window' || w.temporary) return false
  if (w.library) {
    if (key === 'page_down') { w.libraryPage = Math.min(w.libraryPage + 1, Math.floor(Math.max(0, w.presets.length - 1) / 6)); return true }
    if (key === 'page_up') { w.libraryPage = Math.max(0, w.libraryPage - 1); return true }
    if (!/^\d$/.test(key)) return false
    if (w.numberPrefix && !w.libraryIndex.has(w.numberPrefix + key)) finishWindowNumber(state, settings)
    if (!w.library) return true
    w.numberPrefix += key; w.numberDisplay = w.numberPrefix
    const prefix = w.libraryIndex.get(w.numberPrefix)
    if (!prefix) cancelWindowNumber(w)
    else if (prefix.longer) w.numberDeadline = now + settingsNumber(settings, 'number_timeout_ms', 250)
    else finishWindowNumber(state, settings)
    return true
  }
  if (key === '`') {
    if (w.panel === 'quick') endWindowEdit(state, true)
    if (w.panel === 'none') startWindowEdit(state, true, settings)
    cancelWindowNumber(w); w.numberSlot = true; w.numberDisplay = '`'; return true
  }
  if (!/^\d$/.test(key)) return false

  let index = w.numberSlot ? w.slotIndex : w.windowIndex
  if (w.numberPrefix && !index.has(w.numberPrefix + key)) { finishWindowNumber(state, settings); index = w.windowIndex }
  w.numberPrefix += key
  w.numberDisplay = (w.numberSlot ? "`" : "") + w.numberPrefix
  const prefix = index.get(w.numberPrefix)
  if (!prefix) cancelWindowNumber(w)
  else if (prefix.longer) w.numberDeadline = now + settingsNumber(settings, 'number_timeout_ms', 250)
  else finishWindowNumber(state, settings)
  return true
}
function restoreWindows(w: WindowState, before: DemoWindow[]): void {
  // Closed windows stay closed; new windows are not removed by rollback.
  w.windows = w.windows.map(window => {
    const old = before.find(v => v.id === window.id)
    return old ? copy(old) : window
  })
}
function startWindowEdit(state: SimulatorState, tree: boolean, settings: Record<string, any>): void {
  const w = state.window, target = windowTarget(w)
  if (!target) { state.lastEvent = '请先选择窗口'; return }
  if (!tree && (target.resizable === false || target.fullscreen)) { state.lastEvent = '此窗口不可调整尺寸'; return }
  w.screen = target.screen; w.editBefore = snapshot(w); w.editHistory = []; w.swapSource = null
  w.editingPresetId = null; w.savedEditSnapshot = ''
  const windows = w.windows.filter(v => v.screen === w.screen && v.resizable !== false && !v.fullscreen).sort((a, b) => w.numbers[a.id] - w.numbers[b.id])
  if (tree) {
    let cached = w.trees[w.screen] ? copy(w.trees[w.screen]) : null
    if (cached) {
      retainTreeWindows(cached, windows.map(v => v.id))
      const slots = treeSlots(cached)
      const same = windows.every(v => slots.some(s => s.window === v.id)) && slots.every(slot => {
        if (slot.window === null) return true
        const current = windows.find(v => v.id === slot.window), expected = layoutRect(slot.rect, WINDOW_AREA, settingsNumber(settings, 'gap', 8))
        return current && (['x', 'y', 'width', 'height'] as const).every(k => Math.abs(current[k] - expected[k]) < 2)
      })
      if (!same) cached = null
    }
    w.tree = cached ?? automaticTree(windows, target.id, WINDOW_AREA, settingsNumber(settings, 'gap', 8)) ?? importTree(windows, target.id, WINDOW_AREA)
    w.panel = 'tree'
    if (fitTree(w.tree, windows, WINDOW_AREA, settingsNumber(settings, 'gap', 8))) {
      pushEdit(w, editSnapshot(w)); applyTree(w, settings); state.lastEvent = '已自动布局 · Shift 切分 · Ctrl 移动分割线 · X 删除区域'
    } else state.lastEvent = '窗口最小尺寸无法放入此布局，可删除分区后调整'
  } else { w.quick = { horizontal: null, vertical: null }; w.tree = null; w.panel = 'quick'; state.lastEvent = '按方向选择半屏，再调整比例' }
  refreshWindowNumbers(w)
}
function endWindowEdit(state: SimulatorState, commit: boolean): void {
  const w = state.window
  if (w.editBefore) {
    if (commit) { w.group++; changed(w, w.editBefore); if (w.tree) w.trees[w.screen] = copy(w.tree) }
    else restoreWindows(w, w.editBefore)
  }
  w.panel = 'none'; w.tree = null; w.editBefore = null; w.editHistory = []; w.swapSource = null
  cancelWindowNumber(w); refreshWindowNumbers(w)
  state.lastEvent = commit ? '已保留本轮布局' : '已恢复进入编辑前的布局'
}
function applyTree(w: WindowState, settings: Record<string, any>): void {
  if (!w.tree) return
  for (const slot of treeSlots(w.tree)) {
    const window = w.windows.find(v => v.id === slot.window)
    if (window) Object.assign(window, layoutRect(slot.rect, WINDOW_AREA, settingsNumber(settings, 'gap', 8)), { restored: undefined })
  }
}
function editSnapshot(w: WindowState): WindowState['editHistory'][number] {
  return { quick: copy(w.quick), tree: copy(w.tree), windows: snapshot(w) }
}
function pushEdit(w: WindowState, before: WindowState['editHistory'][number]): void {
  if (w.editHistory.length === 32) w.editHistory.shift()
  w.editHistory.push(before)
}
function selectWindow(state: SimulatorState, id: number): void {
  const w = state.window, next = w.windows.find(v => v.id === id)
  if (!next) return
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

  const target = w.windows.find(v => v.screen === w.screen && w.numbers[v.id] === number)
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
      const destination = treeSlots(tree).find(s => s.window === target.id)
      if (w.swapSource !== null) {
        if (destination && w.swapSource !== target.id) changed = moveToSlot(tree, w.swapSource, destination.id)
        w.swapSource = null
      } else {
        selectWindow(state, target.id)
        if (destination) { tree.selected = destination.id; w.swapSource = target.id }
      }
    }
    if (changed) {
      if (!fitTree(tree, w.windows, WINDOW_AREA, settingsNumber(settings, 'gap', 8))) { w.tree = before.tree; state.lastEvent = '窗口最小尺寸无法放入目标区域'; return }
      applyTree(w, settings); pushEdit(w, before); state.lastEvent = '已调整窗口占用'
    }
  } else if (!slot && target) {
    const quick = w.panel === 'quick'
    if (quick) endWindowEdit(state, true)
    selectWindow(state, target.id)
    if (quick) startWindowEdit(state, false, settings)
  }
}
export function saveWindowLayout(state: SimulatorState, note: string): void {
  const w = state.window
  if (!w.tree || w.panel !== 'tree') return
  note = note.trim()
  if ([...note].length > 80 || /[\x00-\x1f\x7f-\x9f]/.test(note)) throw new Error('备注最多 80 个字符，不能包含换行')
  const id = w.editingPresetId ?? Array.from({ length: 99 }, (_, i) => i + 1).find(id => !w.presets.some(p => p.id === id))
  if (!id) throw new Error('布局库已满')
  const preset = { id, note, window_count: treeSlots(w.tree).filter(s => s.window !== null).length, regions: regionTemplate(w.tree.root) }
  w.presets = [...w.presets.filter(p => p.id !== id), preset].sort((a, b) => a.id - b.id)
  w.editingPresetId = id; w.savedEditSnapshot = currentLayoutSnapshot(w)
  w.noteOpen = false; state.lastEvent = `已保存 ${savedLayoutName(preset)}`
}
export function restoreWindowLayout(state: SimulatorState, id: number, settings: Record<string, any> = {}): void {
  const w = state.window, preset = w.presets.find(p => p.id === id)
  if (!preset) return
  if (w.panel !== 'none') endWindowEdit(state, true)
  w.library = false; cancelWindowNumber(w); w.editBefore = snapshot(w); w.editHistory = []; w.swapSource = null
  const recent = [...new Set([...(w.target === null ? [] : [w.target]), ...w.recent, ...w.windows.map(v => v.id).reverse()])]
  const windows = recent.map(id => w.windows.find(v => v.id === id)).filter((v): v is DemoWindow => !!v && v.screen === w.screen)
  w.tree = instantiateLayout(preset, windows); w.panel = 'tree'
  if (fitTree(w.tree, windows, WINDOW_AREA, settingsNumber(settings, 'gap', 8))) {
    pushEdit(w, editSnapshot(w)); applyTree(w, settings); state.lastEvent = `已恢复 ${savedLayoutName(preset)}`
  } else state.lastEvent = '窗口最小尺寸无法放入此布局，可删除区域后调整'
  refreshWindowNumbers(w)
  w.editingPresetId = id; w.savedEditSnapshot = currentLayoutSnapshot(w)
}
function currentLayoutSnapshot(w: WindowState): string { return w.tree ? JSON.stringify([regionTemplate(w.tree.root), treeSlots(w.tree).filter(s => s.window !== null).length]) : '' }
export function hasWindowLayoutChanges(w: WindowState): boolean { return w.panel === 'tree' && !!w.tree && currentLayoutSnapshot(w) !== w.savedEditSnapshot }
export function replaceWindowLayouts(state: SimulatorState, layouts: SavedWindowLayout[]): void {
  if (state.window.panel !== 'none') endWindowEdit(state, true)
  state.window.noteOpen = false
  state.window.presets = layouts; state.window.libraryIndex = numberIndex(layouts.map(p => p.id)); state.window.libraryPage = 0
  state.window.editingPresetId = null; state.window.savedEditSnapshot = ''; cancelWindowNumber(state.window)
}
export function windowDetail(w: WindowState): string {
  if (w.library) return 'Restore'
  const phase = w.panel === 'quick' ? `Quick · ${quickCaption(w.quick)}` : w.panel === 'tree' ? `Edit · 区域 \`${w.tree?.selected}` : w.size ? 'Resize' : 'Move'
  return phase
}
export function windowInputStatus(w: WindowState): string {
  if (w.numberDisplay) return `输入：${w.numberDisplay}${w.numberPrefix || w.numberSlot ? '…' : ''}`
  if (w.swapSource !== null) return `窗口 ${w.numbers[w.swapSource]} → 窗口编号 / 反引号＋区域编号`
  return ''
}
export function windowActionAvailable(w: WindowState, action: string): boolean {
  if (w.library) return ['window_saved_layouts', 'window_cancel', 'window_confirm', 'window_exit'].includes(action)
  if (action === 'window_saved_layouts') return true
  if (action === 'window_save_layout') return w.panel === 'tree'
  if (action === 'window_remove_region') return w.panel === 'tree'
  if (w.panel === 'none') return !/^window_(layout_|split_|ratio_)/.test(action)
  if (/^window_(layout_|split_|ratio_)/.test(action)) return w.panel === 'tree' || action.startsWith('window_layout_')
  return ['window_select', 'window_confirm', 'window_cancel', 'window_exit', 'window_tile', 'window_undo', ...(w.panel === 'quick' ? ['window_layout', 'window_edit'] : [])].includes(action)
}
/** Dispatch configured actions, keeping Normal movement separate from window motion. */
export function applyWindowAction(state: SimulatorState, action: string, settings: Record<string, any> = {}, now = Date.now(), seconds?: number): boolean {
  if (action === 'window') {
    if (settings.enabled === false) return true
    if (state.mode === 'window') exitWindow(state, settings); else enterWindow(state)
    return true
  }
  if (!action.startsWith('window_')) return false
  if (state.mode !== 'window') return true
  const w = state.window, target = windowTarget(w)
  if (w.temporary && action !== 'window_exit') return true
  if (!windowActionAvailable(w, action)) return true
  if (w.library) {
    if (action === 'window_saved_layouts') { w.libraryIndex = numberIndex(w.presets.map(p => p.id)); cancelWindowNumber(w) }
    else if (action === 'window_cancel') { w.library = false; cancelWindowNumber(w) }
    else if (action === 'window_confirm') finishWindowNumber(state, settings)
    else if (action === 'window_exit') exitWindow(state, settings)
    return true
  }
  if (action === 'window_saved_layouts') { w.library = true; w.libraryPage = 0; cancelWindowNumber(w); w.libraryIndex = numberIndex(w.presets.map(p => p.id)); return true }
  if (action === 'window_save_layout') { w.noteOpen = true; return true }
  const layoutAction = /^window_(layout|split|ratio)_(left|down|up|right)$/.exec(action)
  if (layoutAction) { w.swapSource = null; cancelWindowNumber(w)
    const before = editSnapshot(w), direction = layoutAction[2] as LayoutDirection
    if (w.panel === 'quick' && target) {
      quickStep(w.quick, direction)
      const rect = layoutRect(quickRect(w.quick), WINDOW_AREA, settingsNumber(settings, 'gap', 8))
      const cx = rect.x + rect.width / 2, cy = rect.y + rect.height / 2
      rect.width = Math.max(rect.width, target.minWidth ?? 100); rect.height = Math.max(rect.height, target.minHeight ?? 80)
      rect.x = Math.max(0, Math.min(WINDOW_AREA.width - rect.width, cx - rect.width / 2))
      rect.y = Math.max(0, Math.min(WINDOW_AREA.height - rect.height, cy - rect.height / 2))
      Object.assign(target, rect, { restored: undefined })
      if (JSON.stringify(before.quick) !== JSON.stringify(w.quick)) pushEdit(w, before)
      state.lastEvent = quickCaption(w.quick)
    } else if (w.panel === 'tree' && w.tree) {
      if (layoutAction[1] === 'layout') { navigateSlot(w.tree, direction); return true }
      if (layoutAction[1] === 'split') splitSlot(w.tree, direction); else resizeSplitBy(w.tree, direction, seconds === undefined ? settingsNumber(settings, 'resize_step', 20) : seconds * settingsNumber(settings, 'resize_speed', 500), WINDOW_AREA)
      if (!fitTree(w.tree, w.windows, WINDOW_AREA, settingsNumber(settings, 'gap', 8))) { w.tree = before.tree; state.lastEvent = '窗口最小尺寸不允许此次切分'; return true }
      if (JSON.stringify(before.tree) !== JSON.stringify(w.tree)) { applyTree(w, settings); if (layoutAction[1] !== 'ratio' || !w.gesture) pushEdit(w, before); w.gesture = layoutAction[1] === 'ratio'; refreshWindowNumbers(w) }
    }
    return true
  }
  if (WINDOW_MOTION.has(action)) {

    if (w.panel !== 'none') return true
    if (!target) return true
    if (!w.gesture) { w.group++; w.gesture = true }
    const before = snapshot(w), old = { ...target }
    if (target.restored) { Object.assign(target, target.restored); target.restored = undefined }
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
  if (action === 'window_cancel' && (w.numberPrefix || w.numberSlot || w.swapSource !== null)) {
    cancelWindowNumber(w); w.swapSource = null; return true
  }
  cancelWindowNumber(w); w.swapSource = null
  switch (action) {
    case 'window_remove_region': {
      if (!w.tree) return true
      const before = editSnapshot(w)
      if (!removeSlot(w.tree)) { state.lastEvent = '至少保留一个区域'; return true }
      if (!fitTree(w.tree, w.windows, WINDOW_AREA, settingsNumber(settings, 'gap', 8))) { w.tree = before.tree; return true }
      applyTree(w, settings); pushEdit(w, before); refreshWindowNumbers(w); state.lastEvent = '已删除区域'; return true
    }
    case 'window_size': w.size = !w.size; w.panel = 'none'; break
    case 'window_layout':
      if (w.panel === 'none') startWindowEdit(state, false, settings)
      return true
    case 'window_edit':
      if (w.panel === 'quick') endWindowEdit(state, true)
      startWindowEdit(state, true, settings); return true
    case 'window_tile': {
      if (!target) break
      if (w.panel !== 'none') endWindowEdit(state, true)
      const before = snapshot(w)
      const candidates = w.windows.filter(v => v.screen === target.screen && v.resizable !== false && !v.fullscreen).sort((a, b) => Number(b.id === target.id) - Number(a.id === target.id))
      const areas = tileWindows(candidates.length, settingsNumber(settings, 'gap', 8))
      candidates.forEach((v, i) => {
        const area = areas[i]
        area.width = Math.max(area.width, v.minWidth ?? 100); area.height = Math.max(area.height, v.minHeight ?? 80)
        area.x = Math.max(0, Math.min(WINDOW_AREA.width - area.width, area.x)); area.y = Math.max(0, Math.min(WINDOW_AREA.height - area.height, area.y))
        Object.assign(v, area, { restored: undefined })
      })
      delete w.trees[w.screen]
      changed(w, before); w.panel = 'none'; state.lastEvent = `屏幕 ${target.screen + 1}：已平铺 ${candidates.length} 个窗口`; return true
    }
    case 'window_select': {
      if (!w.windows.length) break
      const candidates = w.panel === 'tree' ? w.windows.filter(v => v.screen === w.screen) : w.windows
      if (!candidates.length) break
      const index = candidates.findIndex(v => v.id === w.target), next = candidates[(index + 1) % candidates.length]
      const quick = w.panel === 'quick'
      if (quick) endWindowEdit(state, true)
      selectWindow(state, next.id)
      if (quick) startWindowEdit(state, false, settings)
      if (w.tree) w.tree.selected = treeSlots(w.tree).find(s => s.window === next.id)?.id ?? w.tree.selected
      return true
    }
    case 'window_confirm':
      if (w.panel !== 'none') { endWindowEdit(state, true); return true }
      exitWindow(state, settings); return true
    case 'window_cancel':
      if (w.panel !== 'none') { endWindowEdit(state, true); return true }
      exitWindow(state, settings); return true
    case 'window_exit': exitWindow(state, settings); return true
    case 'window_undo': {
      if (w.panel !== 'none') {
        const previous = w.editHistory.pop()
        if (previous) { w.quick = previous.quick; w.tree = previous.tree; restoreWindows(w, previous.windows); refreshWindowNumbers(w) }
        state.lastEvent = previous ? '已撤销本轮编辑的一步' : '本轮没有可撤销的编辑'; return true
      }
      const previous = w.history.pop()
      if (previous) restoreWindows(w, previous.windows)
      w.trees = {}; refreshWindowNumbers(w)
      w.panel = 'none'; w.screen = windowTarget(w)?.screen ?? w.screen
      state.lastEvent = previous ? '已撤销一步窗口调整' : '没有可撤销的调整'; return true
    }
    case 'window_center': case 'window_maximize': case 'window_screen_next': case 'window_screen_previous': {
      if (!target) break
      const before = snapshot(w), old = { ...target }
      if (action === 'window_maximize') {
        if (target.restored) { Object.assign(target, target.restored); target.restored = undefined }
        else { target.restored = { x: target.x, y: target.y, width: target.width, height: target.height }; Object.assign(target, { x: 0, y: 0, ...WINDOW_AREA }) }
      } else {
        if (target.restored) { Object.assign(target, target.restored); target.restored = undefined }
        if (action === 'window_center') { target.x = (WINDOW_AREA.width - target.width) / 2; target.y = (WINDOW_AREA.height - target.height) / 2 }
        else { target.screen = (target.screen + 1) % 2; w.screen = target.screen; refreshWindowNumbers(w) }
      }
      changed(w, before); followPointer(state, old, target); break
    }
    default: return false
  }
  state.lastEvent = action
  return true
}
