import assert from 'node:assert/strict'
import test from 'node:test'
import { createSimulatorState } from './state.ts'
import { applyWindowAction, chooseWindowNumber, enterWindow, finishWindowNumber, refreshWindowNumbers, temporaryWindow, tileWindows, windowSelectionKey, windowTarget, WINDOW_AREA } from './window.ts'
import { compileWindowLayoutBindings, resolvePhysicalBinding, resolveWindowLayoutBinding, temporaryPhysicalKeys } from './bindings.ts'
import { automaticTree, importTree, fitTree, treeSlots } from './window-layout.ts'
import { parseSplitRatios } from './window-ratios.ts'

test('E enters automatic editing directly; configured back returns one level and exit_mode controls the root', () => {
  const state = createSimulatorState()
  enterWindow(state)
  applyWindowAction(state, 'window_edit')
  assert.equal(state.window.panel, 'tree')
  applyWindowAction(state, 'window_cancel', { exit_mode: 'idle' })
  assert.equal(state.window.panel, 'none')
  assert.equal(state.mode, 'window')
  applyWindowAction(state, 'window_layout')
  assert.equal(state.window.panel, 'quick')
  applyWindowAction(state, 'window_cancel', { exit_mode: 'idle' })
  assert.equal(state.mode, 'window')
  assert.equal(state.window.panel, 'none')
  applyWindowAction(state, 'window_cancel', { exit_mode: 'idle' })
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
  enterWindow(state); applyWindowAction(state, 'window_layout', settings); applyWindowAction(state, 'window_edit', settings)
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

test('Window locks the pointer target and returns to the previous selection state', () => {
  const state = createSimulatorState()
  state.mode = 'grid'; state.targeting.grid.path = ['a', 'b']
  enterWindow(state)
  assert.equal(state.window.target, 2)
  state.pointer = { x: 1, y: 1 }
  applyWindowAction(state, 'window_right')
  assert.equal(state.window.target, 2)
  applyWindowAction(state, 'window_exit')
  assert.equal(state.mode, 'grid')
  assert.deepEqual(state.targeting.grid.path, ['a', 'b'])
  assert.equal(state.window.history.length, 0)
})

test('Tab cycles immediately across screens and centers the pointer without opening a panel', () => {
  const state = createSimulatorState()
  enterWindow(state)
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
  applyWindowAction(state, 'window_layout', {}, 1000)
  applyWindowAction(state, 'window_layout', {}, 1200)
  assert.equal(JSON.stringify(state.window.windows), before)
  assert.equal(state.window.panel, 'quick')
  applyWindowAction(state, 'window_edit')
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
  applyWindowAction(state, 'window_layout', {}, 1000)
  applyWindowAction(state, 'window_layout', {}, 1500)
  assert.equal(state.window.history.length, 0)
  assert.equal(windowSelectionKey(state, 'q', {}), false)
  applyWindowAction(state, 'window_layout_left')
  applyWindowAction(state, 'window_layout_left')
  assert.equal(state.mode, 'window')
  assert.equal(state.window.size, true)
  assert.equal(windowTarget(state.window)!.x, 4)
  assert.equal(windowTarget(state.window)!.width, 1000 / 3 - 8)
  const applied = JSON.stringify(state.window.windows)
  applyWindowAction(state, 'window_cancel')
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
  applyWindowAction(state, 'window_layout', {}, 1000)
  const before = JSON.stringify(state.window.windows)
  temporaryWindow(state.window, true)
  applyWindowAction(state, 'window_right')
  temporaryWindow(state.window, false)
  applyWindowAction(state, 'window_layout', {}, 1100)
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
      bindings: { a: 'window_layout', 'primary+q': 'window_exit', x: 'window_undo', z: 'none' } } }
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

test('compiled layout directions honor aliases, inheritance, WASD, and extra modifiers', () => {
  const document = { key_aliases: { west: 'a' }, normal: { inherits: ['hotkeys'], bindings: { west: 'move_left', w: 'move_up', d: 'move_right', s: 'move_down', h: 'none' } }, hotkeys: { h: 'move_left' } }
  const compiled = compileWindowLayoutBindings(document, false)
  assert.equal(resolveWindowLayoutBinding(compiled, ['a'], 'a'), 'window_layout_left')
  assert.equal(resolveWindowLayoutBinding(compiled, ['left_shift', 'a'], 'a'), 'window_split_left')
  assert.equal(resolveWindowLayoutBinding(compiled, ['right_ctrl', 'd'], 'd'), 'window_ratio_right')
  assert.equal(resolveWindowLayoutBinding(compiled, ['h'], 'h'), undefined)
  assert.equal(resolveWindowLayoutBinding(compiled, ['left_alt', 'a'], 'a'), undefined)
})

test('tree splits, moves into empty slots, swaps, and exits without reverting', () => {
  const state = createSimulatorState(); enterWindow(state)
  applyWindowAction(state, 'window_layout')
  const before = JSON.stringify(state.window.windows)
  applyWindowAction(state, 'window_edit')
  assert.equal(state.window.panel, 'tree')
  const count = treeSlots(state.window.tree!).length
  applyWindowAction(state, 'window_split_right')
  assert.equal(treeSlots(state.window.tree!).length, count + 1)
  const empty = treeSlots(state.window.tree!).find(s => s.window === null)!
  const source = state.window.target!
  chooseWindowNumber(state, state.window.numbers[source])
  assert.equal(state.window.swapSource, source)
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
  applyWindowAction(state, 'window_cancel')
  assert.equal(JSON.stringify(state.window.windows), applied)
  applyWindowAction(state, 'window_undo')
  assert.equal(JSON.stringify(state.window.windows), before)
})

test('tree minimum sizes constrain the divider and infeasible splits leave geometry unchanged', () => {
  const state = createSimulatorState()
  state.window.windows = [
    { id: 1, title: 'Small', app: 'Demo', screen: 0, x: 0, y: 0, width: 220, height: 650 },
    { id: 2, title: 'Constrained', app: 'ChatGPT', screen: 0, x: 240, y: 0, width: 760, height: 650, minWidth: 720, minHeight: 620 },
  ]
  enterWindow(state); applyWindowAction(state, 'window_layout'); applyWindowAction(state, 'window_edit')
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
  applyWindowAction(state, 'window_layout'); applyWindowAction(state, 'window_edit')
  applyWindowAction(state, 'window_split_right'); applyWindowAction(state, 'window_ratio_left')
  applyWindowAction(state, 'window_cancel')
  assert.equal(state.window.history.length, 1)
  applyWindowAction(state, 'window_undo')
  assert.equal(JSON.stringify(state.window.windows), before)
  applyWindowAction(state, 'window_layout'); applyWindowAction(state, 'window_edit')
  const count = treeSlots(state.window.tree!).length, numbers = { ...state.window.numbers }
  const closing = state.window.windows.find(w => w.screen === 0 && w.id !== state.window.target)!
  state.window.windows = state.window.windows.filter(w => w.id !== closing.id); refreshWindowNumbers(state.window)
  assert.equal(treeSlots(state.window.tree!).length, count)
  assert.ok(treeSlots(state.window.tree!).some(s => s.window === null))
  assert.deepEqual(state.window.numbers, numbers)
  applyWindowAction(state, 'window_cancel')
  assert.ok(!state.window.windows.some(w => w.id === closing.id))
})


test('ordinary Window directions reuse Normal movement without implicit arrow bindings', () => {
  const bindings = compileWindowLayoutBindings({ normal: { bindings: { v: 'move_left', l: 'move_right' } } }, false)
  assert.equal(resolveWindowLayoutBinding(bindings, ['v'], 'v', false), 'window_left')
  assert.equal(resolveWindowLayoutBinding(bindings, ['left'], 'left', false), undefined)
  assert.equal(resolveWindowLayoutBinding(bindings, ['shift', 'v'], 'v', false), undefined)
  assert.equal(resolveWindowLayoutBinding(bindings, ['v'], 'v'), 'window_layout_left')
})

test('backtick enters auto layout and region selection activates its window', () => {
  const state = createSimulatorState()
  enterWindow(state)
  const before = structuredClone(state.window.windows)
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
