import assert from 'node:assert/strict'
import test from 'node:test'
import { reactive } from 'vue'
import { readFile } from 'node:fs/promises'

test('held scroll speed validates, exports and resets to shipped defaults', async () => {
  assert.throws(() => parseConfigDocument('[scroll]\nsteps_per_second = 10'), /steps_per_second/)
  const { setConfigPath, deleteConfigPath } = await import('../config-studio/document.ts')
  const { stringify } = await import('smol-toml')
  const defaults = parseConfigDocument(await readFile(new URL('../../../keysteer.default.toml', import.meta.url), 'utf8')).document
  assert.equal(defaults.scroll.speed, 500)
  for (const rate of [0, .5, 250, 500, 1000, 6000]) {
    const document = parseConfigDocument('[scroll]\nscroll_step = 25').document
    assert.equal(resolveConfigDocument(defaults, document).scroll.speed, 500)
    setConfigPath(document, 'scroll.speed', rate)
    const exported = parseConfigDocument(stringify(document)).document
    assert.equal(exported.scroll.speed, rate)
    assert.equal('steps_per_second' in exported.scroll, false)
    deleteConfigPath(exported, 'scroll.speed')
    assert.equal(resolveConfigDocument(defaults, exported).scroll.speed, 500)
    assert.equal(exported.scroll.scroll_step, 25)
  }
  for (const rate of ['-1', 'nan', 'inf', '-inf', '"10"']) {
    assert.throws(() => parseConfigDocument(`[scroll]\nspeed = ${rate}`), /scroll.speed/)
  }
})

test('macOS scroll edits export at platform scope and reset to shipped defaults', async () => {
  const { setConfigPath, deleteConfigPath } = await import('../config-studio/document.ts')
  const { stringify } = await import('smol-toml')
  const defaults = parseConfigDocument(await readFile(new URL('../../../keysteer.default.toml', import.meta.url), 'utf8')).document
  const document = parseConfigDocument('[platform.macos.scroll]\ninvert_horizontal = true\n[scroll]\nscroll_step = 75').document
  assert.equal(resolveConfigDocument(defaults, document).platform.macos.scroll.invert_vertical, true)
  setConfigPath(document, 'platform.macos.scroll.invert_vertical', false)
  setConfigPath(document, 'scroll.scroll_step_full', 900000)
  const exported = parseConfigDocument(stringify(document)).document
  assert.deepEqual({ ...exported.platform.macos.scroll }, { invert_horizontal: true, invert_vertical: false })
  assert.deepEqual({ ...exported.scroll }, { scroll_step: 75, scroll_step_full: 900000 })
  assert.equal(exported.platform.windows, undefined)
  deleteConfigPath(exported, 'platform.macos.scroll.invert_vertical')
  assert.equal(resolveConfigDocument(defaults, exported).platform.macos.scroll.invert_vertical, true)
  assert.equal(exported.platform.macos.scroll.invert_horizontal, true)
})

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


test('search operation bindings replace defaults and preserve optional point styling', async () => {
  const { stringify } = await import('smol-toml')
  const defaults = parseConfigDocument(await readFile(new URL('../../../keysteer.default.toml', import.meta.url), 'utf8')).document
  assert.deepEqual({ ...defaults.ui_hint.search_bindings }, { ctrl: 'point_toggle', tab: 'point_next', 'ctrl+shift+4': 'color_next' })
  assert.equal(defaults.ui_hint.search_point, undefined)
  const source = '[ui_hint.search_bindings]\nf8 = "point_toggle"\nf9 = "color_next"\n[ui_hint.search_point]\ncolor_formats = ["rgb", "hex"]\nmarker_radius = 9\nmarker_color = { light = "#112233FF", dark = "#FFEEDDFF" }'
  const imported = parseConfigDocument(source).document
  const effective = resolveConfigDocument(defaults, imported)
  assert.deepEqual({ ...effective.ui_hint.search_bindings }, { f8: 'point_toggle', f9: 'color_next' })
  const exported = parseConfigDocument(stringify(imported)).document
  assert.deepEqual(exported.ui_hint.search_point.color_formats, ['rgb', 'hex'])
  assert.equal(exported.ui_hint.search_point.marker_width, undefined)
  assert.deepEqual(Object.keys(resolveConfigDocument(defaults, parseConfigDocument('[ui_hint.search_bindings]').document).ui_hint.search_bindings), [])
  for (const entry of ['color_formats = []', 'color_formats = ["hex", "hex"]', 'marker_radius = 0', 'marker_color = "#112233"']) {
    assert.throws(() => parseConfigDocument('[ui_hint.search_point]\n' + entry))
  }
  assert.throws(() => parseConfigDocument('[ui_hint.search_bindings]\nctrl = "bad"'))
})


test('multi-point field modes and next key validate and round-trip', async () => {
  const { stringify } = await import('smol-toml')
  const { fieldLocation } = await import('../config-studio/navigation.ts')
  const document = parseConfigDocument('[ui_hint.search_point]\nfield_modes = ["switch", "concat", "concat", "switch"]\n[ui_hint.search_bindings]\nctrl = "point_toggle"\n"alt+f9" = "point_next"').document
  assert.deepEqual(parseConfigDocument(stringify(document)).document.ui_hint.search_point.field_modes, ['switch', 'concat', 'concat', 'switch'])
  for (let mask = 0; mask < 16; mask++) {
    const modes = Array.from({ length: 4 }, (_, field) => mask & (1 << field) ? 'concat' : 'switch')
    const source = '[ui_hint.search_point]\nfield_modes = ' + JSON.stringify(modes)
    const parsed = parseConfigDocument(source).document
    assert.deepEqual(parseConfigDocument(stringify(parsed)).document.ui_hint.search_point.field_modes, modes)
  }
  assert.equal(document.ui_hint.search_bindings['alt+f9'], 'point_next')
  assert.deepEqual(fieldLocation('ui_hint.search_point.field_modes'), { page: 'ui_hint', tab: 'behavior' })
  for (const values of ['[]', '["concat"]', '["concat", "concat", "switch", "switch", "switch"]', '["concat", "concat", "all", "switch"]']) {
    assert.throws(() => parseConfigDocument('[ui_hint.search_point]\nfield_modes = ' + values))
  }
})

test('copy keys and color swatch overrides round-trip without expanding defaults', async () => {
  const { stringify } = await import('smol-toml')
  const defaults = parseConfigDocument(await readFile(new URL('../../../keysteer.default.toml', import.meta.url), 'utf8')).document
  assert.deepEqual(defaults.ui_hint.search_copy_keys, ['ctrl+1', 'ctrl+2', 'ctrl+3', 'ctrl+4'])
  const document = parseConfigDocument('[ui_hint]\nsearch_copy_keys = ["ctrl+9", "cmd+2", "alt+3", "alt+4"]\n[ui_hint.search_point.color_preview]\nwidth = 24\nx_offset = -3').document
  const effective = resolveConfigDocument(defaults, document)
  assert.deepEqual(effective.ui_hint.search_copy_keys, ['ctrl+9', 'cmd+2', 'alt+3', 'alt+4'])
  assert.deepEqual({ ...parseConfigDocument(stringify(document)).document.ui_hint.search_point.color_preview }, { width: 24, x_offset: -3 })
  for (const entry of ['width = 0', 'height = 65', 'x_offset = -201', 'y_offset = 201', 'border_width = 11', 'enabled = 1', 'unknown = 1']) {
    assert.throws(() => parseConfigDocument('[ui_hint.search_point.color_preview]\n' + entry))
  }
})


test('Point input colors are optional, theme aware, and stay on the appearance page', async () => {
  const { searchPanelColors } = await import('./search-input-style.ts')
  const { fieldLocation } = await import('../config-studio/navigation.ts')
  const { stringify } = await import('smol-toml')
  const defaults = parseConfigDocument(await readFile(new URL('../../../keysteer.default.toml', import.meta.url), 'utf8')).document
  const config = parseConfigDocument('[ui_hint.search_point]\ninput_background_color = { light = "#11223344", dark = "#55667788" }\ninput_border_color = "#99AABBCC"').document
  for (const appearance of ['light', 'dark']) {
    const normal = searchPanelColors(defaults, appearance)
    assert.equal(normal.point_background_color, appearance === 'dark' ? '#284D44FF' : '#E8F6F0FF')
    assert.equal(normal.point_border_color, normal.border_color)
    const custom = searchPanelColors(resolveConfigDocument(defaults, config), appearance)
    assert.equal(custom.point_background_color, appearance === 'dark' ? '#55667788' : '#11223344')
    assert.equal(custom.point_border_color, '#99AABBCC')
  }
  assert.deepEqual(parseConfigDocument(stringify(config)).document.ui_hint.search_point, config.ui_hint.search_point)
  for (const field of ['input_background_color', 'input_border_color']) {
    assert.deepEqual(fieldLocation('ui_hint.search_point.' + field), { page: 'ui_hint', tab: 'appearance' })
    assert.throws(() => parseConfigDocument('[ui_hint.search_point]\n' + field + ' = "invalid"'))
  }
  assert.deepEqual(fieldLocation('ui_hint.search_input_ui.border_color'), { page: 'ui_hint', tab: 'appearance' })
})

test('search match priorities validate, round-trip and reset without expanding sparse documents', async () => {
  const { setConfigPath, deleteConfigPath } = await import('../config-studio/document.ts')
  const { stringify } = await import('smol-toml')
  const { fields } = await import('../config-studio/fields.ts')
  const defaults = parseConfigDocument(await readFile(new URL('../../../keysteer.default.toml', import.meta.url), 'utf8')).document
  assert.deepEqual(defaults.ui_hint.search_match_priority, ['pinyin', 'text', 'label'])
  const document = parseConfigDocument('[ui_hint]\nhint_characters = "asdf"').document
  setConfigPath(document, 'ui_hint.search_match_priority', ['label', 'text', 'pinyin'])
  assert.deepEqual(parseConfigDocument(stringify(document)).document.ui_hint.search_match_priority, ['label', 'text', 'pinyin'])
  deleteConfigPath(document, 'ui_hint.search_match_priority')
  assert.equal('search_match_priority' in document.ui_hint, false)
  assert.deepEqual(resolveConfigDocument(defaults, document).ui_hint.search_match_priority, ['pinyin', 'text', 'label'])
  const field = fields.ui_hint.advanced.find(field => field.path === 'ui_hint.search_match_priority')
  assert.equal(field?.requireAll, true)
  for (const value of ['[]', '["label"]', '["label", "label", "pinyin"]', '["text", "pinyin", "unknown"]', '"label"']) {
    assert.throws(() => parseConfigDocument(`[ui_hint]\nsearch_match_priority = ${value}`), /search_match_priority/)
  }
})

test('search editing key-action tables inherit actions and migrate legacy imports', async () => {
  const { stringify } = await import('smol-toml')
  const { normalizeSearchEditBindings } = await import('../config-studio/document.ts')
  const defaults = parseConfigDocument(await readFile(new URL('../../../keysteer.default.toml', import.meta.url), 'utf8')).document
  assert.equal(defaults.ui_hint.search_edit_keys['enter / primary+q'], 'accept')
  assert.equal(defaults.ui_hint.search_edit_keys.accept, undefined)
  for (const source of [
    '[ui_hint.search_edit_keys]\nf8 = "accept"\nf9 = "accept"\n"alt+v" = "paste"',
    '[ui_hint.search_edit_keys]\naccept = "f8 f9"\npaste = "alt+v"',
  ]) {
    const document = parseConfigDocument(source).document
    const effective = resolveConfigDocument(defaults, document)
    const bindings = effective.ui_hint.search_edit_keys
    assert.equal(bindings['enter / primary+q'], undefined)
    assert.equal(bindings['primary+v'], undefined)
    assert.equal(bindings.esc, 'cancel')
    assert.equal(bindings['alt+v'], 'paste')
    assert.deepEqual(Object.entries(bindings).filter(([, action]) => action === 'accept').flatMap(([keys]) => keys.split(/\s+/)).sort(), ['f8', 'f9'])
    const exported = parseConfigDocument(stringify(document)).document
    assert.equal(exported.ui_hint.search_edit_keys.accept, undefined)
    assert.equal(exported.ui_hint.search_edit_keys.paste, undefined)
    assert.deepEqual(cloneConfigDocument(resolveConfigDocument(defaults, exported)), cloneConfigDocument(effective))
  }
  const disabled = normalizeSearchEditBindings({ copy: '', cut: '' })
  assert.deepEqual(Object.values(disabled).sort(), ['copy', 'cut'])
  const effective = resolveConfigDocument(defaults, { ui_hint: { search_edit_keys: disabled } })
  assert.equal(effective.ui_hint.search_edit_keys['primary+c'], undefined)
  assert.equal(effective.ui_hint.search_edit_keys['primary+x'], undefined)
  assert.throws(() => parseConfigDocument('[ui_hint.search_edit_keys]\nf9 = "unknown_action"'), /editing actions/)
})
