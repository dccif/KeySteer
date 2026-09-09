import assert from 'node:assert/strict'
import test from 'node:test'
import { readFileSync } from 'node:fs'
import { createSimulatorState } from './state.ts'
import { applyWindowAction, enterWindow, saveWindowLayout, restoreWindowLayout, windowSelectionKey } from './window.ts'
import { decodeLayoutFile, encodeLayoutFile, instantiateLayout, readSavedLayouts, savedLayoutName } from './window-presets.ts'
import type { RegionTemplate, SavedWindowLayout } from './window-presets.ts'
import { treeSlots } from './window-layout.ts'

function regions(first: number, last: number): RegionTemplate {
  if (first === last) return { kind: 'slot', id: first }
  const mid = Math.floor((first + last) / 2)
  return { kind: 'split', axis: 'x', ratio: .5, first: regions(first, mid), second: regions(mid + 1, last) }
}
function preset(count: number): SavedWindowLayout { return { id: 1, note: '', window_count: count, regions: regions(1, count) } }
test('native binary fixture roundtrips byte for byte and rejects truncation', () => {
  const file = new Uint8Array(readFileSync(new URL('../../../tests/fixtures/window-layouts-v1.kslayout', import.meta.url)))
  const layouts = decodeLayoutFile(file)
  assert.equal(layouts[0].note, 'Code · 阅读'); assert.equal(layouts[1].id, 12)
  assert.deepEqual(encodeLayoutFile(layouts), file)
  for (let length = 0; length < file.length; length++) assert.throws(() => decodeLayoutFile(file.subarray(0, length)))
})
test('editing an imported layout updates its number and keeps other layouts unchanged', () => {
  const layouts = decodeLayoutFile(new Uint8Array(readFileSync(new URL('../../../tests/fixtures/window-layouts-v1.kslayout', import.meta.url))))
  const state = createSimulatorState(); state.pointer = { x: 25, y: 25 }; enterWindow(state); state.window.presets = layouts
  const other = JSON.stringify(layouts[1]); restoreWindowLayout(state, 1)
  applyWindowAction(state, 'window_ratio_right'); saveWindowLayout(state, 'Edited · 中文')
  assert.equal(state.window.presets.length, 2); assert.equal(state.window.presets[0].id, 1)
  assert.equal(JSON.stringify(state.window.presets[1]), other)
  assert.equal(decodeLayoutFile(encodeLayoutFile(state.window.presets))[0].note, 'Edited · 中文')
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
  applyWindowAction(state, 'window_saved_layouts')
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
  applyWindowAction(state, 'window_layout'); applyWindowAction(state, 'window_edit')
  applyWindowAction(state, 'window_save_layout'); assert.equal(state.window.noteOpen, true)
  saveWindowLayout(state, '  中文 🦀  ')
  assert.equal(savedLayoutName(state.window.presets[0]), '中文 🦀')
  const encoded = JSON.stringify(state.window.presets)
  assert.deepEqual(readSavedLayouts(encoded), state.window.presets)
  assert.ok(!encoded.includes('"window":') && !encoded.includes('title') && !encoded.includes('app'))
  const empty = preset(9); empty.window_count = 4
  assert.equal(savedLayoutName(empty), 'Layout 1 · 4 windows / 9 regions')
  assert.throws(() => readSavedLayouts('[{"id":1}]'))
  assert.throws(() => readSavedLayouts(JSON.stringify([{ ...empty, window_count: 10 }])))
})
