import test from 'node:test'
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { stringify } from 'smol-toml'
import { resolveModeIndicator, modeIndicatorPreview } from './mode-indicator.ts'
import { parseConfigDocument, resolveConfigDocument, setConfigPath, deleteConfigPath } from '../config-studio/document.ts'
import { indicatorFields } from '../config-studio/indicator-fields.ts'
import { fieldLocation, pages, searchSettings, utilitySearchFields } from '../config-studio/navigation.ts'

const defaults = parseConfigDocument(readFileSync(new URL('../../../keysteer.default.toml', import.meta.url), 'utf8')).document

test('badge defaults, sparse overrides, reset inheritance and export match configuration', () => {
  const document = parseConfigDocument('[mode_indicator.ui]\nfont_size = 18\nposition = "top_right"').document
  let effective = resolveConfigDocument(defaults, document)
  assert.equal(resolveModeIndicator(effective, 'text_input').enabled, false)
  assert.equal(resolveModeIndicator(effective, 'normal').text, 'Normal')
  setConfigPath(document, 'mode_indicator.modes.normal.ui.font_size', 24)
  effective = resolveConfigDocument(defaults, document)
  assert.equal(resolveModeIndicator(effective, 'normal').ui.font_size, 24)
  assert.equal(resolveModeIndicator(effective, 'normal').ui.position, 'top_right')
  assert.equal(resolveModeIndicator(effective, 'grid').ui.font_size, 18)
  deleteConfigPath(document, 'mode_indicator.modes.normal.ui.font_size')
  assert.equal(resolveModeIndicator(resolveConfigDocument(defaults, document), 'normal').ui.font_size, 18)
  const exported = parseConfigDocument(stringify(document)).document
  assert.deepEqual(exported, document)
  assert.equal(exported.mode_indicator.modes.normal.ui.position, undefined)
  for (const path of ['mode_indicator.ui', 'mode_indicator.modes.normal.ui']) {
    assert.throws(() => parseConfigDocument(`[${path}]\nposition = "invalid"`))
  }
})

test('badge preview follows corners, signed offsets, edges and theme overrides', () => {
  const positions = ['bottom_left', 'bottom_right', 'top_left', 'top_right']
  for (const position of positions) {
    const document = { mode_indicator: { ui: { position, indicator_x_offset: 20, indicator_y_offset: -30, text_color: { light: '#111111FF', dark: '#EEEEEEFF' } } } }
    const badge = modeIndicatorPreview(document, 'normal', 'dark', { x: 50, y: 50 }, undefined, { platform: 'macos', scale: 1 })
    const width = parseFloat(badge.style.width), height = parseFloat(badge.style.height)
    assert.equal(parseFloat(badge.style.left), 500 - (position.endsWith('left') ? width : 0))
    assert.equal(parseFloat(badge.style.top), 270 - (position.startsWith('top') ? height : 0))
    assert.equal(badge.style.color, '#EEEEEEFF')
    assert.equal(modeIndicatorPreview(document, 'normal', 'light', { x: 50, y: 50 }).style.color, '#111111FF')
    for (const point of [0, 100]) {
      const edge = modeIndicatorPreview(document, 'normal', 'dark', { x: point, y: point }, undefined, { platform: 'macos', scale: 1 }).style
      assert.ok(parseFloat(edge.left) >= 0 && parseFloat(edge.left) + width <= 960)
      assert.ok(parseFloat(edge.top) >= 0 && parseFloat(edge.top) + height <= 600)
    }
  }
})

test('Windows preview preserves native placement without scaling configured offsets or visible font', () => {
  const document = resolveConfigDocument(defaults, { mode_indicator: { ui: { position: 'bottom_right', indicator_x_offset: 40, indicator_y_offset: 30 } } })
  for (const [scale, left] of [[1, 523.8], [1.5, 494.6], [2, 469.6]]) {
    const badge = modeIndicatorPreview(document, 'normal', 'light', { x: 50, y: 50 }, undefined, { platform: 'windows', scale })
    assert.equal(badge.style.fontSize, '11px')
    assert.ok(Math.abs(parseFloat(badge.style.width) - 54.2) < .001)
    assert.ok(Math.abs(parseFloat(badge.style.left) - left) < .001)
    assert.equal(badge.style.height, '15px')
    assert.equal(badge.style.top, '330px')
    assert.equal(badge.style.borderRadius, '4px')
    assert.equal(badge.style.boxShadow, 'inset 0 0 0 1px #465FBCFF')
    assert.equal(badge.style.background, '#465FBCFF')
    assert.equal(badge.style.color, '#F8FAFFFF')
  }
})

test('macOS uses logical points at both Retina and non-Retina scale', () => {
  const document = resolveConfigDocument(defaults, { mode_indicator: { ui: { position: 'bottom_right', indicator_x_offset: 40, indicator_y_offset: 30 } } })
  const preview = (scale: number) => modeIndicatorPreview(document, 'normal', 'light', { x: 50, y: 50 }, undefined, { platform: 'macos', scale }).style
  assert.deepEqual(preview(1), preview(2))
  assert.equal(preview(2).left, '520px')
  assert.equal(preview(2).top, '330px')
  assert.equal(preview(2).width, '58px')
  assert.equal(preview(2).height, '20px')
  assert.equal(preview(2).background, '#465FBCFF')
  assert.equal(preview(2).color, '#F8FAFFFF')
})

test('native mode colors and explicit themed overrides remain distinct', () => {
  assert.equal(modeIndicatorPreview(defaults, 'recursive_grid', 'light', { x: 50, y: 50 }).style.background, '#0B2377FF')
  assert.equal(modeIndicatorPreview(defaults, 'window', 'light', { x: 50, y: 50 }).style.background, '#EEF2FFF2')
  const document = resolveConfigDocument(defaults, { mode_indicator: { ui: { background_color: '#112233FF', text_color: '#ABCDEF88' } } })
  for (const platform of ['windows', 'macos']) {
    const style = modeIndicatorPreview(document, 'normal', 'light', { x: 50, y: 50 }, undefined, { platform, scale: 2 }).style
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
