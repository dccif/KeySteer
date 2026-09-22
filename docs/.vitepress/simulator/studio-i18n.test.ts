import test from 'node:test'
import assert from 'node:assert/strict'
import { translate } from '../config-studio/messages.ts'
import { pages, categories, tabLabels, utilitySearchFields, searchSettings } from '../config-studio/navigation.ts'
import { commonSearchFields, styleSearchFields, frequentFields, advancedFields } from '../config-studio/fields.ts'

test('all settings metadata has English labels and descriptions', () => {
  const labels = [
    ...pages.map(item => item.label), ...categories.map(item => item.label), ...Object.values(tabLabels),
    ...[...commonSearchFields, ...styleSearchFields, ...utilitySearchFields].map(item => item.label),
    ...[...frequentFields, ...advancedFields].flatMap(item => [item.description, ...(item.options ?? []).map(option => option.label)]),
  ]
  for (const label of labels) {
    assert.equal(translate(label, 'zh'), label)
    assert.doesNotMatch(translate(label, 'en'), /[\u4e00-\u9fff]/, label)
  }
})

test('search accepts Chinese, English and TOML paths without changing metadata', () => {
  const entries = [...commonSearchFields, ...styleSearchFields, ...utilitySearchFields]
  const before = JSON.stringify(entries)
  for (const query of ['起始速度', 'initial speed', 'pointer.initial_speed']) {
    assert.equal(searchSettings(entries, query)[0]?.path, 'pointer.initial_speed')
  }
  assert.ok(searchSettings(entries, 'quick switch border').some(item => item.path === 'quick_switch.ui.border_width'))
  assert.equal(JSON.stringify(entries), before)
})

test('localization preserves commands, unknown text and opaque parameters', () => {
  for (const text of ['normal', 'window_editor', 'primary+s', 'screen next', '我的布局 $& {0}']) {
    assert.equal(translate(text, 'en'), text)
  }
  assert.equal(translate('继承自 {0}', 'en', ['normal']), 'Inherited from normal')
  assert.equal(translate('继承自 {0}', 'zh', ['normal']), '继承自 normal')
  assert.equal(translate('已保存 我的布局 $& {0}', 'en'), 'Saved 我的布局 $& {0}')
  assert.equal(translate('已取消 primary+s 的绑定，不再执行本模式或继承的绑定动作', 'en'), 'Disabled primary+s, including inherited bindings')
})
