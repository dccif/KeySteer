import assert from 'node:assert/strict'
import test from 'node:test'
import { highlightTomlCode } from '../config-studio/toml-highlight.ts'

test('highlights TOML tokens without treating quoted hashes as comments', () => {
  const html = highlightTomlCode('[normal]\nname = "#hello" # comment\nenabled = true\nspeed = 300')
  for (const kind of ['section', 'key', 'string', 'comment', 'boolean', 'number']) assert.ok(html.includes(`ks-syntax-${kind}`))
  assert.ok(html.includes('ks-syntax-string">&quot;#hello&quot;'))
})

test('escapes pasted markup in tokens and unfinished input', () => {
  for (const source of ['key = "<img src=x onerror=alert(1)>"', '<script>alert(1)</script>', '# <svg onload=alert(1)>', 'x = "unterminated <b>']) {
    const html = highlightTomlCode(source)
    assert.doesNotMatch(html, /<(?:img|script|svg|b)[\s>]/)
    assert.ok(html.includes('&lt;'))
  }
})

test('multiline strings keep comment-like contents in the string token', () => {
  const html = highlightTomlCode('text = """first\n# text\nlast"""\n# real comment')
  assert.equal((html.match(/ks-syntax-comment/g) ?? []).length, 1)
  assert.equal((html.match(/ks-syntax-string/g) ?? []).length, 1)
})
