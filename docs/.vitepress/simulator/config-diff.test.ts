import assert from 'node:assert/strict'
import test from 'node:test'
import { diffToml } from './config-diff.ts'

test('ignores formatting, comments, ordering and table spelling', () => {
  assert.deepEqual(diffToml('# source\n[a]\nx = 1\ny = "ok"', 'a = { y = "ok", x = 1 }'), [])
  assert.deepEqual(diffToml('a = [{x=1,y=2}]', 'a = [{y=2,x=1}]'), [])
})

test('reports additions, removals, value changes and quoted paths without inherited defaults', () => {
  assert.deepEqual(diffToml('[normal.bindings]\n"a.b" = "left"\na = "old"', '[normal.bindings]\n"a.b" = "right"\nb = "new"'), [
    { path: 'normal.bindings.a', kind: 'removed', before: '"old"', after: undefined },
    { path: 'normal.bindings."a.b"', kind: 'modified', before: '"left"', after: '"right"' },
    { path: 'normal.bindings.b', kind: 'added', before: undefined, after: '"new"' },
  ])
  assert.deepEqual(diffToml('', '# empty'), [])
  assert.equal(diffToml('', '[normal]')[0].kind, 'added')
})

test('preserves arrays, types, dates, infinities and empty tables', () => {
  for (const [left, right] of [['[1,2]', '[2,1]'], ['false', '"false"'], ['nan', 'inf'], ['{}', '[]'], ['1979-05-27', '1979-05-28']]) {
    assert.equal(diffToml(`x = ${left}`, `x = ${right}`)[0].kind, 'modified')
  }
  assert.deepEqual(diffToml('x = nan\nd = 1979-05-27', 'd = 1979-05-27\nx = nan'), [])
  assert.equal(diffToml('x = 1', '[x]\ny = 2')[0].kind, 'modified')
  assert.equal(diffToml('constructor = 1', 'constructor = 2')[0].path, 'constructor')
})

test('accepts arbitrary TOML schemas, but rejects malformed files', () => {
  assert.equal(diffToml('[other]\nvalue = 1', '[other]\nvalue = 2').length, 1)
  assert.throws(() => diffToml('x = [', ''))
  assert.throws(() => diffToml('', 'x = 1\nx = 2'))
  assert.throws(() => diffToml('x = [', 'x = ['))
})

test('unchanged scalar fast path preserves special number differences', () => {
  assert.deepEqual(diffToml('a = nan\nb = inf\nc = "text"\nd = true', 'd = true\nc = "text"\nb = inf\na = nan'), [])
  assert.equal(diffToml('value = inf', 'value = -inf')[0].kind, 'modified')
})
