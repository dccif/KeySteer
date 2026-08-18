import { defineComponent, onBeforeUnmount, onMounted, ref } from 'vue'
import { useData, withBase } from 'vitepress'
import { RELEASE_TAG, RELEASE_URL, RELEASES_URL, releaseAssetUrl } from '../generated/release'

interface DownloadAsset {
  key: string
  platform: 'windows' | 'macos'
  platformLabel: string
  label: string
  description: { zh: string; en: string }
  target: string
}

const ASSETS: DownloadAsset[] = [
  {
    key: 'windows-x64',
    platform: 'windows',
    platformLabel: 'Windows',
    label: 'x86_x64',
    description: { zh: '绝大多数电脑', en: 'Most PCs' },
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

function assetUrl(asset: DownloadAsset) {
  return releaseAssetUrl(asset.target)
}

export default defineComponent({
  name: 'DownloadButton',
  setup() {
    const { lang } = useData()
    const detected = ref<DownloadAsset>(detectAsset())
    const menuOpen = ref(false)
    const isEnglish = () => lang.value === 'en-US'
    const text = (zh: string, en: string) => isEnglish() ? en : zh
    const localPath = (path: string) => withBase(`${isEnglish() ? '/en' : ''}${path}`)

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
      <div class="hero-action-row">
        <div class="hero-download" onClick={(event) => event.stopPropagation()}>
          <a
            class="hero-download-main"
            href={assetUrl(detected.value)}
            title={text(`下载 KeySteer ${detected.value.platformLabel} ${detected.value.label}`, `Download KeySteer for ${detected.value.platformLabel} ${detected.value.label}`)}
          >
            <span class="hero-download-emoji" aria-hidden="true">💾</span>
            <strong>{text('立即下载', 'Download now')}</strong>
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
                {ASSETS.filter((asset) => asset.platform === 'windows').map((asset) => (
                  <a
                    key={asset.key}
                    class={`hero-download-option ${asset.key === detected.value.key ? 'detected' : ''}`}
                    href={assetUrl(asset)}
                    role="menuitem"
                    onClick={closeMenu}
                  >
                    <span class="hero-download-option-emoji" aria-hidden="true">📦</span>
                    <span class="hero-download-option-copy">
                      <strong>{asset.label}</strong>
                      <small>{asset.description[isEnglish() ? 'en' : 'zh']}</small>
                    </span>
                    {asset.key === detected.value.key && <span class="hero-download-option-status">{text('当前设备', 'This device')}</span>}
                  </a>
                ))}
              </div>
              <div class="hero-download-group">
                <div class="hero-download-group-title"><strong>macOS</strong></div>
                {ASSETS.filter((asset) => asset.platform === 'macos').map((asset) => (
                  <a
                    key={asset.key}
                    class={`hero-download-option ${asset.key === detected.value.key ? 'detected' : ''}`}
                    href={assetUrl(asset)}
                    role="menuitem"
                    onClick={closeMenu}
                  >
                    <span class="hero-download-option-emoji" aria-hidden="true">📦</span>
                    <span class="hero-download-option-copy">
                      <strong>{asset.label}</strong>
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
    )
  },
})
