import assert from 'node:assert/strict'
import test from 'node:test'
import { createSimulatorState, applyModeAction } from './state.ts'
import { applyWindowAction, chooseWindowNumber, enterWindow, finishWindowNumber, refreshWindowNumbers, temporaryWindow, tileWindows, windowSelectionKey, windowTarget, WINDOW_AREA } from './window.ts'
import { resolvePhysicalBinding, shortcutCaption, temporaryPhysicalKeys } from './bindings.ts'
import { automaticTree, importTree, fitTree, treeSlots, splitSlot, removeSlot, resizeRegionBy } from './window-layout.ts'
import { parseSplitRatios } from './window-ratios.ts'

test('E enters automatic editing directly; ordinary mode bindings control all return destinations', () => {
  const state = createSimulatorState()
  enterWindow(state)
  applyWindowAction(state, 'window_editor')
  assert.equal(state.window.panel, 'tree')
  applyWindowAction(state, 'window')
  assert.equal(state.window.panel, 'none')
  assert.equal(state.mode, 'window')
  applyWindowAction(state, 'window_quick')
  assert.equal(state.window.panel, 'quick')
  applyWindowAction(state, 'window')
  assert.equal(state.mode, 'window')
  assert.equal(state.window.panel, 'none')
  applyWindowAction(state, 'window')
  assert.equal(state.mode, 'idle')
})

test('automatic arrangement fits overlapping windows using rows as well as columns', () => {
  const windows = Array.from({ length: 9 }, (_, i) => ({ id: i + 1, x: 100, y: 100, width: 1000, height: 700, minWidth: 600, minHeight: 250 }))
  const area = { width: 1920, height: 1080 }
  assert.equal(fitTree(importTree(windows, 5, area), windows, area, 8), false)
  const tree = automaticTree(windows, 5, area, 8)!
  assert.ok(tree)
  assert.equal(tree.selected, 5)
  assert.equal(treeSlots(tree).length, 9)
  assert.equal(fitTree(tree, windows, area, 8), true)
  assert.equal(automaticTree(windows.map(w => ({ ...w, minWidth: 1000, minHeight: 700 })), 5, area, 8), null)
})

test('divider short presses use pixel steps independently of legacy fractions', () => {
  const state = createSimulatorState()
  state.window.windows = [
    { id: 1, title: 'Left', app: 'Demo', screen: 0, x: 0, y: 0, width: 490, height: 650 },
    { id: 2, title: 'Right', app: 'Demo', screen: 0, x: 510, y: 0, width: 490, height: 650 },
  ]
  const settings = { split_ratios: ['4/5', .4, '1/5', .6, '2/5', .8] }
  state.pointer = { x: 25, y: 30 }
  enterWindow(state); applyWindowAction(state, 'window_quick', settings); applyWindowAction(state, 'window_editor', settings)
  for (const expected of [.52, .54, .56]) {
    applyWindowAction(state, 'window_ratio_right', settings)
    const root = state.window.tree!.root
    assert.equal(root.kind, 'split')
    if (root.kind === 'split') assert.ok(Math.abs(root.ratio - expected) < 1e-9)
  }
  const cached = parseSplitRatios(settings.split_ratios)
  assert.equal(parseSplitRatios(settings.split_ratios), cached)
  settings.split_ratios[0] = '1/10'
  assert.equal(parseSplitRatios(settings.split_ratios)[0], .1)
})

test('mixed ratios normalize order and duplicates while preserving source', () => {
  const input = ['3/4', .3, '1/2', .4, '2/4', .5]
  const original = input.slice()
  assert.deepEqual(parseSplitRatios(input), [.3, .4, .5, .75])
  assert.deepEqual(input, original)
  assert.deepEqual(parseSplitRatios([.4, .3, .4]), [.3, .4])
})

test('Window locks the pointer target and an Idle binding releases its session', () => {
  const state = createSimulatorState()
  state.mode = 'grid'; state.targeting.grid.path = ['a', 'b']
  enterWindow(state)
  assert.equal(state.window.target, 2)
  state.pointer = { x: 1, y: 1 }
  applyWindowAction(state, 'window_right')
  assert.equal(state.window.target, 2)
  applyModeAction(state, 'idle')
  assert.equal(state.mode, 'idle')
  assert.deepEqual(state.targeting.grid.path, ['a', 'b'])
  assert.equal(state.window.history.length, 0)
})

test('Tab with all screens enabled cycles across screens and centers the pointer', () => {
  const state = createSimulatorState()
  enterWindow(state, { screens: 'all' })
  applyWindowAction(state, 'window_size')
  for (const id of [3, 4, 1, 2]) {
    applyWindowAction(state, 'window_select')
    const target = windowTarget(state.window)!
    assert.equal(target.id, id)
    assert.equal(state.pointer.x, (target.x + target.width / 2) / WINDOW_AREA.width * 100)
    assert.equal(state.pointer.y, (target.y + target.height / 2) / WINDOW_AREA.height * 100)
    assert.equal(state.window.panel, 'none')
    assert.equal(state.window.size, true)
  }
})

test('A opens quick, E arranges and edits, and undo restores the entry geometry', () => {
  const state = createSimulatorState()
  enterWindow(state)
  const before = JSON.stringify(state.window.windows)
  applyWindowAction(state, 'window_quick', {}, 1000)
  assert.equal(JSON.stringify(state.window.windows), before)
  assert.equal(state.window.panel, 'quick')
  applyWindowAction(state, 'window_editor')
  assert.notEqual(JSON.stringify(state.window.windows), before)
  assert.equal(state.window.panel, 'tree')
  applyWindowAction(state, 'window_undo')
  assert.equal(JSON.stringify(state.window.windows), before)
  assert.equal(state.window.panel, 'tree')
})

test('quick ratios apply immediately and back keeps them until undo', () => {
  const state = createSimulatorState()
  enterWindow(state)
  applyWindowAction(state, 'window_size')
  const before = JSON.stringify(state.window.windows)
  applyWindowAction(state, 'window_quick', {}, 1000)
  assert.equal(state.window.history.length, 0)
  assert.equal(windowSelectionKey(state, 'q', {}), false)
  applyWindowAction(state, 'window_layout_left')
  applyWindowAction(state, 'window_layout_left')
  assert.equal(state.mode, 'window_quick')
  assert.equal(state.window.size, true)
  assert.equal(windowTarget(state.window)!.x, 0)
  assert.equal(windowTarget(state.window)!.width, 1000 / 3)
  const applied = JSON.stringify(state.window.windows)
  applyWindowAction(state, 'window')
  assert.equal(JSON.stringify(state.window.windows), applied)
  applyWindowAction(state, 'window_undo')
  assert.equal(JSON.stringify(state.window.windows), before)
  assert.equal(state.window.panel, 'none')
})

test('center resize is symmetric and a held gesture is one undo step', () => {
  const state = createSimulatorState()
  enterWindow(state)
  const original = { ...windowTarget(state.window)! }
  applyWindowAction(state, 'window_size')
  for (let frame = 0; frame < 12; frame++) applyWindowAction(state, 'window_left', {}, 1000 + frame * 16, .016)
  const resized = windowTarget(state.window)!
  assert.equal(resized.x + resized.width / 2, original.x + original.width / 2)
  assert.equal(state.window.history.length, 1)
  applyWindowAction(state, 'window_undo')
  assert.equal(windowTarget(state.window)!.width, original.width)
})

test('temporary Normal preserves target and substate without a double-tap state', () => {
  const state = createSimulatorState()
  enterWindow(state)
  applyWindowAction(state, 'window_size')
  applyWindowAction(state, 'window_quick', {}, 1000)
  const before = JSON.stringify(state.window.windows)
  temporaryWindow(state.window, true)
  applyWindowAction(state, 'window_right')
  temporaryWindow(state.window, false)
  assert.equal(JSON.stringify(state.window.windows), before)
  assert.equal(state.window.size, true)
  assert.equal(state.window.panel, 'quick')
})

test('all tiling counts have equal areas and contain no empty or overlapping cells', () => {
  for (let count = 1; count <= 40; count++) {
    const cells = tileWindows(count, 0)
    assert.equal(cells.length, count)
    cells.forEach((a, i) => {
      assert.ok(Math.abs(a.width * a.height - WINDOW_AREA.width * WINDOW_AREA.height / count) < .001)
      assert.ok(a.x >= 0 && a.y >= 0 && a.x + a.width <= WINDOW_AREA.width + .001 && a.y + a.height <= WINDOW_AREA.height + .001)
      cells.slice(i + 1).forEach(b => assert.ok(a.x + a.width <= b.x + .001 || b.x + b.width <= a.x + .001 || a.y + a.height <= b.y + .001 || b.y + b.height <= a.y + .001))
    })
  }
})

test('physical Window bindings honor custom aliases and independent Normal controls', () => {
  const document = { key_aliases: { windows: { Primary: 'right_alt' } },
    normal: { bindings: { a: 'move_left' } }, window: { temporary_mode: 'normal', temporary_mode_keys: ['primary'],
      bindings: { a: 'window_quick', 'primary+q': 'window_exit', x: 'window_undo', z: 'none' } } }
  const pressed = ['right_alt', 'a']
  assert.deepEqual(temporaryPhysicalKeys(document, 'window', pressed, false), ['right_alt'])
  assert.equal(resolvePhysicalBinding(document, 'window', pressed, 'a', false), undefined)
  assert.equal(resolvePhysicalBinding(document, 'normal', ['a'], 'a', false)?.value, 'move_left')
  assert.equal(resolvePhysicalBinding(document, 'window', ['right_alt', 'q'], 'q', false)?.value, 'window_exit')
  assert.equal(resolvePhysicalBinding(document, 'window', ['x'], 'x', false)?.value, 'window_undo')
  assert.equal(resolvePhysicalBinding(document, 'window', ['z'], 'z', false)?.value, 'none')
})

test('number parsing waits only for live ambiguous prefixes and never activates an intermediate window', () => {
  for (const count of [1, 9, 10, 20, 23, 30]) {
    for (let digit = 1; digit <= Math.min(9, count); digit++) {
      const state = createSimulatorState()
      state.window.windows = Array.from({ length: count }, (_, i) => ({ id: i + 1, title: `W${i + 1}`, app: 'Demo', screen: 0,
        x: i === 0 ? 0 : 200 + i % 8 * 90, y: i === 0 ? 0 : 150 + Math.floor(i / 8) * 100, width: 100, height: 80 }))
      state.pointer = { x: 5, y: 5 }; enterWindow(state)
      windowSelectionKey(state, String(digit), {}, 1000)
      const ambiguous = Array.from({ length: count }, (_, i) => String(i + 1)).some(n => n.length > 1 && n.startsWith(String(digit)))
      assert.equal(state.window.numberDeadline !== null, ambiguous, `count=${count}, digit=${digit}`)
      if (ambiguous) {
        assert.equal(state.window.target, 1)
        finishWindowNumber(state)
      }
      assert.equal(state.window.target, digit)
    }
  }
  const state = createSimulatorState()
  state.window.windows = Array.from({ length: 23 }, (_, i) => ({ id: i + 1, title: `W${i + 1}`, app: 'Demo', screen: 0, x: 10 + i * 20, y: 10, width: 100, height: 80 }))
  enterWindow(state)
  const original = state.window.target
  windowSelectionKey(state, '1', {}, 1000)
  assert.equal(state.window.target, original)
  windowSelectionKey(state, '2', {}, 1100)
  const window12 = state.window.windows.find(w => state.window.numbers[w.id] === 12)!
  assert.equal(state.window.target, window12.id)
  assert.equal(state.window.numberDeadline, null)
})

test('independent layout bindings honor aliases, inheritance and explicit modifiers', () => {
  const document = { key_aliases: { west: 'a' }, window_quick: { bindings: { west: 'window_layout_left', h: 'none' } }, window_editor: { inherits: ['window_quick'], bindings: { 'shift+west': 'window_split_left', 'ctrl+d': 'window_ratio_right' } }, normal: { bindings: { a: 'move_right' } } }
  assert.equal(resolvePhysicalBinding(document, 'window_quick', ['a'], 'a', false)?.value, 'window_layout_left')
  assert.equal(resolvePhysicalBinding(document, 'window_editor', ['left_shift', 'a'], 'a', false)?.value, 'window_split_left')
  assert.equal(resolvePhysicalBinding(document, 'window_editor', ['right_ctrl', 'd'], 'd', false)?.value, 'window_ratio_right')
  assert.equal(resolvePhysicalBinding(document, 'window_quick', ['h'], 'h', false)?.value, 'none')
})

test('tree splits, moves into empty slots, swaps, and exits without reverting', () => {
  const state = createSimulatorState(); enterWindow(state)
  applyWindowAction(state, 'window_quick')
  const before = JSON.stringify(state.window.windows)
  applyWindowAction(state, 'window_editor')
  assert.equal(state.window.panel, 'tree')
  const count = treeSlots(state.window.tree!).length
  applyWindowAction(state, 'window_split_right')
  assert.equal(treeSlots(state.window.tree!).length, count + 1)
  const empty = treeSlots(state.window.tree!).find(s => s.window === null)!
  const source = state.window.target!
  chooseWindowNumber(state, state.window.numbers[source])
  assert.equal(state.window.swapSource, source)
  if (state.mode !== 'window_editor') applyWindowAction(state, 'window_editor')
  windowSelectionKey(state, '`', {})
  windowSelectionKey(state, String(empty.id), {})
  assert.equal(treeSlots(state.window.tree!).find(s => s.id === empty.id)!.window, source)
  assert.equal(state.window.swapSource, null)
  const another = state.window.windows.find(w => w.screen === 0 && w.id !== source)!
  const oldSourceSlot = treeSlots(state.window.tree!).find(s => s.window === source)!.id
  const oldTargetSlot = treeSlots(state.window.tree!).find(s => s.window === another.id)!.id
  chooseWindowNumber(state, state.window.numbers[source]); chooseWindowNumber(state, state.window.numbers[another.id])
  assert.equal(treeSlots(state.window.tree!).find(s => s.id === oldTargetSlot)!.window, source)
  assert.equal(treeSlots(state.window.tree!).find(s => s.id === oldSourceSlot)!.window, another.id)
  const applied = JSON.stringify(state.window.windows)
  applyWindowAction(state, 'window')
  assert.equal(JSON.stringify(state.window.windows), applied)
  applyWindowAction(state, 'window_undo')
  assert.equal(JSON.stringify(state.window.windows), before)
})

test('tree minimum sizes constrain the divider and infeasible splits leave geometry unchanged', () => {
  const state = createSimulatorState()
  state.window.windows = [
    { id: 1, title: 'Small', app: 'Demo', screen: 0, x: 0, y: 0, width: 220, height: 650 },
    { id: 2, title: 'Constrained', app: 'ChatGPT', screen: 0, x: 240, y: 0, width: 760, height: 650, minWidth: 720, minHeight: 650 },
  ]
  enterWindow(state); applyWindowAction(state, 'window_quick'); applyWindowAction(state, 'window_editor')
  assert.equal(state.window.panel, 'tree')
  applyWindowAction(state, 'window_ratio_right')
  assert.ok(windowTarget(state.window)!.width >= 720 - 1e-6)
  const before = JSON.stringify(state.window.windows), tree = JSON.stringify(state.window.tree)
  applyWindowAction(state, 'window_split_down')
  assert.equal(JSON.stringify(state.window.tree), tree)
  assert.equal(JSON.stringify(state.window.windows), before)
})

test('leaving tree editing is one ordinary undo, and closing windows preserves empty slots and numbering', () => {
  const state = createSimulatorState(); enterWindow(state)
  const before = JSON.stringify(state.window.windows)
  applyWindowAction(state, 'window_quick'); applyWindowAction(state, 'window_editor')
  applyWindowAction(state, 'window_split_right'); applyWindowAction(state, 'window_ratio_left')
  applyWindowAction(state, 'window')
  assert.equal(state.window.history.length, 1)
  applyWindowAction(state, 'window_undo')
  assert.equal(JSON.stringify(state.window.windows), before)
  applyWindowAction(state, 'window_quick'); applyWindowAction(state, 'window_editor')
  const count = treeSlots(state.window.tree!).length, numbers = { ...state.window.numbers }
  const closing = state.window.windows.find(w => w.screen === 0 && w.id !== state.window.target)!
  state.window.windows = state.window.windows.filter(w => w.id !== closing.id); refreshWindowNumbers(state.window)
  assert.equal(treeSlots(state.window.tree!).length, count)
  assert.ok(treeSlots(state.window.tree!).some(s => s.window === null))
  delete numbers[closing.id]
  assert.deepEqual(state.window.numbers, numbers)
  applyWindowAction(state, 'window')
  assert.ok(!state.window.windows.some(w => w.id === closing.id))
})


test('Window directions are independent of Normal bindings', () => {
  const document = { window: { bindings: { h: 'window_left' } }, normal: { bindings: { v: 'move_left' } } }
  assert.equal(resolvePhysicalBinding(document, 'window', ['h'], 'h', false)?.value, 'window_left')
  assert.equal(resolvePhysicalBinding(document, 'window', ['v'], 'v', false), undefined)
  assert.equal(resolvePhysicalBinding(document, 'window', ['left'], 'left', false), undefined)
})

test('backtick in Editor selects a region and region selection activates its window', () => {
  const state = createSimulatorState()
  enterWindow(state)
  const before = structuredClone(state.window.windows)
  if (state.mode !== 'window_editor') applyWindowAction(state, 'window_editor')
  windowSelectionKey(state, '`', {})
  assert.equal(state.window.panel, 'tree')
  assert.notDeepEqual(state.window.windows, before)
  const slot = treeSlots(state.window.tree!).find(s => s.window !== state.window.target)!
  for (const digit of String(slot.id)) windowSelectionKey(state, digit, {})
  assert.equal(state.window.target, slot.window)
  assert.equal(state.window.tree!.selected, slot.id)
  assert.equal(state.window.numberDisplay, '`' + slot.id)
})

test('continuous divider motion is one undo and X removes a region with stable IDs', () => {
  const state = createSimulatorState()
  state.window.windows = [{ id: 1, title: 'One', app: 'Demo', screen: 0, x: 100, y: 100, width: 600, height: 400 }]
  enterWindow(state)
  if (state.mode !== 'window_editor') applyWindowAction(state, 'window_editor')
  windowSelectionKey(state, '`', {})
  applyWindowAction(state, 'window_split_right')
  const before = structuredClone(state.window.tree)
  applyWindowAction(state, 'window_ratio_right')
  for (let i = 0; i < 5; i++) applyWindowAction(state, 'window_ratio_right', {}, 0, .02)
  assert.equal(state.window.editHistory.length, 3)
  state.window.gesture = false
  applyWindowAction(state, 'window_undo')
  assert.deepEqual(state.window.tree, before)
  chooseWindowNumber(state, 2, {}, true)
  applyWindowAction(state, 'window_remove_region')
  assert.equal(treeSlots(state.window.tree!).length, 1)
  assert.equal(state.window.windows.length, 1)
  applyWindowAction(state, 'window_undo')
  assert.equal(treeSlots(state.window.tree!).length, 2)
})


test('all window modes launch directly from Idle and share session numbers', () => {
  for (const mode of ['window', 'window_quick', 'window_editor', 'window_restore']) {
    const state = createSimulatorState(); state.mode = 'idle'
    applyWindowAction(state, mode)
    assert.equal(state.mode, mode)
    const target = state.window.target, numbers = { ...state.window.numbers }
    applyWindowAction(state, mode === 'window_restore' ? 'window_delete' : 'window_restore')
    assert.equal(state.window.target, target)
    assert.deepEqual(state.window.numbers, numbers)
    applyModeAction(state, 'grid')
    assert.equal(state.mode, 'grid')
    assert.equal(state.window.library, false)
  }
})

test('Quick uses independently configured ratios', () => {
  const state = createSimulatorState(); applyWindowAction(state, 'window_quick', { split_ratios: [.2, .4, .6, .8] })
  applyWindowAction(state, 'window_layout_left')
  assert.equal(windowTarget(state.window)!.width, 400)
  applyWindowAction(state, 'window_layout_left')
  assert.equal(windowTarget(state.window)!.width, 200)
})


test('state cycle preserves target, restores original geometry and rejects old names and supports undo', () => {
  const state = createSimulatorState()
  enterWindow(state)
  const target = windowTarget(state.window)!
  const original = { x: target.x, y: target.y, width: target.width, height: target.height }
  for (const old of ['window_maximize', 'window_cycle_state']) assert.equal(applyWindowAction(state, old), false)
  for (let i = 0; i < 2; i++) {
    applyWindowAction(state, 'size_cycle')
    assert.equal(target.width, WINDOW_AREA.width)
    applyWindowAction(state, 'size_cycle')
    assert.equal(target.minimized, true)
    assert.equal(windowTarget(state.window)?.id, target.id)
    applyWindowAction(state, 'size_cycle')
    assert.equal(target.minimized, false)
    assert.deepEqual({ x: target.x, y: target.y, width: target.width, height: target.height }, original)
  }
  applyWindowAction(state, 'window_undo')
  assert.equal(windowTarget(state.window)?.minimized, true)
  applyWindowAction(state, 'window_undo')
  assert.equal(!!windowTarget(state.window)?.minimized, false)
  assert.equal(windowTarget(state.window)?.width, WINDOW_AREA.width)
})


test('temporary chords ignore held entry keys without blocking a fresh alternative chord', () => {
  const document = { window: { temporary_mode: 'normal', temporary_mode_keys: ['alt', 'ctrl'] } }
  const entry = new Set(['left_alt', 'w'])
  assert.deepEqual(temporaryPhysicalKeys(document, 'window', ['left_alt'], false, entry), [])
  assert.deepEqual(temporaryPhysicalKeys(document, 'window', ['left_alt', 'left_ctrl'], false, entry), ['left_ctrl'])
  entry.delete('left_alt')
  assert.deepEqual(temporaryPhysicalKeys(document, 'window', ['left_alt'], false, entry), ['left_alt'])
})


test('undo and redo round-trip moves and a new action invalidates redo', () => {
  const state = createSimulatorState(); enterWindow(state)
  const initial = structuredClone(windowTarget(state.window)!)
  applyWindowAction(state, 'window_right'); applyWindowAction(state, 'window_size')
  const moved = structuredClone(windowTarget(state.window)!)
  applyWindowAction(state, 'window_undo')
  assert.deepEqual(windowTarget(state.window), initial)
  applyWindowAction(state, 'window_undo')
  applyWindowAction(state, 'window_redo')
  assert.deepEqual(windowTarget(state.window), moved)
  applyWindowAction(state, 'window_undo'); applyWindowAction(state, 'window_center')
  assert.equal(state.window.redo.length, 0)
})

test('Quick and Editor preserve redo across the original-state undo boundary', () => {
  for (const mode of ['window_quick', 'window_editor']) {
    const state = createSimulatorState(); enterWindow(state)
    const initial = structuredClone(state.window.windows)
    applyWindowAction(state, mode)
    if (mode === 'window_quick') applyWindowAction(state, 'window_layout_left')
    const arranged = JSON.parse(JSON.stringify(state.window.windows))
    assert.notDeepEqual(arranged, initial)
    applyWindowAction(state, 'window_undo')
    assert.deepEqual(state.window.windows, initial)
    applyWindowAction(state, 'window_undo')
    applyWindowAction(state, 'window_redo')
    assert.deepEqual(state.window.windows, arranged)
    applyWindowAction(state, 'window_undo')
    applyWindowAction(state, mode === 'window_quick' ? 'window_layout_right' : 'window_split_right')
    assert.equal(state.window.editRedo.length, 0)
  }
})

test('initial reset crosses modes and remains undoable without arranging again', () => {
  const state = createSimulatorState(); enterWindow(state)
  const initial = structuredClone(state.window.windows)
  applyWindowAction(state, 'window_right'); applyWindowAction(state, 'window_editor')
  const arranged = JSON.parse(JSON.stringify(state.window.windows))
  applyWindowAction(state, 'window_reset_initial')
  assert.deepEqual(state.window.windows, initial)
  assert.equal(state.mode, 'window_editor')
  applyWindowAction(state, 'window_undo')
  assert.deepEqual(state.window.windows, arranged)
  applyWindowAction(state, 'window_redo')
  assert.deepEqual(state.window.windows, initial)
})


test('shortcut captions and physical input share platform, ordinary and chained aliases', () => {
  const config = { key_aliases: { Modifier: 'right_ctrl', Exit: 'f10', Leave: 'Exit', windows: { Modifier: 'left_alt' }, macos: { Modifier: 'left_cmd', Exit: 'k' } }, window_restore: { bindings: { 'Modifier+Leave': 'idle' } } }
  for (const [mac, modifier, key, caption] of [[false, 'left_alt', 'f10', 'LEFT ALT+F10'], [true, 'left_cmd', 'k', 'LEFT CMD+K']] as const) {
    assert.equal(shortcutCaption(config, 'Modifier+Leave', mac), caption)
    assert.equal(resolvePhysicalBinding(config, 'window_restore', [modifier, key], key, mac)?.value, 'idle')
  }
  config.key_aliases.windows.Modifier = 'right_ctrl'
  config.key_aliases.Exit = 'f8'
  assert.equal(shortcutCaption(config, 'Modifier+Leave', false), 'RIGHT CTRL+F8')
  assert.equal(resolvePhysicalBinding(config, 'window_restore', ['right_ctrl', 'f8'], 'f8', false)?.value, 'idle')
})


test('region splits reuse gaps across repeated deletion and preserve remaining labels', () => {
  const tree = importTree([], null, { width: 1000, height: 800 })
  for (let i = 0; i < 100; i++) {
    assert.equal(splitSlot(tree, 'right'), true)
    assert.deepEqual(treeSlots(tree).map(slot => slot.id), [1, 2])
    tree.selected = 2
    assert.equal(removeSlot(tree), true)
    assert.equal(tree.selected, 1)
  }
})

test('region size actions grow the selected side on either axis and preserve tiling', () => {
  const area = { width: 1000, height: 800 }
  for (const [split, grow, shrink] of [['right', 'right', 'left'], ['down', 'down', 'up']] as const) {
    for (const selected of [1, 2]) {
      const tree = importTree([], null, area); splitSlot(tree, split); tree.selected = selected
      const size = () => { const slot = treeSlots(tree).find(slot => slot.id === selected)!; return split === 'right' ? slot.rect.width * area.width : slot.rect.height * area.height }
      const initial = size()
      resizeRegionBy(tree, grow, 40, area, [], 0)
      assert(Math.abs(size() - initial - 40) < 1e-6)
      assert(Math.abs(treeSlots(tree).reduce((sum, slot) => sum + slot.rect.width * slot.rect.height, 0) - 1) < 1e-9)
      resizeRegionBy(tree, shrink, 40, area, [], 0)
      assert(Math.abs(size() - initial) < 1e-6)
    }
  }
})

test('region sizing can borrow from another ancestor when the nearest neighbor is constrained', () => {
  const area = { width: 1000, height: 800 }
  const tree = { root: { kind: 'split' as const, axis: 'x' as const, ratio: .6,
    first: { kind: 'split' as const, axis: 'x' as const, ratio: .5, first: { kind: 'slot' as const, id: 1, window: 1 }, second: { kind: 'slot' as const, id: 2, window: 2 } },
    second: { kind: 'slot' as const, id: 3, window: 3 } }, selected: 1, nextSlot: 4 }
  const windows = [280, 300, 100].map((minWidth, i) => ({ id: i + 1, minWidth, minHeight: 100, x: 0, y: 0, width: minWidth, height: 800 }))
  resizeRegionBy(tree, 'right', 40, area, windows, 0)
  assert(Math.abs(treeSlots(tree)[0].rect.width * area.width - 340) < 1e-6)
  for (const slot of treeSlots(tree)) assert(slot.rect.width * area.width >= windows[slot.window! - 1].minWidth - 1e-6)
})

test('ordinary close removes only the target and undo does not reopen it', () => {
  const state = createSimulatorState()
  enterWindow(state)
  const id = state.window.target
  const remaining = state.window.windows.filter(w => w.id !== id).map(w => w.id)
  assert.equal(applyWindowAction(state, 'window_close'), true)
  assert.deepEqual(state.window.windows.map(w => w.id), remaining)
  assert.equal(state.window.target, null)
  applyWindowAction(state, 'window_undo')
  assert.deepEqual(state.window.windows.map(w => w.id), remaining)
})

test('ordinary close targets the active tab and dissolves a one-member group', () => {
  const state = createSimulatorState()
  enterWindow(state)
  const [first, second, third] = state.window.windows
  state.window.tabs.groups = [{ id: 1, members: [first.id, second.id, third.id], active: third.id }]
  state.window.target = first.id
  applyWindowAction(state, 'window_close')
  assert.deepEqual(state.window.tabs.groups[0].members, [first.id, second.id])
  state.window.target = first.id
  applyWindowAction(state, 'window_close')
  assert.equal(state.window.tabs.groups.length, 0)
  assert.ok(state.window.windows.some(w => w.id === first.id))
  assert.ok(!state.window.windows.some(w => w.id === second.id || w.id === third.id))
})
