import test from 'node:test'
import assert from 'node:assert/strict'
import { categories, pages, fieldLocation, searchSettings, utilitySearchFields } from '../config-studio/navigation.ts'
import { commonSearchFields, styleSearchFields } from '../config-studio/fields.ts'

test('every existing visual control has a reachable category and tab', () => {
  const entries = [...commonSearchFields, ...styleSearchFields, ...utilitySearchFields]
  assert.ok(entries.length > 100)
  for (const entry of entries) {
    const page = pages.find(page => page.id === entry.page)
    assert.ok(page, entry.path)
    assert.ok(page.tabs.includes(entry.tab), entry.path)
    assert.ok(categories.some(category => category.id === page.category))
  }
  assert.equal(new Set(pages.map(page => page.id)).size, pages.length)
})

test('shared appearance and per-mode behavior retain their original TOML ownership', () => {
  assert.deepEqual(fieldLocation('window.card.position'), {page:'window_card', tab:'appearance'})
  assert.deepEqual(fieldLocation('window_editor.card.position'), {page:'window_editor', tab:'appearance'})
  assert.deepEqual(fieldLocation('window_quick.ui.font_size'), {page:'window_quick', tab:'appearance'})
  assert.deepEqual(fieldLocation('window.screens'), {page:'window', tab:'behavior'})
  assert.deepEqual(fieldLocation('pointer.acceleration'), {page:'normal', tab:'behavior'})
  assert.deepEqual(fieldLocation('theme.dark.text'), {page:'key_help', tab:'appearance'})
})

test('search finds Chinese labels, mode names and exact configuration paths', () => {
  const entries = [...commonSearchFields, ...styleSearchFields, ...utilitySearchFields]
  assert.ok(searchSettings(entries, '停止拖动').some(entry => entry.path === 'normal.auto_release_ms'))
  assert.ok(searchSettings(entries, 'window_editor resize_speed').some(entry => entry.path === 'window_editor.resize_speed'))
  assert.ok(searchSettings(entries, 'window.card.position').some(entry => entry.page === 'window_card'))
  assert.ok(searchSettings(entries, '触发键').some(entry => entry.path === 'quick_switch.key'))
  assert.deepEqual(searchSettings(entries, '  '), [])
  assert.deepEqual(searchSettings(entries, 'missing_nonexistent_setting'), [])
})
