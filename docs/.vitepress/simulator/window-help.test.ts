import assert from 'node:assert/strict'
import test from 'node:test'
import { windowHelpSections, windowHelpGrid, windowHelpActionSupported, type HelpEntry } from './window-help.ts'

const entries = (pairs: string[][]): HelpEntry[] => pairs.map(([id, keys]) => ({ id, keys, action: id }))

test('Window help groups remapped directions and A/E/R/T actions by purpose', () => {
  const plan = windowHelpSections(entries([
    ['window_left', 'B'], ['window_down', 'N'], ['window_up', 'M'], ['window_right', 'V'],
    ['window_quick', 'F9'], ['window_editor', 'F8'], ['window_restore', 'F7'], ['window_tab', 'F6'],
    ['window_undo', 'U'], ['idle', 'ESCAPE'], ['send:custom', 'F1'],
  ]), 'window')
  assert.equal(plan.exit, 'ESCAPE')
  assert.equal(plan.left[1].keys, 'B/N/M/V')
  assert.ok(plan.left.some(entry => entry.keys === 'F1'))
  assert.deepEqual(plan.modes.map(entry => entry.keys), ['F9', 'F8', 'F7', 'F6'])
  const grid = windowHelpGrid(plan, 900)
  assert.equal(grid.columns, 2)
  const middle = grid.entries.length / 2
  assert.ok(grid.entries.slice(0, middle).some(entry => entry.keys === 'B/N/M/V'))
  assert.ok(grid.entries.slice(middle).some(entry => entry.keys === 'U'))
  assert.equal(windowHelpGrid(plan, 500).columns, 1)
})

test('mixed modifiers and long custom keys retain their complete configured spelling', () => {
  const plan = windowHelpSections(entries([['window_ratio_left', 'CTRL+H'], ['window_ratio_right', 'F9'], ['window_split_up', 'SHIFT+K']]), 'window_editor')
  assert.ok(plan.left.some(entry => entry.keys === 'F9' && entry.id === 'window_ratio_right'))
  assert.ok(plan.left.some(entry => entry.keys === 'CTRL+H' && entry.id === 'window_ratio_left'))
  const keys = 'RIGHT CTRL+RIGHT SHIFT+F12 / LEFT ALT+F9'
  const custom = windowHelpSections([{ id: 'send:custom', keys, action: 'Custom action' }], 'window')
  assert.ok(custom.left.some(entry => entry.keys === keys))
})

test('return hint follows the local destination while inherited launchers stay common', () => {
  const plan = windowHelpSections([
    { id: 'normal', keys: 'F9 / ALT+E', action: 'Normal', returnCandidate: true },
    { id: 'window', keys: 'ALT+W', action: 'Window' },
    { id: 'idle', keys: 'ALT+Q', action: 'Idle', returnCandidate: true },
  ], 'window_editor')
  assert.equal(plan.exit, 'F9 / ALT+E')
  assert.equal(plan.exitLabel, 'Back → Normal')
  assert.ok(plan.right.some(entry => entry.id === 'window'))
  assert.ok(plan.right.some(entry => entry.id === 'idle'))
})

test('Tabs groups all four inherited motion keys without absorbing Tab shortcuts', () => {
  const input = entries([['move_left', 'B'], ['move_down', 'N'], ['move_up', 'M'], ['move_right', 'V'],
    ['window_tab_previous', 'SHIFT+TAB'], ['window_tab_next', 'TAB']])
  const plan = windowHelpSections(input, 'window_tab')
  assert.ok(plan.left.some(entry => entry.keys === 'B/N/M/V' && entry.action === 'Move / switch tab'))
  assert.ok(plan.left.some(entry => entry.keys === 'TAB / SHIFT+TAB' && entry.action === 'Next / previous tab'))
  assert.ok(!plan.left.some(entry => entry.action.startsWith('move_')))
  const ordinary = windowHelpSections(input, 'window')
  assert.ok(!ordinary.left.some(entry => entry.keys === 'B/N/M/V'))
})

test('persistent help lists supported save and confirm keys without transient readiness', () => {
  assert.equal(windowHelpActionSupported('window_editor', 'window_save_layout'), true)
  assert.equal(windowHelpActionSupported('window_restore', 'window_confirm'), true)
  assert.equal(windowHelpActionSupported('window_restore', 'window_delete'), true)
  assert.equal(windowHelpActionSupported('window', 'window_save_layout'), false)
  assert.equal(windowHelpActionSupported('window_quick', 'window_save_layout'), false)
})


test('Quick screen preview keeps configured labels and screen aspect', async () => {
  const { splitRatioTicks, quickRulerPlan } = await import('./window-ratios.ts')
  const ticks = splitRatioTicks(['1/2', '1/3', .3, '0.45', '0.4501'])
  assert.deepEqual(ticks.map(t => t.label), ['0.3', '1/3', '0.45', '0.4501', '1/2', '1'])
  for (const aspect of [16/9, 9/16]) {
    const plan = quickRulerPlan(ticks.map(t => t.value), ticks.map(t => t.label), {x:.55, y:0, width:.45, height:.3}, 280, 12, 140, aspect)
    assert.ok(Math.abs(plan.frame.width / plan.frame.height - aspect) < 1e-9)
    assert.deepEqual(plan.captions.map(c => c.text), ['0.45', '0.3'])
    assert.ok(Math.abs(plan.selection.x + plan.selection.width - plan.frame.x - plan.frame.width) < 1e-9)
  }
})


test('mute help merges actual Shift bindings and preserves custom bindings', () => {
  for (const mode of ['window', 'window_quick', 'window_editor']) {
    assert.equal(windowHelpActionSupported(mode, 'window_system_volume_mute'), true)
    const plan = windowHelpSections(entries([['window_volume_mute', 'V+M'], ['window_system_volume_mute', 'SHIFT+V+M']]), mode)
    assert.ok(plan.right.some(e => e.keys === 'V+M' && e.action === 'Mute / unmute · Shift: system'))
    const custom = windowHelpSections(entries([['window_volume_mute', 'F8'], ['window_system_volume_mute', 'F9']]), mode)
    assert.ok(custom.right.some(e => e.keys === 'F8 / F9' && e.action === 'App / system Mute / unmute'))
  }
  assert.equal(windowHelpActionSupported('window_tab', 'window_system_volume_mute'), false)
})
