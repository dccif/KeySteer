import assert from 'node:assert/strict'
import test from 'node:test'
import { reactive } from 'vue'
import { cloneConfigDocument } from '../config-studio/document.ts'

test('Vue reactive configuration can be cloned before a style update', () => {
  const document = reactive({ theme: { dark: { accent_alt: '#8FA2F0FF' } } })
  const clone = cloneConfigDocument(document)

  clone.theme.dark.accent_alt = '#112233FF'
  assert.equal(clone.theme.dark.accent_alt, '#112233FF')
  assert.equal(document.theme.dark.accent_alt, '#8FA2F0FF')
})
