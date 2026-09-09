import assert from 'node:assert/strict'
import test from 'node:test'
import { deflateSync } from 'node:zlib'
import { readFileSync } from 'node:fs'
import { decodeLayoutFile } from './window-presets.ts'
import { consumeConfigHandoff } from './config-handoff.ts'

function handoffHash(source: string, version = 'v1'): string {
  const bytes = Buffer.from(source)
  const payload = deflateSync(bytes).toString('base64url')
  return `#ks-config=${version}.${bytes.length}.${payload}`
}

function browser(hash: string) {
  const replacements: string[] = []
  const location = { hash, pathname: '/KeySteer/simulator', search: '?test=1' }
  const history = {
    state: { retained: true },
    replaceState(_data: unknown, _unused: string, url?: string | URL | null) {
      replacements.push(String(url))
    },
  }
  return { location, history, replacements }
}

test('imports UTF-8 TOML locally and removes the fragment from history', async () => {
  const source = '# 注释\n[normal.bindings]\n空格 = "left_click"\n'
  const context = browser(handoffHash(source))
  const result = await consumeConfigHandoff(context.location, context.history)

  assert.deepEqual(result, { kind: 'config', source })
  assert.deepEqual(context.replacements, ['/KeySteer/simulator?test=1'])
})

test('leaves unrelated page anchors untouched', async () => {
  const context = browser('#keyboard')
  assert.deepEqual(await consumeConfigHandoff(context.location, context.history), { kind: 'none' })
  assert.deepEqual(context.replacements, [])
})

test('rejects malformed, damaged, unknown and oversized handoffs after clearing them', async () => {
  const cases = [
    '#ks-config=v1.10.not-valid-base64!',
    '#ks-config=v1.10.eJwDAAAAAAE',
    handoffHash('expanded data').replace('v1.13.', 'v1.1.'),
    handoffHash('x', 'v2'),
    '#ks-config=v1.262145.eJwDAAAAAAE',
    '#ks-config-error=too-large',
  ]
  for (const hash of cases) {
    const context = browser(hash)
    const result = await consumeConfigHandoff(context.location, context.history)
    assert.equal(result.kind, 'error', hash)
    assert.equal(context.replacements.length, 1, hash)
  }
})

test('supports an empty TOML document', async () => {
  const context = browser(handoffHash(''))
  assert.deepEqual(await consumeConfigHandoff(context.location, context.history), {
    kind: 'config',
    source: '',
  })
})
test('one-click handoff imports current bindings and native layout bytes together', async () => {
  const file = readFileSync(new URL('../../../tests/fixtures/window-layouts-v1.kslayout', import.meta.url))
  const source = '# current bindings\n[normal.bindings]\nh = "move_left"\n'
  const context = browser(handoffHash(JSON.stringify({ source, layouts: file.toString('base64url'), layout_error: null }), 'v2'))
  assert.deepEqual(await consumeConfigHandoff(context.location, context.history), { kind: 'config', source, layouts: decodeLayoutFile(file) })
  assert.equal(context.replacements.length, 1)
})
test('a layout read error still imports bindings and explains the missing layouts', async () => {
  const source = '# current bindings\n'
  const context = browser(handoffHash(JSON.stringify({ source, layouts: null, layout_error: 'File is damaged' }), 'v2'))
  assert.deepEqual(await consumeConfigHandoff(context.location, context.history), { kind: 'config', source, layoutError: 'File is damaged' })
})
