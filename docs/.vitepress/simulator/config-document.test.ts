import assert from 'node:assert/strict'
import test from 'node:test'
import { reactive } from 'vue'
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
