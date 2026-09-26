export const RELEASES_URL = 'https://github.com/dccif/KeySteer/releases'
export const LATEST_RELEASE_URL = `${RELEASES_URL}/latest`

export const DOWNLOAD_TARGETS = [
  'x86_64-pc-windows-msvc',
  'aarch64-pc-windows-msvc',
  'aarch64-apple-darwin',
  'x86_64-apple-darwin',
] as const

const RELEASES_API = 'https://api.github.com/repos/dccif/KeySteer/releases'

function selectedTag(tag?: string): string | undefined {
  const value = tag?.trim()
  return value && value !== 'latest' ? value : undefined
}

function releaseUrl(tag: string): string {
  return `${RELEASES_URL}/tag/${encodeURIComponent(tag)}`
}

interface GitHubReleaseAsset {
  name?: unknown
  browser_download_url?: unknown
}

interface GitHubRelease {
  tag_name?: unknown
  html_url?: unknown
  assets?: unknown
  draft?: unknown
  prerelease?: unknown
}

export interface LatestRelease {
  tag: string
  url: string
  assets: Readonly<Record<string, string>>
}

function githubUrl(value: unknown): string | undefined {
  if (typeof value !== 'string') return undefined

  try {
    const url = new URL(value)
    return url.protocol === 'https:' && url.hostname === 'github.com'
      ? url.toString()
      : undefined
  } catch {
    return undefined
  }
}

export function parseLatestRelease(value: unknown, targets: readonly string[]): LatestRelease | undefined {
  if (!value || typeof value !== 'object') return undefined

  const release = value as GitHubRelease
  if (typeof release.tag_name !== 'string' || release.tag_name.trim() === '') return undefined

  const assets: Record<string, string> = {}
  if (Array.isArray(release.assets)) {
    for (const entry of release.assets as GitHubReleaseAsset[]) {
      if (!entry || typeof entry.name !== 'string') continue
      const assetName = entry.name
      const downloadUrl = githubUrl(entry.browser_download_url)
      if (!downloadUrl) continue

      const target = targets.find((candidate) => assetName.endsWith(`-${candidate}.zip`))
      if (target) assets[target] = downloadUrl
    }
  }

  return {
    tag: release.tag_name,
    url: githubUrl(release.html_url) ?? releaseUrl(release.tag_name),
    assets,
  }
}

export async function fetchLatestRelease(
  targets: readonly string[],
  signal?: AbortSignal,
  token?: string,
  tag?: string,
): Promise<LatestRelease> {
  const requestedTag = selectedTag(tag)
  const endpoint = requestedTag ? `tags/${encodeURIComponent(requestedTag)}` : 'latest'
  const response = await fetch(`${RELEASES_API}/${endpoint}`, {
    headers: {
      Accept: 'application/vnd.github+json',
      'X-GitHub-Api-Version': '2022-11-28',
      ...(token ? { Authorization: `Bearer ${token}` } : {}),
    },
    signal,
  })

  if (!response.ok) throw new Error(`GitHub release (${requestedTag ?? 'latest'}) request failed: HTTP ${response.status}`)
  const payload: GitHubRelease = await response.json()
  const release = parseLatestRelease(payload, targets)
  if (!release) throw new Error('GitHub release response has no valid release tag')
  if (payload.draft || (!requestedTag && payload.prerelease)) {
    throw new Error('GitHub release is not a published stable release')
  }
  if (requestedTag && release.tag !== requestedTag) {
    throw new Error(`GitHub release tag does not match requested tag: ${requestedTag}`)
  }
  return release
}

/** Development can run offline; publishing must use verified release metadata. */
export async function loadLatestRelease(command: string, token?: string, tag?: string): Promise<LatestRelease> {
  try {
    return await fetchLatestRelease(
      DOWNLOAD_TARGETS,
      AbortSignal.timeout(command === 'serve' ? 3_000 : 15_000),
      token,
      tag,
    )
  } catch (error) {
    if (command !== 'serve') throw error
    console.warn('[docs] 无法获取所选 Release，本地预览继续启动；下载入口将指向 GitHub Releases。')
    const requestedTag = selectedTag(tag)
    return { tag: 'Release unavailable · 本地预览', url: requestedTag ? releaseUrl(requestedTag) : LATEST_RELEASE_URL, assets: {} }
  }
}
