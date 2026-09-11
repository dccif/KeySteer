import assert from 'node:assert/strict'
import test from 'node:test'
import { readFileSync } from 'node:fs'
import { createSimulatorState, applyModeAction } from './state.ts'
import { switchWindowMode, windowSelectionKey, applyWindowAction, restoreWindowPreset, chooseWindowNumber, availableWindowPresets, replaceWindowPresets, saveWindowPreset, hasWindowPresetChanges } from './window.ts'
import { treeSlots } from './window-layout.ts'
import { chooseTabTarget, tabAction, selectTemplateMember, autoGroupTabs, pruneTabGroups } from './window-tabs.ts'
import { encodeWorkspaceFile, decodeWorkspaceFile, presetName } from './window-presets.ts'

function setup(count = 4) {
  const state = createSimulatorState()
  state.window.windows = Array.from({ length: count }, (_, i) => ({ id: i + 1, app: `app${String(i).padStart(3, '0')}`, title: `Window ${i + 1}`, screen: 0, x: i * 10, y: 100, width: 400, height: 300 }))
  state.pointer = { x: 99, y: 99 }; switchWindowMode(state, 'window_tab')
  return state
}
function keys(state: ReturnType<typeof setup>, text: string) {
  for (const key of text) {
    const action = key === 't' ? 'window_tab_end' : key === '~' ? 'window_tab_group' : key === ' ' ? 'window_number_end' : null
    if (action) applyWindowAction(state, action)
    else windowSelectionKey(state, key, {})
  }
}

test('Tabs consumes move verbs as reorder and selection without moving the pointer', () => {
  const state = setup()
  keys(state, '123t')
  const pointer = { ...state.pointer }
  assert.equal(applyWindowAction(state, 'move_left'), true)
  assert.deepEqual(state.window.tabs.groups[0].members, [1, 3, 2])
  assert.deepEqual(state.pointer, pointer)
  applyWindowAction(state, 'move_right')
  assert.deepEqual(state.window.tabs.groups[0].members, [1, 2, 3])
  applyWindowAction(state, 'move_up')
  assert.equal(state.window.tabs.groups[0].active, 2)
  applyWindowAction(state, 'move_down')
  assert.equal(state.window.tabs.groups[0].active, 3)
})

test('Window cycles within the active group and number selection can leave it', () => {
  const state = setup()
  keys(state, '12t')
  switchWindowMode(state, 'window')
  chooseWindowNumber(state, 1, {})
  assert.equal(state.window.tabs.groups[0].active, 1)
  applyWindowAction(state, 'window_select')
  assert.equal(state.window.target, 2)
  assert.equal(state.window.tabs.groups[0].active, 2)
  applyWindowAction(state, 'window_select')
  assert.equal(state.window.target, 1)
  applyWindowAction(state, 'window_select_previous')
  assert.equal(state.window.target, 2)
  assert.equal(state.window.tabs.groups[0].active, 2)
  chooseWindowNumber(state, 1, {})
  assert.equal(state.window.tabs.groups[0].active, 1)
  chooseWindowNumber(state, 3, {})
  assert.equal(state.window.target, 3)
  applyWindowAction(state, 'window_select')
  assert.equal(state.window.target, 4)
})

test('tab modes and templates remain available on both supported platforms', () => {
  const state = createSimulatorState()
  switchWindowMode(state, 'window_tab'); assert.equal(state.mode, 'window_tab')
  const layouts = decodeWorkspaceFile(new Uint8Array(readFileSync(new URL('../../../tests/fixtures/workspace.ksw', import.meta.url))))
  replaceWindowPresets(state, layouts)
  assert(availableWindowPresets(state.window).some(p => p.template.kind === 'tabs'))
  restoreWindowPreset(state, layouts.find(p => p.template.kind === 'tabs')!.id); assert.equal(state.mode, 'window_tab')
})
test('typed Rust/browser fixture stays byte-identical', () => {
  const bytes = new Uint8Array(readFileSync(new URL('../../../tests/fixtures/workspace.ksw', import.meta.url)))
  const layouts = decodeWorkspaceFile(bytes)
  assert.equal((layouts.find(p => p.id === 3)?.template.data as import('./window-presets.ts').TabTemplate).active, 2)
  assert.deepEqual(encodeWorkspaceFile(layouts), bytes)
})
test('12t34t composes two independent groups and leaving preserves them', () => {
  const state = setup(); keys(state, '12t34t')
  assert.deepEqual(state.window.tabs.groups.map(g => g.members), [[1, 2], [3, 4]])
  assert.equal(state.window.tabs.target, null)
  const history = state.window.tabs.history.length; keys(state, 'tt')
  assert.equal(state.window.tabs.history.length, history)
  applyModeAction(state, 'idle'); assert.equal(state.window.tabs.groups.length, 2)
})

test('group labels reuse gaps and start at one after all groups dissolve', () => {
  const state = setup(); keys(state, '12t34t~1 ')
  tabAction(state, 'window_tab_dissolve')
  keys(state, '12t')
  assert.equal(state.window.tabs.groups.find(g => g.members.includes(1))?.id, 1)
  assert.equal(state.window.tabs.groups.find(g => g.members.includes(3))?.id, 2)
  for (const id of [1, 2]) {
    keys(state, `~${id} `); tabAction(state, 'window_tab_dissolve')
  }
  for (let i = 0; i < 3; i++) {
    keys(state, '12')
    assert.equal(state.window.tabs.groups[0].id, 1)
    tabAction(state, 'window_tab_dissolve')
  }
})
test('spaces disambiguate 1 and 2 from 12; group prefix merges whole groups', () => {
  const state = setup(23); keys(state, '1 2 t3 4 t~1 ~2 t')
  assert.deepEqual(state.window.tabs.groups.map(g => g.members), [[1, 2, 3, 4]])
  keys(state, '12 t'); assert.equal(state.window.tabs.groups.length, 1)
  assert.equal(state.window.target, 12)
})
test('window transfer removes one member; group selection activation does not add history', () => {
  const state = setup(); keys(state, '123t~1 ')
  const history = state.window.tabs.history.length; keys(state, '2')
  assert.equal(state.window.tabs.history.length, history)
  keys(state, 't42t'); assert.deepEqual(state.window.tabs.groups.map(g => g.members), [[1, 3], [4, 2]])
  tabAction(state, 'window_undo'); assert.deepEqual(state.window.tabs.groups.map(g => g.members), [[1, 2, 3]])
  tabAction(state, 'window_redo'); assert.deepEqual(state.window.tabs.groups.map(g => g.members), [[1, 3], [4, 2]])
})
test('auto grouping is one undo and leaves no composition target', () => {
  const state = setup(); state.window.windows.forEach(w => { w.app = 'Explorer' }); autoGroupTabs(state)
  assert.equal(state.window.tabs.groups.length, 1); assert.equal(state.window.tabs.target, null)
  assert.equal(state.window.tabs.history.length, 1); tabAction(state, 'window_undo')
  assert.equal(state.window.tabs.groups.length, 0)
})
test('failed joining rolls back geometry and membership; closed windows stay closed after undo', () => {
  const state = setup(); keys(state, '12')
  const before = structuredClone(state.window.tabs.groups); state.window.windows[2].minWidth = 2000
  assert.equal(chooseTabTarget(state, { kind: 'window', id: 3 }), false)
  assert.deepEqual(state.window.tabs.groups, before)
  state.window.windows = state.window.windows.filter(w => w.id !== 2); pruneTabGroups(state.window)
  tabAction(state, 'window_undo'); assert.ok(!state.window.windows.some(w => w.id === 2))
})
test('Tab template mixed codec and deferred restore cancellation preserve windows', () => {
  const template = { id: 2, note: 'Tabs', window_count: 2, template: { kind: 'tabs' as const, data: { region: { x: .1, y: .2, width: .6, height: .5 }, active: 1 } } }
  const layouts = [{ id: 1, note: 'Layout', window_count: 1, template: { kind: 'layout' as const, data: { kind: 'slot' as const, id: 1 } } }, template]
  const bytes = encodeWorkspaceFile(layouts); assert.equal(bytes[8], 1); assert.deepEqual(decodeWorkspaceFile(bytes), layouts)
  for (let length = 0; length < bytes.length; length++) assert.throws(() => decodeWorkspaceFile(bytes.subarray(0, length)))
  const state = setup(); state.window.presets = layouts; const before = structuredClone(state.window.windows)
  restoreWindowPreset(state, 2); selectTemplateMember(state, 1); assert.deepEqual(state.window.windows, before)
  switchWindowMode(state, 'window'); assert.deepEqual(state.window.windows, before)
  restoreWindowPreset(state, 2); selectTemplateMember(state, 1); selectTemplateMember(state, 2)
  assert.deepEqual(state.window.tabs.groups[0].members, [1, 2]); assert.equal(state.window.tabs.groups[0].active, 2)
  assert.equal(state.window.windows[1].x, 100); assert.equal(state.window.tabs.restore, null)
})
test('group is one tile target and geometry undo does not dissolve it', () => {
  const state = setup(); keys(state, '12t'); switchWindowMode(state, 'window')
  applyWindowAction(state, 'window_tile')
  assert.notEqual(state.window.windows[0].width, state.window.windows[1].width)
  applyWindowAction(state, 'window_undo'); assert.equal(state.window.tabs.groups.length, 1)
})
test('Editor can select and move a group through a non-anchor member number', () => {
  const state = setup(); keys(state, '12t'); switchWindowMode(state, 'window_editor')
  const slots = treeSlots(state.window.tree!); assert.equal(slots.filter(s => s.window !== null).length, 3)
  const from = slots.find(s => s.window === 1)!, to = slots.find(s => s.window === 3)!
  chooseWindowNumber(state, 2); assert.equal(state.window.target, 2); assert.equal(state.window.swapSource, 1)
  chooseWindowNumber(state, 3)
  assert.equal(treeSlots(state.window.tree!).find(s => s.id === to.id)?.window, 1)
  assert.equal(treeSlots(state.window.tree!).find(s => s.id === from.id)?.window, 3)
  assert.notEqual(state.window.windows[0].width, state.window.windows[1].width)
})

test('only the active member moves and the next member is aligned on selection', () => {
  const state = setup(); keys(state, '12t'); switchWindowMode(state, 'window')
  const hidden = structuredClone(state.window.windows[0])
  applyWindowAction(state, 'window_right')
  applyWindowAction(state, 'window_down')
  assert.deepEqual(state.window.windows[0], hidden)
  const active = state.window.windows[1]
  chooseWindowNumber(state, 1)
  assert.equal(state.window.tabs.groups[0].active, 1)
  assert.equal(state.window.windows[0].x, active.x)
  assert.equal(state.window.windows[0].y, active.y)
  const previouslyActive = structuredClone(active)
  applyWindowAction(state, 'window_right')
  assert.deepEqual(state.window.windows[1], previouslyActive)
})


test('Tabs uses shared names and save updates; restored content is unchanged', () => {
  const state = setup(); keys(state, '12')
  saveWindowPreset(state, '')
  assert.equal(presetName(state.window.presets[0]), 'Tabs 1')
  assert.equal(hasWindowPresetChanges(state.window), false)
  saveWindowPreset(state, '  Reading  ')
  assert.equal(state.window.presets.length, 1)
  assert.equal(presetName(state.window.presets[0]), 'Reading')
  const file = encodeWorkspaceFile(state.window.presets)
  const restored = setup(); replaceWindowPresets(restored, decodeWorkspaceFile(file))
  restoreWindowPreset(restored, 1)
  selectTemplateMember(restored, 3); selectTemplateMember(restored, 4)
  assert.equal(hasWindowPresetChanges(restored.window), false)
  saveWindowPreset(restored, '')
  assert.equal(restored.window.presets.length, 1)
  assert.equal(presetName(restored.window.presets[0]), 'Tabs 1')
})
