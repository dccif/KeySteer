import assert from 'node:assert/strict'
import test from 'node:test'
import { effectiveBindings, resolveBinding } from './bindings.ts'

test('targeting modes inherit normal bindings after their local bindings', () => {
  const document = {
    hotkeys: { 'primary+e': 'normal' },
    normal: { inherits: ['hotkeys'], bindings: { h: 'move_left', ';': 'left_click' } },
    grid: { inherits: ['hotkeys', 'normal'], bindings: { '`': 'follow' } },
  }
  assert.equal(resolveBinding(document, 'grid', 'h')?.value, 'move_left')
  assert.equal(resolveBinding(document, 'grid', 'primary+e')?.value, 'normal')
  assert.equal(resolveBinding(document, 'grid', '`')?.value, 'follow')
  assert.equal(effectiveBindings(document, 'grid').get(';')?.source, 'normal')
})

test('local none blocks an inherited binding', () => {
  const document = {
    normal: { bindings: { h: 'move_left' } },
    ui_hint: { inherits: ['normal'], bindings: { h: 'none' } },
  }
  assert.equal(resolveBinding(document, 'ui_hint', 'h')?.value, 'none')
  assert.equal(effectiveBindings(document, 'ui_hint').has('h'), false)
})

test('whitespace-separated binding aliases color and resolve each key', () => {
  const document = {
    normal: { bindings: { 'v b': 'fast' } },
  }

  assert.equal(resolveBinding(document, 'normal', 'v')?.value, 'fast')
  assert.equal(resolveBinding(document, 'normal', 'b')?.value, 'fast')
  assert.equal(effectiveBindings(document, 'normal').has('v b'), false)
})
