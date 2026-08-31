import assert from 'node:assert/strict'
import test from 'node:test'
import { LATEST_RELEASE_URL, parseLatestRelease } from '../latest-release.ts'

const targets = [
  'x86_64-pc-windows-msvc',
  'aarch64-apple-darwin',
] as const

test('uses the latest release tag and actual GitHub asset URLs', () => {
  const release = parseLatestRelease({
    tag_name: 'v1.2.3',
    html_url: 'https://github.com/dccif/KeySteer/releases/tag/v1.2.3',
    assets: [
      {
        name: 'KeySteer-v1.2.3-x86_64-pc-windows-msvc.zip',
        browser_download_url: 'https://github.com/dccif/KeySteer/releases/download/v1.2.3/windows.zip',
      },
      {
        name: 'KeySteer-v1.2.3-aarch64-apple-darwin.zip',
        browser_download_url: 'https://github.com/dccif/KeySteer/releases/download/v1.2.3/macos.zip',
      },
    ],
  }, targets)

  assert.equal(release?.tag, 'v1.2.3')
  assert.equal(
    release?.assets['x86_64-pc-windows-msvc'],
    'https://github.com/dccif/KeySteer/releases/download/v1.2.3/windows.zip',
  )
  assert.equal(
    release?.assets['aarch64-apple-darwin'],
    'https://github.com/dccif/KeySteer/releases/download/v1.2.3/macos.zip',
  )
})

test('rejects invalid payloads and non-GitHub download URLs', () => {
  assert.equal(parseLatestRelease({}, targets), undefined)

  const release = parseLatestRelease({
    tag_name: 'v1.2.3',
    html_url: 'https://example.com/release',
    assets: [{
      name: 'KeySteer-v1.2.3-x86_64-pc-windows-msvc.zip',
      browser_download_url: 'javascript:alert(1)',
    }],
  }, targets)

  assert.equal(release?.url, LATEST_RELEASE_URL)
  assert.deepEqual(release?.assets, {})
})
