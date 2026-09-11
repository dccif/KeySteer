import assert from 'node:assert/strict'
import test from 'node:test'
import { readFileSync } from 'node:fs'
import { createSimulatorState, applyModeAction } from './state.ts'
import { applyWindowAction, enterWindow, saveWindowPreset, restoreWindowPreset, windowSelectionKey } from './window.ts'
import { decodeWorkspaceFile, encodeWorkspaceFile, instantiateLayout, readSavedPresets, presetName } from './window-presets.ts'
import type { RegionTemplate, SavedWindowPreset } from './window-presets.ts'
import { treeSlots } from './window-layout.ts'

function regions(first: number, last: number): RegionTemplate {
  if (first === last) return { kind: 'slot', id: first }
  const mid = Math.floor((first + last) / 2)
  return { kind: 'split', axis: 'x', ratio: .5, first: regions(first, mid), second: regions(mid + 1, last) }
}
function preset(count: number): SavedWindowPreset { return { id: 1, note: '', window_count: count, template: { kind: 'layout', data: regions(1, count) } } }
test('native binary fixture roundtrips byte for byte and rejects truncation', () => {
  const file = new Uint8Array(readFileSync(new URL('../../../tests/fixtures/workspace.ksw', import.meta.url)))
  const layouts = decodeWorkspaceFile(file)
  assert.equal(layouts[0].note, 'Code · 阅读'); assert.equal(layouts.at(-1)!.id, 12)
  assert.deepEqual(encodeWorkspaceFile(layouts), file)
  for (let length = 0; length < file.length; length++) assert.throws(() => decodeWorkspaceFile(file.subarray(0, length)))
})
test('editing an imported layout updates its number and keeps other layouts unchanged', () => {
  const layouts = decodeWorkspaceFile(new Uint8Array(readFileSync(new URL('../../../tests/fixtures/workspace.ksw', import.meta.url))))
  const state = createSimulatorState(); state.pointer = { x: 25, y: 25 }; enterWindow(state); state.window.presets = layouts
  const other = JSON.stringify(layouts.filter(p => p.id !== 1)); restoreWindowPreset(state, 1)
  applyWindowAction(state, 'window_ratio_right'); saveWindowPreset(state, 'Edited · 中文')
  assert.equal(state.window.presets.length, 3); assert.equal(state.window.presets[0].id, 1)
  assert.equal(JSON.stringify(state.window.presets.filter(p => p.id !== 1)), other)
  assert.equal(decodeWorkspaceFile(encodeWorkspaceFile(state.window.presets))[0].note, 'Edited · 中文')
})
test('saved layouts restore MRU first and retain empty regions', () => {
  const windows = Array.from({ length: 9 }, (_, i) => ({ id: 9 - i, title: 'private', app: 'private', screen: 0, x: 0, y: 0, width: 100, height: 80 }))
  assert.deepEqual(treeSlots(instantiateLayout(preset(4), windows)).map(s => s.window), [9, 8, 7, 6])
  assert.deepEqual(treeSlots(instantiateLayout(preset(9), windows.slice(0, 4))).map(s => s.window), [9, 8, 7, 6, null, null, null, null, null])
})
test('R then 1 restores a saved layout and extra windows stay put', () => {
  const state = createSimulatorState()
  state.pointer = { x: 25, y: 25 }; enterWindow(state)
  state.window.presets = [preset(1)]
  const before = state.window.windows.map(w => ({ ...w }))
  applyWindowAction(state, 'window_restore')
  assert.equal(state.window.library, true)
  assert.equal(windowSelectionKey(state, '1', {}), true)
  assert.equal(state.window.library, false)
  const placed = treeSlots(state.window.tree!)[0].window
  assert.ok(placed !== null)
  for (const window of state.window.windows.filter(w => w.id !== placed)) assert.deepEqual(window, before.find(w => w.id === window.id))
  applyWindowAction(state, 'window_undo')
  assert.deepEqual(state.window.windows, before)
})
test('notes and automatic names persist as geometry without app or window metadata', () => {
  const state = createSimulatorState()
  state.pointer = { x: 25, y: 25 }; enterWindow(state)
  applyWindowAction(state, 'window_quick'); applyWindowAction(state, 'window_editor')
  applyWindowAction(state, 'window_save_layout'); assert.equal(state.window.noteOpen, true)
  saveWindowPreset(state, '  中文 🦀  ')
  assert.equal(presetName(state.window.presets[0]), '中文 🦀')
  const encoded = JSON.stringify(state.window.presets)
  assert.deepEqual(readSavedPresets(encoded), state.window.presets)
  assert.ok(!encoded.includes('"window":') && !encoded.includes('title') && !encoded.includes('app'))
  const empty = preset(9); empty.window_count = 4
  assert.equal(presetName(empty), 'Layout 1')
  assert.throws(() => readSavedPresets('[{"id":1}]'))
  assert.throws(() => readSavedPresets(JSON.stringify([{ ...empty, window_count: 10 }])))
})


test('Delete requires confirmation, preserves IDs and produces a valid empty file', () => {
  const state = createSimulatorState(); enterWindow(state)
  state.window.presets = [{ ...preset(1), id: 2 }, { ...preset(1), id: 7 }]
  applyWindowAction(state, 'window_restore'); applyWindowAction(state, 'window_delete')
  windowSelectionKey(state, '7', {})
  assert.equal(state.window.presets.length, 2)
  assert.equal(state.window.deleteSelection?.id, 7)
  applyWindowAction(state, 'window_restore')
  applyWindowAction(state, 'window_restore'); applyWindowAction(state, 'window_delete')
  applyWindowAction(state, 'window_confirm')
  assert.equal(state.window.presets.length, 2)
  for (const id of [7, 2]) {
    windowSelectionKey(state, String(id), {})
    applyWindowAction(state, 'window_confirm')
    assert.equal(state.mode, 'window_restore')
  }
  assert.deepEqual(decodeWorkspaceFile(encodeWorkspaceFile(state.window.presets)), [])
})

test('Delete refuses a record replaced after selection', () => {
  const state = createSimulatorState(); enterWindow(state)
  state.window.presets = [preset(1)]
  applyWindowAction(state, 'window_restore'); applyWindowAction(state, 'window_delete'); windowSelectionKey(state, '1', {})
  state.window.presets[0] = { ...state.window.presets[0], note: 'Changed' }
  applyWindowAction(state, 'window_confirm')
  assert.equal(state.window.presets.length, 1)
  assert.equal(state.window.deleteSelection, null)
})

test('Restore uses lifecycle targets and failed fitting stays in Restore', () => {
  const state = createSimulatorState(); enterWindow(state)
  state.window.presets = [preset(1)]
  applyWindowAction(state, 'window_restore')
  windowSelectionKey(state, '1', { lifecycle: { after_finish: 'keep' } })
  assert.equal(state.mode, 'window_restore')
  assert.equal(state.window.library, true)
  state.window.windows.forEach(w => { w.minWidth = 5000 })
  const before = JSON.stringify(state.window.windows)
  windowSelectionKey(state, '1', { lifecycle: { after_finish: 'idle' } })
  assert.equal(state.mode, 'window_restore')
  assert.equal(JSON.stringify(state.window.windows), before)
  state.window.windows.forEach(w => { w.minWidth = 10 })
  windowSelectionKey(state, '1', { lifecycle: { after_finish: 'idle' } })
  assert.equal(state.mode, 'idle')
})

test('delete is internal to Restore and toggling preserves its page while cancelling selection', () => {
  const state = createSimulatorState(); enterWindow(state)
  state.window.presets = Array.from({ length: 7 }, (_, i) => ({ ...preset(1), id: i + 1 }))
  applyWindowAction(state, 'window_delete')
  assert.equal(state.mode, 'window')
  assert.equal(state.window.deletingPresets, false)
  applyWindowAction(state, 'window_restore')
  windowSelectionKey(state, 'page_down', {})
  for (const action of ['window_delete', 'window_delete', 'window_delete']) {
    applyWindowAction(state, action)
    assert.equal(state.mode, 'window_restore')
    assert.equal(state.window.libraryPage, 1)
    assert.equal(state.window.deleteSelection, null)
    if (state.window.deletingPresets) windowSelectionKey(state, '7', {})
  }
  applyWindowAction(state, 'window_delete')
  applyWindowAction(state, 'window_confirm')
  assert.equal(state.window.presets.length, 7)
  applyWindowAction(state, 'window_delete')
  applyWindowAction(state, 'window')
  applyWindowAction(state, 'window_restore')
  assert.equal(state.window.deletingPresets, false)
})
