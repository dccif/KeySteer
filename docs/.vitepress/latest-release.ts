export const RELEASES_URL = 'https://github.com/dccif/KeySteer/releases'
export const LATEST_RELEASE_URL = `${RELEASES_URL}/latest`

const LATEST_RELEASE_API = 'https://api.github.com/repos/dccif/KeySteer/releases/latest'

interface GitHubReleaseAsset {
  name?: unknown
  browser_download_url?: unknown
}

interface GitHubRelease {
  tag_name?: unknown
  html_url?: unknown
  assets?: unknown
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
    url: githubUrl(release.html_url) ?? LATEST_RELEASE_URL,
    assets,
  }
}

export async function fetchLatestRelease(
  targets: readonly string[],
  signal?: AbortSignal,
): Promise<LatestRelease | undefined> {
  const response = await fetch(LATEST_RELEASE_API, {
    headers: {
      Accept: 'application/vnd.github+json',
      'X-GitHub-Api-Version': '2022-11-28',
    },
    signal,
  })

  if (!response.ok) return undefined
  return parseLatestRelease(await response.json(), targets)
}
