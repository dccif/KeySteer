import { defineComponent, onBeforeUnmount, onMounted, ref } from 'vue'
import { withBase } from 'vitepress'

interface DownloadAsset {
  key: string
  platform: 'windows' | 'macos'
  platformLabel: string
  label: string
  description: string
  fileName: string
}

const RELEASES_URL = 'https://github.com/dccif/KeySteer/releases'
const LATEST_RELEASE_URL = 'https://github.com/dccif/KeySteer/releases/latest'

const ASSETS: DownloadAsset[] = [
  {
    key: 'windows-x64',
    platform: 'windows',
    platformLabel: 'Windows',
    label: 'x64',
    description: '绝大多数电脑',
    fileName: 'KeySteer-x86_64-pc-windows-msvc.zip',
  },
  {
    key: 'windows-arm64',
    platform: 'windows',
    platformLabel: 'Windows',
    label: 'ARM64',
    description: 'Windows on ARM',
    fileName: 'KeySteer-aarch64-pc-windows-msvc.zip',
  },
  {
    key: 'macos-apple-silicon',
    platform: 'macos',
    platformLabel: 'macOS',
    label: 'Apple Silicon',
    description: 'M 系列芯片',
    fileName: 'KeySteer-aarch64-apple-darwin.zip',
  },
  {
    key: 'macos-intel',
    platform: 'macos',
    platformLabel: 'macOS',
    label: 'Intel',
    description: 'Intel 芯片',
    fileName: 'KeySteer-x86_64-apple-darwin.zip',
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

function assetUrl(fileName: string) {
  return `${LATEST_RELEASE_URL}/download/${fileName}`
}

export default defineComponent({
  name: 'DownloadButton',
  setup() {
    const detected = ref<DownloadAsset>(detectAsset())
    const menuOpen = ref(false)

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
            href={assetUrl(detected.value.fileName)}
            title={`下载 KeySteer ${detected.value.platformLabel} ${detected.value.label}`}
          >
            <span class="hero-download-emoji" aria-hidden="true">💾</span>
            <strong>立即下载</strong>
          </a>
          <button
            class="hero-download-toggle"
            type="button"
            aria-label="选择下载平台和架构"
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
                    href={assetUrl(asset.fileName)}
                    role="menuitem"
                    onClick={closeMenu}
                  >
                    <span class="hero-download-option-emoji" aria-hidden="true">📦</span>
                    <span class="hero-download-option-copy">
                      <strong>{asset.label}</strong>
                      <small>{asset.description}</small>
                    </span>
                    {asset.key === detected.value.key && <span class="hero-download-option-status">当前设备</span>}
                  </a>
                ))}
              </div>
              <div class="hero-download-group">
                <div class="hero-download-group-title"><strong>macOS</strong></div>
                {ASSETS.filter((asset) => asset.platform === 'macos').map((asset) => (
                  <a
                    key={asset.key}
                    class={`hero-download-option ${asset.key === detected.value.key ? 'detected' : ''}`}
                    href={assetUrl(asset.fileName)}
                    role="menuitem"
                    onClick={closeMenu}
                  >
                    <span class="hero-download-option-emoji" aria-hidden="true">📦</span>
                    <span class="hero-download-option-copy">
                      <strong>{asset.label}</strong>
                      <small>{asset.description}</small>
                    </span>
                    {asset.key === detected.value.key && <span class="hero-download-option-status">当前设备</span>}
                  </a>
                ))}
              </div>
              <a class="hero-download-all" href={RELEASES_URL} target="_blank" rel="noopener" onClick={closeMenu}>
                查看全部 Release →
              </a>
            </div>
          )}
        </div>
        <a class="hero-action-link" href={withBase('/guide/getting-started')}>快速开始</a>
        <a class="hero-action-link" href={withBase('/development/architecture')}>一起开发</a>
      </div>
    )
  },
})
