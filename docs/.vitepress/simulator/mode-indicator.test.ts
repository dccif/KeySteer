import test from 'node:test'
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { stringify } from 'smol-toml'
import { resolveModeIndicator, modeIndicatorPreview } from './mode-indicator.ts'
import { cloneConfigDocument, parseConfigDocument, resolveConfigDocument, setConfigPath, deleteConfigPath } from '../config-studio/document.ts'
import { indicatorFields } from '../config-studio/indicator-fields.ts'
import { fieldLocation, pages, searchSettings, utilitySearchFields } from '../config-studio/navigation.ts'

const defaults = parseConfigDocument(readFileSync(new URL('../../../keysteer.default.toml', import.meta.url), 'utf8')).document

test('badge defaults, sparse overrides, reset inheritance and export match configuration', () => {
  const document = parseConfigDocument('[mode_indicator.ui]\nfont_size = 18\nindicator_offset = [20, -30]').document
  let effective = resolveConfigDocument(defaults, document)
  assert.equal(resolveModeIndicator(effective, 'text_input').enabled, false)
  assert.equal(resolveModeIndicator(effective, 'normal').text, 'Normal')
  setConfigPath(document, 'mode_indicator.modes.normal.ui.font_size', 24)
  effective = resolveConfigDocument(defaults, document)
  assert.equal(resolveModeIndicator(effective, 'normal').ui.font_size, 24)
  assert.deepEqual(resolveModeIndicator(effective, 'normal').ui.indicator_offset, [20, -30])
  assert.equal(resolveModeIndicator(effective, 'grid').ui.font_size, 18)
  deleteConfigPath(document, 'mode_indicator.modes.normal.ui.font_size')
  assert.equal(resolveModeIndicator(resolveConfigDocument(defaults, document), 'normal').ui.font_size, 18)
  const exported = parseConfigDocument(stringify(document)).document
  // Edited tables and parsed TOML can have different prototypes.
  assert.deepEqual(cloneConfigDocument(exported), cloneConfigDocument(document))
  assert.equal(exported.mode_indicator.modes.normal.ui.indicator_offset, undefined)
  for (const path of ['mode_indicator.ui', 'mode_indicator.modes.normal.ui']) {
    for (const field of ['position = "bottom_left"', 'indicator_x_offset = -12', 'indicator_y_offset = 18']) {
      assert.throws(() => parseConfigDocument(`[${path}]\n${field}`), /was removed/)
    }
  }
})

test('badge preview follows signed offsets, edges and theme overrides', () => {
  for (const pair of [[20, -30], [-12, 18], [-80, -60], [0, 0]]) {
    const document = { mode_indicator: { ui: { indicator_offset: pair, text_color: { light: '#111111FF', dark: '#EEEEEEFF' } } } }
    const badge = modeIndicatorPreview(document, 'normal', 'dark', { x: 50, y: 50 }, undefined, { platform: 'macos' })
    const width = parseFloat(badge.style.width), height = parseFloat(badge.style.height)
    assert.equal(parseFloat(badge.style.left), 480 + pair[0] - width)
    assert.equal(parseFloat(badge.style.top), 300 + pair[1])
    assert.equal(badge.style.color, '#EEEEEEFF')
    assert.equal(modeIndicatorPreview(document, 'normal', 'light', { x: 50, y: 50 }).style.color, '#111111FF')
    for (const point of [0, 100]) {
      const edge = modeIndicatorPreview(document, 'normal', 'dark', { x: point, y: point }, undefined, { platform: 'macos' }).style
      assert.ok(parseFloat(edge.left) >= 0 && parseFloat(edge.left) + width <= 960)
      assert.ok(parseFloat(edge.top) >= 0 && parseFloat(edge.top) + height <= 600)
    }
  }
})

test('offset pair has identical anchor and dimensions on Windows and macOS', () => {
  for (const platform of ['windows', 'macos']) for (const [pair, left, top] of [[[-12, 18], 413.8, 318], [[40, 30], 465.8, 330]] as const) {
    const document = resolveConfigDocument(defaults, { mode_indicator: { ui: { indicator_offset: [...pair] } } })
    const badge = modeIndicatorPreview(document, 'normal', 'light', { x: 50, y: 50 }, undefined, { platform })
    assert.equal(badge.style.fontSize, '11px')
    assert.ok(Math.abs(parseFloat(badge.style.width) - 54.2) < 1e-9)
    assert.equal(parseFloat(badge.style.left), left)
    assert.equal(badge.style.height, '15px')
    assert.equal(parseFloat(badge.style.top), top)
    assert.equal(badge.style.borderRadius, '4px')
    assert.equal(badge.style.boxShadow, 'inset 0 0 0 1px #465FBCFF')
    assert.equal(badge.style.background, '#465FBCFF')
    assert.equal(badge.style.color, '#F8FAFFFF')
  }
})

test('offset pair enforces i16 bounds and survives export with per-mode inheritance', () => {
  for (const root of ['mode_indicator.ui', 'mode_indicator.modes.normal.ui']) {
    for (const value of ['[-32769, 0]', '[32768, 0]', '[1]', '[1, 2, 3]', '[0.5, 1]']) assert.throws(() => parseConfigDocument(`[${root}]\nindicator_offset = ${value}`))
    assert.doesNotThrow(() => parseConfigDocument(`[${root}]\nindicator_offset = [-32768, 32767]`))
  }
  const document = parseConfigDocument('[mode_indicator.ui]\nindicator_offset = [-12, 18]\n[mode_indicator.modes.normal.ui]\nindicator_offset = [40, 30]').document
  assert.deepEqual(resolveModeIndicator(document, 'normal').ui.indicator_offset, [40, 30])
  assert.deepEqual(resolveModeIndicator(document, 'grid').ui.indicator_offset, [-12, 18])
  assert.deepEqual(parseConfigDocument(stringify(document)).document, document)
})

test('sparse global style keeps the default offset', () => {
  const document = resolveConfigDocument(defaults, parseConfigDocument('[mode_indicator.ui]\nfont_size = 18').document)
  assert.deepEqual(resolveModeIndicator(document, 'normal').ui.indicator_offset, [-12, 18])
})

test('native mode colors and explicit themed overrides remain distinct', () => {
  assert.equal(modeIndicatorPreview(defaults, 'recursive_grid', 'light', { x: 50, y: 50 }).style.background, '#0B2377FF')
  assert.equal(modeIndicatorPreview(defaults, 'window', 'light', { x: 50, y: 50 }).style.background, '#EEF2FFF2')
  const document = resolveConfigDocument(defaults, { mode_indicator: { ui: { background_color: '#112233FF', text_color: '#ABCDEF88' } } })
  for (const platform of ['windows', 'macos']) {
    const style = modeIndicatorPreview(document, 'normal', 'light', { x: 50, y: 50 }, undefined, { platform }).style
    assert.equal(style.background, '#112233FF')
    assert.equal(style.color, '#ABCDEF88')
  }
})

test('shared and per-mode badge controls can be found on appearance pages', () => {
  for (const mode of [undefined, 'normal', 'text_input', 'grid']) {
    for (const field of indicatorFields(mode)) {
      const location = fieldLocation(field.path)
      assert.equal(location.page, mode ?? 'mode_indicator')
      assert.ok(pages.find(page => page.id === location.page)?.tabs.includes('appearance'))
      assert.ok(searchSettings(utilitySearchFields, field.path).some(result => result.path === field.path))
    }
  }
})
