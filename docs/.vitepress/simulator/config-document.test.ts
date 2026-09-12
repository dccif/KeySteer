import assert from 'node:assert/strict'
import test from 'node:test'
import { reactive } from 'vue'

test('fraction ratios preserve source strings and reject invalid configurations', () => {
  const parsed = parseConfigDocument('[window_quick]\nsplit_ratios = ["1/5", " 2 / 5 ", "4/5"]')
  assert.deepEqual(parsed.document.window_quick.split_ratios, ['1/5', ' 2 / 5 ', '4/5'])
  for (const values of ['[]', '[0.0]', '[1.0]', '[-0.1]', '[nan]', '[inf]', '["1/0"]', '["0/1"]', '["1/1"]', '["1/4294967296"]']) {
    assert.throws(() => parseConfigDocument(`[window_quick]\nsplit_ratios = ${values}`), /split_ratios/)
  }
})
import {
  cloneConfigDocument,
  getConfigPath,
  parseConfigDocument,
  resolveConfigDocument,
} from '../config-studio/document.ts'

test('Vue reactive configuration can be cloned before a style update', () => {
  const document = reactive({ theme: { dark: { accent_alt: '#8FA2F0FF' } } })
  const clone = cloneConfigDocument(document)

  clone.theme.dark.accent_alt = '#112233FF'
  assert.equal(clone.theme.dark.accent_alt, '#112233FF')
  assert.equal(document.theme.dark.accent_alt, '#8FA2F0FF')
})

test('uploaded TOML is parsed with useful source statistics', () => {
  const parsed = parseConfigDocument('[pointer]\ninitial_speed = 1200.0\n\n[normal]\nlong_press_toggle_ms = 0\n')

  assert.equal(parsed.sections, 2)
  assert.equal(parsed.values, 2)
  assert.ok(parsed.bytes > 20)
  assert.equal(getConfigPath(parsed.document, 'pointer.initial_speed'), 1200)
})

test('sparse struct sections inherit fields from the generated defaults', () => {
  const defaults = parseConfigDocument(`
    [pointer]
    initial_speed = 1000.0
    max_speed = 2200.0
    smooth_acceleration = true
  `).document
  const uploaded = parseConfigDocument('[pointer]\ninitial_speed = 1250.0\n').document
  const effective = resolveConfigDocument(defaults, uploaded)

  assert.equal(effective.pointer.initial_speed, 1250)
  assert.equal(effective.pointer.max_speed, 2200)
  assert.equal(effective.pointer.smooth_acceleration, true)
  assert.equal(uploaded.pointer.max_speed, undefined)
})

test('an explicit binding table replaces that default map like Rust serde', () => {
  const defaults = parseConfigDocument(`
    [normal.bindings]
    h = "move_left"
    j = "move_down"
  `).document
  const uploaded = parseConfigDocument('[normal.bindings]\nh = "move_right"\n').document
  const effective = resolveConfigDocument(defaults, uploaded)

  assert.deepEqual(effective.normal.bindings, { h: 'move_right' })
})

test('a missing binding table still receives the built-in defaults', () => {
  const defaults = parseConfigDocument('[normal.bindings]\nh = "move_left"\n').document
  const uploaded = parseConfigDocument('[normal]\nlong_press_toggle_ms = 800\n').document
  const effective = resolveConfigDocument(defaults, uploaded)

  assert.deepEqual(effective.normal.bindings, { h: 'move_left' })
})


test('shipped audio bindings and edited configuration survive browser export/import', async () => {
  const { readFileSync } = await import('node:fs')
  const { stringify } = await import('smol-toml')
  const defaults = parseConfigDocument(readFileSync(new URL('../../../keysteer.default.toml', import.meta.url), 'utf8')).document
  const expected = {
    'v+j': 'window_volume_down', 'v+k': 'window_volume_up', 'v+m': 'window_volume_mute',
    'v+h': 'window_audio_previous', 'v+l': 'window_audio_next',
    'shift+v+j': 'window_system_volume_down', 'shift+v+k': 'window_system_volume_up',
    'shift+v+m': 'window_system_volume_mute', 'shift+v+h': 'window_system_audio_previous', 'shift+v+l': 'window_system_audio_next',
  }
  for (const mode of ['window', 'window_quick', 'window_editor']) {
    for (const [key, action] of Object.entries(expected)) assert.equal(defaults[mode].bindings[key], action)
    delete defaults[mode].bindings['shift+v+m']
    defaults[mode].bindings.f9 = 'window_system_volume_mute'
  }
  defaults.window_quick.split_ratios = ['1/3', '0.30', '0.45']
  defaults.window_editor.gap = 7
  const restored = parseConfigDocument(stringify(defaults)).document
  for (const mode of ['window', 'window_quick', 'window_editor']) {
    assert.equal(restored[mode].bindings.f9, 'window_system_volume_mute')
    assert.equal(restored[mode].bindings['shift+v+m'], undefined)
    assert.equal(restored[mode].bindings['v+m'], 'window_volume_mute')
  }
  assert.deepEqual(restored.window_quick.split_ratios, ['1/3', '0.30', '0.45'])
  assert.equal(restored.window_editor.gap, 7)
})
