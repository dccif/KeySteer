import { computed, defineComponent, onBeforeUnmount, onMounted, ref } from 'vue'
import { useData, withBase } from 'vitepress'
import {
  type DOWNLOAD_TARGETS,
  type LatestRelease,
  RELEASES_URL,
} from '../latest-release'

declare const __KEYSTEER_LATEST_RELEASE__: LatestRelease
const latestRelease = __KEYSTEER_LATEST_RELEASE__

interface DownloadAsset {
  key: string
  platform: 'windows' | 'macos'
  platformLabel: string
  label: string
  description: { zh: string; en: string }
  target: (typeof DOWNLOAD_TARGETS)[number]
  optional?: boolean
}

const ASSETS: DownloadAsset[] = [
  {
    key: 'windows-x64',
    platform: 'windows',
    platformLabel: 'Windows',
    label: 'x64 · AVX2',
    description: { zh: '推荐默认版', en: 'Recommended default' },
    target: 'x86_64-pc-windows-msvc',
  },
  {
    key: 'windows-arm64',
    platform: 'windows',
    platformLabel: 'Windows',
    label: 'ARM64',
    description: { zh: 'Windows on ARM', en: 'Windows on ARM' },
    target: 'aarch64-pc-windows-msvc',
  },
  {
    key: 'macos-apple-silicon',
    platform: 'macos',
    platformLabel: 'macOS',
    label: 'Apple Silicon',
    description: { zh: 'M 系列芯片', en: 'M-series chips' },
    target: 'aarch64-apple-darwin',
  },
  {
    key: 'macos-intel',
    platform: 'macos',
    platformLabel: 'macOS',
    label: 'Intel',
    description: { zh: 'Intel 芯片', en: 'Intel chips' },
    target: 'x86_64-apple-darwin',
  },
  {
    key: 'windows-x64-compatible',
    platform: 'windows',
    platformLabel: 'Windows',
    label: 'x64 · Compatible',
    description: { zh: '旧 CPU 兼容版 · 无需 AVX2 / AVX-512', en: 'Older CPUs · No AVX2 / AVX-512 required' },
    target: 'x86_64-pc-windows-msvc-compatible',
    optional: true,
  },
  {
    key: 'windows-x64-avx512',
    platform: 'windows',
    platformLabel: 'Windows',
    label: 'x64 · AVX-512',
    description: { zh: '可选优化版 · 需支持 AVX-512', en: 'Optional · Requires AVX-512 support' },
    target: 'x86_64-pc-windows-msvc-avx512',
    optional: true,
  },
]

function isAppleSilicon() {
  const userAgentData = (navigator as Navigator & {
    userAgentData?: { architecture?: string }
  }).userAgentData
  if (userAgentData?.architecture === 'arm') return true

  try {
    const canvas = document.createElement('canvas')
    const context = canvas.getContext('webgl')
    const debugInfo = context?.getExtension('WEBGL_debug_renderer_info')
    const renderer = debugInfo
      ? context?.getParameter(debugInfo.UNMASKED_RENDERER_WEBGL) as string
      : ''
    return /Apple GPU|Apple M[1-9]/i.test(renderer)
  } catch {
    return false
  }
}

function detectAsset(): DownloadAsset {
  if (typeof navigator === 'undefined') return ASSETS[0]

  const userAgent = navigator.userAgent
  const userAgentData = (navigator as Navigator & {
    userAgentData?: { architecture?: string }
  }).userAgentData

  if (/Windows/i.test(userAgent)) {
    const isArm = /ARM64|AARCH64/i.test(userAgent) || userAgentData?.architecture === 'arm'
    return ASSETS[isArm ? 1 : 0]
  }

  if (/Macintosh|Mac OS X/i.test(userAgent)) {
    return ASSETS[isAppleSilicon() ? 2 : 3]
  }

  return ASSETS[0]
}

export default defineComponent({
  name: 'DownloadButton',
  setup() {
    const { lang } = useData()
    const detected = ref<DownloadAsset>(ASSETS[0])
    const menuOpen = ref(false)
    const isEnglish = () => lang.value === 'en-US'
    const text = (zh: string, en: string) => isEnglish() ? en : zh
    const localPath = (path: string) => withBase(`${isEnglish() ? '/en' : ''}${path}`)
    const downloadLabel = computed(() => text('立即下载', 'Download now'))
    const assetLabel = (asset: DownloadAsset) => asset.key === 'windows-x64-compatible'
      ? text('x64 · 兼容版', 'x64 · Compatible') : asset.label
    const assetUrl = (asset: DownloadAsset) => (
      latestRelease.assets[asset.target] ?? latestRelease.url
    )

    // Browser architecture detection does not prove support for an instruction set.
    // Keep advanced builds manual, and hide them until the selected release has them.
    const availableAssets = ASSETS.filter((asset) => !asset.optional || latestRelease.assets[asset.target])
    const compatibleUrl = latestRelease.assets['x86_64-pc-windows-msvc-compatible']

    const closeMenu = () => {
      menuOpen.value = false
    }

    onMounted(() => {
      detected.value = detectAsset()
      document.addEventListener('click', closeMenu)
    })

    onBeforeUnmount(() => {
      document.removeEventListener('click', closeMenu)
    })

    return () => (
      <div class="hero-download-section">
        <div class="hero-action-row">
          <div class="hero-download" onClick={(event) => event.stopPropagation()}>
            <a
              class="hero-download-main"
              href={assetUrl(detected.value)}
              title={text(`下载 KeySteer ${detected.value.platformLabel} ${detected.value.label}`, `Download KeySteer for ${detected.value.platformLabel} ${detected.value.label}`)}
            >
              <span class="hero-download-emoji" aria-hidden="true">💾</span>
              <span class="hero-download-copy">
                <strong>{downloadLabel.value}</strong>
                <small>{latestRelease.tag} · {detected.value.key === 'windows-x64' ? text('默认版 · AVX2', 'Default · AVX2') : detected.value.label}</small>
              </span>
            </a>
            <button
              class="hero-download-toggle"
              type="button"
              aria-label={text('选择下载平台和架构', 'Choose download platform and architecture')}
              aria-expanded={menuOpen.value}
              onClick={() => { menuOpen.value = !menuOpen.value }}
            >
              <svg
                class={`hero-download-chevron ${menuOpen.value ? 'open' : ''}`}
                width="16"
                height="16"
                viewBox="0 0 16 16"
                fill="none"
                aria-hidden="true"
              >
                <path d="m4 6 4 4 4-4" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" />
              </svg>
            </button>
            {menuOpen.value && (
              <div class="hero-download-menu" role="menu">
                <div class="hero-download-group">
                  <div class="hero-download-group-title"><strong>Windows</strong></div>
                  {availableAssets.filter((asset) => asset.platform === 'windows').map((asset) => (
                    <a
                      key={asset.key}
                      class={`hero-download-option ${asset.key === detected.value.key ? 'detected' : ''}`}
                      href={assetUrl(asset)}
                      role="menuitem"
                      onClick={closeMenu}
                    >
                      <span class="hero-download-option-emoji" aria-hidden="true">📦</span>
                      <span class="hero-download-option-copy">
                        <strong>{assetLabel(asset)}</strong>
                        <small>{asset.description[isEnglish() ? 'en' : 'zh']}</small>
                      </span>
                      {asset.key === detected.value.key && <span class="hero-download-option-status">{text('当前设备', 'This device')}</span>}
                    </a>
                  ))}
                </div>
                <div class="hero-download-group">
                  <div class="hero-download-group-title"><strong>macOS</strong></div>
                  {availableAssets.filter((asset) => asset.platform === 'macos').map((asset) => (
                    <a
                      key={asset.key}
                      class={`hero-download-option ${asset.key === detected.value.key ? 'detected' : ''}`}
                      href={assetUrl(asset)}
                      role="menuitem"
                      onClick={closeMenu}
                    >
                      <span class="hero-download-option-emoji" aria-hidden="true">📦</span>
                      <span class="hero-download-option-copy">
                        <strong>{assetLabel(asset)}</strong>
                        <small>{asset.description[isEnglish() ? 'en' : 'zh']}</small>
                      </span>
                      {asset.key === detected.value.key && <span class="hero-download-option-status">{text('当前设备', 'This device')}</span>}
                    </a>
                  ))}
                </div>
                <a class="hero-download-all" href={RELEASES_URL} target="_blank" rel="noopener" onClick={closeMenu}>
                  {text('查看全部 Release →', 'View all releases →')}
                </a>
              </div>
            )}
          </div>
          <a class="hero-action-link" href={localPath('/guide/getting-started')}>{text('快速开始', 'Get started')}</a>
          <a class="hero-action-link" href={localPath(isEnglish() ? '/editor/' : '/development/architecture')}>
            {text('一起开发', 'Configuration & simulator')}
          </a>
        </div>
        {detected.value.platform === 'windows' && detected.value.key !== 'windows-arm64' && (
          <details class="hero-cpu-note">
            <summary>{text('Windows 下载哪个版本？', 'Which Windows build should I download?')}</summary>
            <div class="hero-cpu-note-body">
              <p>{text('一般下载默认版即可；如果无法启动，请尝试', 'Use the default build for most PCs. If it will not start, try the ')}{compatibleUrl ? <a href={compatibleUrl}>{text('兼容版', 'compatible build')}</a> : text('兼容版', 'compatible build')}{text('。', '.')}</p>
              <p>{text('启动后使用「检查更新」，程序会自动选择这台电脑支持的最高等级（AVX-512 / AVX2 / Compatible），同一版本也能切换，无需自行判断；下载完成后确认安装即可。', 'Once running, use Check for Updates to automatically select the highest build your PC supports (AVX-512 / AVX2 / Compatible), even within the same version. No manual CPU checks are needed; confirm installation after the download.')}</p>
            </div>
          </details>
        )}
      </div>
    )
  },
})
