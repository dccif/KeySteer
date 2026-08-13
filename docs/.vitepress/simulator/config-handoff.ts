const CONFIG_PREFIX = '#ks-config='
const CONFIG_ERROR_PREFIX = '#ks-config-error='
const MAX_SOURCE_BYTES = 256 * 1024
const MAX_FRAGMENT_BYTES = 24 * 1024

export type ConfigHandoff =
  | { kind: 'none' }
  | { kind: 'config'; source: string }
  | { kind: 'error'; message: string }

interface BrowserLocation {
  hash: string
  pathname: string
  search: string
}

interface BrowserHistory {
  state: unknown
  replaceState(data: unknown, unused: string, url?: string | URL | null): void
}

/**
 * Capture a KeySteer handoff and remove it from the visible/history URL before
 * doing any asynchronous work. URL fragments are not sent in HTTP requests.
 */
export async function consumeConfigHandoff(
  location: BrowserLocation,
  history: BrowserHistory,
): Promise<ConfigHandoff> {
  const hash = location.hash
  if (!hash.startsWith(CONFIG_PREFIX) && !hash.startsWith(CONFIG_ERROR_PREFIX)) {
    return { kind: 'none' }
  }

  history.replaceState(history.state, '', `${location.pathname}${location.search}`)

  if (hash === `${CONFIG_ERROR_PREFIX}too-large`) {
    return { kind: 'error', message: '当前配置过大，无法自动传递；请手动导入 TOML 文件' }
  }
  if (hash.length - 1 > MAX_FRAGMENT_BYTES) {
    return { kind: 'error', message: '配置传递数据超过安全上限，请手动导入 TOML 文件' }
  }

  const match = /^#ks-config=(v[^.]+)\.(\d+)\.([A-Za-z0-9_-]+)$/.exec(hash)
  if (!match) {
    return { kind: 'error', message: '配置传递数据格式无效，请手动导入 TOML 文件' }
  }
  if (match[1] !== 'v1') {
    return { kind: 'error', message: `模拟器不支持配置传递协议 ${match[1]}` }
  }

  const expectedLength = Number(match[2])
  if (!Number.isSafeInteger(expectedLength) || expectedLength > MAX_SOURCE_BYTES) {
    return { kind: 'error', message: '配置内容超过 256 KiB 安全上限，请手动导入 TOML 文件' }
  }

  try {
    const compressed = decodeBase64Url(match[3])
    const compressedBuffer = new ArrayBuffer(compressed.byteLength)
    new Uint8Array(compressedBuffer).set(compressed)
    const stream = new Blob([compressedBuffer]).stream().pipeThrough(new DecompressionStream('deflate'))
    const bytes = await readBounded(stream, expectedLength)
    if (bytes.byteLength !== expectedLength || bytes.byteLength > MAX_SOURCE_BYTES) {
      throw new Error('配置长度校验失败')
    }
    return {
      kind: 'config',
      source: new TextDecoder('utf-8', { fatal: true }).decode(bytes),
    }
  } catch (error) {
    const detail = error instanceof Error ? error.message : String(error)
    return { kind: 'error', message: `无法读取 KeySteer 配置：${detail}` }
  }
}

async function readBounded(stream: ReadableStream<Uint8Array>, expectedLength: number): Promise<Uint8Array> {
  const reader = stream.getReader()
  const chunks: Uint8Array[] = []
  let length = 0
  while (true) {
    const { done, value } = await reader.read()
    if (done) break
    length += value.byteLength
    if (length > expectedLength || length > MAX_SOURCE_BYTES) {
      await reader.cancel('decompressed configuration exceeds its declared length')
      throw new Error('配置长度校验失败')
    }
    chunks.push(value)
  }
  const output = new Uint8Array(length)
  let offset = 0
  for (const chunk of chunks) {
    output.set(chunk, offset)
    offset += chunk.byteLength
  }
  return output
}

function decodeBase64Url(value: string): Uint8Array {
  const base64 = value.replaceAll('-', '+').replaceAll('_', '/')
  const padded = base64.padEnd(Math.ceil(base64.length / 4) * 4, '=')
  const binary = atob(padded)
  return Uint8Array.from(binary, (character) => character.charCodeAt(0))
}
