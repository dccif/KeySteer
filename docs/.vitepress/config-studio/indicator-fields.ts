import type { StyleField } from './fields.ts'

export const indicatorModes = ['normal', 'text_input', 'grid', 'recursive_grid', 'ui_hint', 'window', 'window_quick', 'window_editor', 'window_restore', 'window_tab']
export const indicatorPositions = ['bottom_left', 'bottom_right', 'top_left', 'top_right']
export function indicatorFields(mode?: string): StyleField[] {
  const root = mode ? `mode_indicator.modes.${mode}` : 'mode_indicator'
  return [
    ...(mode ? [
      { path: `${root}.enabled`, label: '显示模式标识符', kind: 'boolean' as const },
      { path: `${root}.text`, label: '标识符文字', kind: 'text' as const },
    ] : []),
    { path: `${root}.ui.position`, label: '标识符位置', kind: 'select', options: indicatorPositions },
    { path: `${root}.ui.indicator_x_offset`, label: '水平偏移（正数向右）', kind: 'number', step: 1 },
    { path: `${root}.ui.indicator_y_offset`, label: '垂直偏移（正数向下）', kind: 'number', step: 1 },
    { path: `${root}.ui.font_size`, label: '字号', kind: 'number', min: 1, step: 1 },
    { path: `${root}.ui.font_family`, label: '字体', kind: 'text' },
    ...['background_color', 'text_color', 'border_color'].map((key, index) => ({ path: `${root}.ui.${key}`, label: ['背景色', '文字色', '边框色'][index], kind: 'color' as const })),
    ...['border_radius', 'padding_x', 'padding_y', 'border_width'].map((key, index) => ({ path: `${root}.ui.${key}`, label: ['圆角', '水平内边距', '垂直内边距', '边框宽度'][index], kind: 'number' as const, min: key === 'border_width' ? 0 : -1, step: 1 })),
  ]
}
