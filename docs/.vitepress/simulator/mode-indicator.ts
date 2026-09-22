import type { ConfigDocument } from '../config-studio/document.ts'

const names: Record<string, string> = { normal: 'Normal', text_input: 'Text Input', grid: 'Grid', recursive_grid: 'Recursive Grid', ui_hint: 'UI Hint', window: 'Window', window_quick: 'Quick', window_editor: 'Edit', window_restore: 'Restore', window_tab: 'Tabs', idle: 'Idle' }
const palettes = {
  light: { surface: '#EEF2FFFF', accent: '#465FBCFF', accent_alt: '#0B2377FF', on_accent_alt: '#F8FAFFFF', text: '#17327AFF' },
  dark: { surface: '#0A1338FF', accent: '#6E82D6FF', accent_alt: '#8FA2F0FF', on_accent_alt: '#081022FF', text: '#E8EEFFFF' },
}
function readableOn(background: string, text: string, alternative: string): string {
  const luminance = (color: string) => {
    const channels = [1, 3, 5].map(start => parseInt(color.slice(start, start + 2), 16) / 255)
      .map(value => value <= .03928 ? value / 12.92 : ((value + .055) / 1.055) ** 2.4)
    return channels[0] * .2126 + channels[1] * .7152 + channels[2] * .0722
  }
  const bg = luminance(background)
  const contrast = (color: string) => (Math.max(bg, luminance(color)) + .05) / (Math.min(bg, luminance(color)) + .05)
  return contrast(text) >= contrast(alternative) ? text : alternative
}
export function resolveModeIndicator(document: ConfigDocument, mode: string) {
  const entry = document.mode_indicator?.modes?.[mode] ?? {}
  return {
    enabled: entry.enabled ?? mode !== 'idle',
    text: entry.text ?? names[mode] ?? mode,
    ui: { position: 'bottom_left', indicator_x_offset: -12, indicator_y_offset: 18, font_size: 11, font_family: '', border_radius: -1, padding_x: -1, padding_y: -1, border_width: 1, ...document.mode_indicator?.ui, ...entry.ui },
  }
}

/** Same right-edge/top-edge geometry as presentation::dynamic; pointer uses canvas percentages. */
export function modeIndicatorPreview(document: ConfigDocument, mode: string, appearance: string, pointer: { x: number; y: number }, canvas = { width: 960, height: 600 }, rendering = { platform: 'windows', scale: 1 }) {
  const badge = resolveModeIndicator(document, mode)
  const ui = badge.ui
  const auto = (value: number, fallback: number) => value < 0 ? fallback : value
  const size = Math.max(1, ui.font_size)
  const px = auto(ui.padding_x, Math.round(size * .4))
  const py = auto(ui.padding_y, Math.round(size * .2))
  const anchorWidth = Math.ceil(Math.max([...badge.text].length * size * .75 + px * 2, size * 2))
  const anchorHeight = Math.ceil(size * 1.4 + py * 2)
  // Keep the native Windows left-edge calculation separate from preview sizing.
  // DPI affects the width subtracted from the right-edge anchor, not the offsets.
  const windows = rendering.platform === 'windows'
  const scale = windows && Number.isFinite(rendering.scale) ? Math.max(1, Math.min(4, rendering.scale)) : 1
  const fontSize = size
  const width = windows ? Math.max([...badge.text].length * fontSize * .7 + px * 2, fontSize * 2) : anchorWidth
  const height = windows ? fontSize + py * 2 : anchorHeight
  const nativeFontSize = Math.round(size * scale)
  const placementWidth = windows ? Math.max([...badge.text].length * nativeFontSize * .7 + Math.round(px * scale) * 2, nativeFontSize * 2) : width
  const right = ui.position.endsWith('right')
  const top = ui.position.startsWith('top')
  const anchorX = Math.max(Math.min(anchorWidth, canvas.width), Math.min(canvas.width, pointer.x / 100 * canvas.width + ui.indicator_x_offset + (right ? anchorWidth : 0)))
  const anchorY = Math.max(0, Math.min(Math.max(0, canvas.height - anchorHeight), pointer.y / 100 * canvas.height + ui.indicator_y_offset - (top ? anchorHeight : 0)))
  const x = windows ? Math.round(anchorX) : anchorX
  const y = windows ? Math.round(anchorY) : anchorY
  const theme = { ...palettes[appearance === 'light' ? 'light' : 'dark'], ...document.theme?.[appearance] }
  const background = ['normal', 'grid', 'ui_hint'].includes(mode) ? theme.accent : mode === 'recursive_grid' ? theme.accent_alt : theme.surface.slice(0, 7) + 'F2'
  const color = (value: any, fallback: string): string => typeof value === 'string' ? value : value?.[appearance] ?? fallback
  return { ...badge, style: {
    position: 'absolute' as const, pointerEvents: 'none' as const, zIndex: 25,
    left: `${x - placementWidth}px`, top: `${y}px`, width: `${width}px`, height: `${height}px`,
    boxSizing: 'border-box' as const, display: 'flex', alignItems: 'center', justifyContent: 'center', whiteSpace: 'nowrap' as const,
    fontSize: `${fontSize}px`, fontFamily: ui.font_family || (windows ? '"Segoe UI", sans-serif' : 'system-ui, sans-serif'), fontWeight: 700,
    borderRadius: `${auto(ui.border_radius, Math.round(size * .35))}px`,
    background: color(ui.background_color, background),
    color: color(ui.text_color, readableOn(background, theme.text, theme.on_accent_alt)),
    boxShadow: `inset 0 0 0 ${Math.max(0, ui.border_width)}px ${color(ui.border_color, theme.accent)}`,
  } }
}
