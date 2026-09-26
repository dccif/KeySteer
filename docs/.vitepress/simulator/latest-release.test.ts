import assert from 'node:assert/strict'
import test from 'node:test'
import { fetchLatestRelease, loadLatestRelease, LATEST_RELEASE_URL, parseLatestRelease } from '../latest-release.ts'

const targets = [
  'x86_64-pc-windows-msvc',
  'aarch64-apple-darwin',
] as const

test('local preview survives forbidden, offline, timeout and invalid release responses', async (context) => {
  context.mock.method(console, 'warn', () => {})
  const request = context.mock.method(globalThis, 'fetch', async () => new Response(null, { status: 403 }))
  const fallback = { tag: 'Release unavailable · 本地预览', url: LATEST_RELEASE_URL, assets: {} }
  assert.deepEqual(await loadLatestRelease('serve'), fallback)
  request.mock.mockImplementation(async () => { throw new TypeError('offline') })
  assert.deepEqual(await loadLatestRelease('serve'), fallback)
  request.mock.mockImplementation(async () => { throw new DOMException('timed out', 'TimeoutError') })
  assert.deepEqual(await loadLatestRelease('serve'), fallback)
  request.mock.mockImplementation(async () => Response.json({ assets: [] }))
  assert.deepEqual(await loadLatestRelease('serve'), fallback)
})

test('production remains strict while development uses fresh metadata when available', async (context) => {
  const request = context.mock.method(globalThis, 'fetch', async () => new Response(null, { status: 403 }))
  await assert.rejects(loadLatestRelease('build'), /HTTP 403/)
  request.mock.mockImplementation(async () => Response.json({ tag_name: 'v9.8.7', assets: [] }))
  assert.equal((await loadLatestRelease('serve')).tag, 'v9.8.7')
  assert.equal((await loadLatestRelease('build')).tag, 'v9.8.7')
})

test('build fetch uses the supplied signal and token and resolves fresh release metadata', async (context) => {
  const signal = new AbortController().signal
  let requests = 0
  context.mock.method(globalThis, 'fetch', async (url: string, init: RequestInit) => {
    assert.equal(url, 'https://api.github.com/repos/dccif/KeySteer/releases/latest')
    assert.equal(init.signal, signal)
    assert.equal(new Headers(init.headers).get('Authorization'), 'Bearer test-build-token')
    requests += 1
    return Response.json({ tag_name: `v1.2.${requests}`, assets: [] })
  })
  assert.equal((await fetchLatestRelease(targets, signal, 'test-build-token')).tag, 'v1.2.1')
  assert.equal((await fetchLatestRelease(targets, signal, 'test-build-token')).tag, 'v1.2.2')
})

test('build fetch fails on API errors or invalid metadata instead of inventing a version', async (context) => {
  const request = context.mock.method(globalThis, 'fetch', async () => new Response(null, { status: 403 }))
  await assert.rejects(fetchLatestRelease(targets), /HTTP 403/)
  request.mock.mockImplementation(async () => Response.json({ assets: [] }))
  await assert.rejects(fetchLatestRelease(targets), /no valid release tag/)
})

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

  assert.equal(release?.url, 'https://github.com/dccif/KeySteer/releases/tag/v1.2.3')
  assert.deepEqual(release?.assets, {})
})

test('explicit release selection uses the encoded tag and its own asset fallback', async (context) => {
  context.mock.method(globalThis, 'fetch', async (url: string) => {
    assert.equal(url, 'https://api.github.com/repos/dccif/KeySteer/releases/tags/release%2F1.2.0')
    return Response.json({ tag_name: 'release/1.2.0', prerelease: true, assets: [] })
  })
  const release = await loadLatestRelease('build', undefined, ' release/1.2.0 ')
  assert.equal(release.tag, 'release/1.2.0')
  assert.equal(release.url, 'https://github.com/dccif/KeySteer/releases/tag/release%2F1.2.0')
  assert.deepEqual(release.assets, {})
})

test('blank and latest selection resolve stable releases again after a rollback', async (context) => {
  let tag = 'v1.2.3'
  context.mock.method(globalThis, 'fetch', async (url: string) => {
    assert.equal(url, 'https://api.github.com/repos/dccif/KeySteer/releases/latest')
    return Response.json({ tag_name: tag, assets: [] })
  })
  assert.equal((await loadLatestRelease('build', undefined, 'latest')).tag, 'v1.2.3')
  tag = 'v1.2.2'
  assert.equal((await loadLatestRelease('build', undefined, ' ')).tag, 'v1.2.2')
})

test('selection errors never silently publish a different release', async (context) => {
  const request = context.mock.method(globalThis, 'fetch', async () => new Response(null, { status: 404 }))
  await assert.rejects(loadLatestRelease('build', undefined, 'v1.2.0'), /HTTP 404/)
  request.mock.mockImplementation(async () => Response.json({ tag_name: 'v1.2.1' }))
  await assert.rejects(loadLatestRelease('build', undefined, 'v1.2.0'), /does not match/)
  request.mock.mockImplementation(async () => Response.json({ tag_name: 'v1.2.0', draft: true }))
  await assert.rejects(loadLatestRelease('build', undefined, 'v1.2.0'), /not a published stable release/)
  request.mock.mockImplementation(async () => Response.json({ tag_name: 'v1.2.0', prerelease: true }))
  await assert.rejects(loadLatestRelease('build'), /not a published stable release/)
})

test('offline preview keeps an explicitly selected release page', async (context) => {
  context.mock.method(console, 'warn', () => {})
  context.mock.method(globalThis, 'fetch', async () => { throw new TypeError('offline') })
  const release = await loadLatestRelease('serve', undefined, 'v1.2.0')
  assert.equal(release.url, 'https://github.com/dccif/KeySteer/releases/tag/v1.2.0')
  assert.deepEqual(release.assets, {})
})
