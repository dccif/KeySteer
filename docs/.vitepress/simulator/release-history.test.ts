import test from 'node:test'
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { createMarkdownRenderer } from 'vitepress'
import { releaseHistory } from '../release-history.ts'

const md = await createMarkdownRenderer(process.cwd())
md.use(releaseHistory)

test('both release histories fold old versions without changing content or anchors', async () => {
  for (const file of ['docs/releases/index.md', 'docs/en/releases/index.md']) {
    const source = readFileSync(file, 'utf8')
    const versions = [...source.matchAll(/^## (\d+\.\d+\.\d+)$/gm)]
    const html = md.render(source, {})
    assert.equal((html.match(/<details class="ks-release-history">/g) ?? []).length, versions.length - 1)
    assert.ok(html.indexOf(versions[0][1]) < html.indexOf('<details'))
    assert.doesNotMatch(html, /<details[^>]*\bopen\b/)
    assert.equal(html.replace(/<details class="ks-release-history"><summary>\n|<\/summary>\n|<\/details>\n/g, ''), md.render(source.replace('releaseHistory: true', 'releaseHistory: false'), {}))
  }
})

test('folding is opt-in and ignores headings inside code blocks', async () => {
  const source = '## 1.0.0\n\nLatest\n\n```md\n## 0.9.0\n```\n\n## 0.8.0\n\nOld\n\n## Links\n\nEnd\n'
  assert.doesNotMatch(md.render(source, {}), /<details/)
  const html = md.render('---\nreleaseHistory: true\n---\n' + source, {})
  assert.equal((html.match(/<details/g) ?? []).length, 1)
  assert.ok(html.indexOf('</details>') < html.indexOf('<h2 id="links"'))
})
