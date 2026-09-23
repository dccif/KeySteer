import assert from 'node:assert/strict'
import test from 'node:test'
import { reactive } from 'vue'
import { readFile } from 'node:fs/promises'

test('optional Normal targeting stays absent by default and survives import/export', async () => {
  const { stringify } = await import('smol-toml')
  const defaults = parseConfigDocument(await readFile(new URL('../../../keysteer.default.toml', import.meta.url), 'utf8')).document
  assert.equal(defaults.normal.targeting, undefined)
  for (const targeting of ['[normal.targeting]', '[normal.targeting]\nmethod = "recursive_grid"\nreset_on = []', '[normal.targeting]\ngrid_cols = 2\ngrid_rows = 2\nkeys = "1234"\nmax_depth = 3', '[normal.targeting]\nmethod = "recursive_grid"\nmin_size_width = 8\nlayers = []', '[normal.targeting]\nmethod = "recursive_grid"\n[[normal.targeting.layers]]\ndepth = 0\ngrid_cols = 2\ngrid_rows = 2\nkeys = "1234"']) {
    const imported = parseConfigDocument(targeting).document
    const effective = resolveConfigDocument(defaults, imported)
    const exported = parseConfigDocument(stringify(effective)).document
    assert.deepEqual(cloneConfigDocument(exported.normal.targeting), cloneConfigDocument(imported.normal.targeting))
  }
})

test('complete shipped defaults load with guide-line settings', async () => {
  const source = await readFile(new URL('../../../keysteer.default.toml', import.meta.url), 'utf8')
  const parsed = parseConfigDocument(source).document
  assert.equal(parsed.window.card.guide_line_enabled, true)
  assert.equal(parsed.quick_switch.position, 'mouse')
  assert.equal(parsed.recursive_grid.ui.font_size, 0)
})

test('usage checkpoint and quick-switch options survive sparse import and export', async () => {
  const { readFileSync } = await import('node:fs')
  const { stringify } = await import('smol-toml')
  const defaults = parseConfigDocument(readFileSync(new URL('../../../keysteer.default.toml', import.meta.url), 'utf8')).document
  const uploaded = parseConfigDocument('[mode_usage]\nsave_after_entries = 37\n[quick_switch]\nposition = "mouse"\nblacklist = ["idle", "window_tab"]\n[quick_switch.ui]\ntext_color = "#123456FF"').document
  const effective = resolveConfigDocument(defaults, uploaded)
  assert.equal(effective.mode_usage.save_after_entries, 37)
  assert.equal(effective.quick_switch.hold_ms, 350)
  assert.equal(effective.quick_switch.ui.font_size, 28)
  // TOML tables may have null prototypes; cloning compares configuration data.
  assert.deepEqual(cloneConfigDocument(parseConfigDocument(stringify(effective)).document), cloneConfigDocument(effective))
})

test('window card defaults and custom styles survive browser export', async () => {
  const { readFileSync } = await import('node:fs')
  const { stringify } = await import('smol-toml')
  const defaults = parseConfigDocument(readFileSync(new URL('../../../keysteer.default.toml', import.meta.url), 'utf8')).document
  for (const mode of ['window']) {
    assert.equal(defaults[mode].card.text_width, 260)
    const uploaded = parseConfigDocument(`[${mode}.card]\ntitle_font_size = 30\napp_color = {light = "#123456FF", dark = "#FEDCBAFF"}`).document
    const effective = resolveConfigDocument(defaults, uploaded)
    assert.equal(effective[mode].card.app_font_size, 0)
    assert.equal(effective[mode].card.title_font_size, 30)
    assert.deepEqual(cloneConfigDocument(parseConfigDocument(stringify(effective)).document), cloneConfigDocument(effective))
    for (const child of ['window_quick', 'window_editor', 'window_restore', 'window_tab']) {
      if (child === 'window_editor') assert.deepEqual(effective[child].card.position, ['0%', '50%', '100%', '50%'])
      else assert.equal(effective[child].card, undefined)
      assert.throws(() => parseConfigDocument(`[${child}.card]\ntext_width = 300`), /window.card/)
    }
    for (const invalid of ['text_width = 0', 'line_height = 0.5', 'app_font_size = nan', 'title_color = "bad"']) {
      assert.throws(() => parseConfigDocument(`[${mode}.card]\n${invalid}`), /card/)
    }
  }
})

test('Editor position overrides without duplicating shared card styling', async () => {
  const { stringify } = await import('smol-toml')
  const document = parseConfigDocument('[window.card]\napp_font_size = 24\n[window_editor.card]\nposition = ["20%", "50%", "80%", "50%"]').document
  assert.equal(document.window.card.app_font_size, 24)
  assert.equal(document.window_editor.card.app_font_size, undefined)
  assert.deepEqual(parseConfigDocument(stringify(document)).document, document)
})

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

test('Nested reactive configuration survives binding edits and targeting toggles', async () => {
  const { stringify } = await import('smol-toml')
  const original = reactive({ normal: { bindings: { tab: 'toggle' } } })
  const edited = reactive({ ...original })
  const enabled = cloneConfigDocument(edited)
  enabled.normal.targeting = { method: 'recursive_grid', layers: [reactive({ depth: 0, keys: 'asdf' })] }
  const effective = resolveConfigDocument({}, reactive(enabled))
  const exported = parseConfigDocument(stringify(effective)).document
  assert.equal(exported.normal.targeting.method, 'recursive_grid')
  assert.equal(exported.normal.bindings.tab, 'toggle')
  const disabled = cloneConfigDocument({ ...reactive(enabled) })
  delete disabled.normal.targeting
  disabled.normal.bindings.tab = 'none'
  assert.equal(original.normal.bindings.tab, 'toggle')
  assert.equal(enabled.normal.targeting.layers[0].keys, 'asdf')
  assert.equal(disabled.normal.targeting, undefined)
})

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


test('text input keeps independent editing bindings and configurable temporary Normal', async () => {
  const { resolveLayeredPhysicalBinding } = await import('./bindings.ts')
  const source = await readFile(new URL('../../../keysteer.default.toml', import.meta.url), 'utf8')
  const defaults = parseConfigDocument(source).document
  assert.equal(defaults.normal.bindings['\\'], 'text_input')
  assert.deepEqual(cloneConfigDocument(defaults.text_input.bindings), { 'enter \\ esc': 'normal' })
  assert.deepEqual(defaults.text_input.temporary_mode_keys, ['primary'])
  const examples = source.split('[text_input.bindings]')[1].split('[window]')[0].split('\n').filter(line => line.startsWith('# \"') && line.includes('primary+')).map(line => line.slice(2))
  assert.equal(examples.length, 36)
  const configured = resolveConfigDocument(defaults, parseConfigDocument('[text_input.bindings]\n' + examples.join('\n') + '\n').document)
  assert.equal(resolveLayeredPhysicalBinding(configured, 'text_input', ['left_alt', 'h'], 'h', false)?.value, 'arrow_left')
  assert.equal(resolveLayeredPhysicalBinding(configured, 'text_input', ['left_ctrl', 'left_shift', 'left_alt', 'h'], 'h', false)?.value, 'send ctrl+shift+arrow_left')
  assert.equal(resolveLayeredPhysicalBinding(defaults, 'text_input', ['left_alt', 'h'], 'h', false)?.value, 'move_left')
  assert.equal(resolveLayeredPhysicalBinding(defaults, 'text_input', ['h'], 'h', false), undefined)
  const custom = parseConfigDocument('[text_input]\ninherits = ["normal"]\ntemporary_mode_keys = ["right_ctrl"]\n[text_input.bindings]\nf8 = "normal"').document
  const effective = resolveConfigDocument(defaults, custom)
  assert.deepEqual(effective.text_input.bindings, { f8: 'normal' })
  assert.deepEqual(effective.text_input.inherits, ['normal'])
  assert.deepEqual(effective.text_input.temporary_mode_keys, ['right_ctrl'])
})
