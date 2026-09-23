import { fieldLocation } from './navigation.ts'
export type TargetingMode = 'grid' | 'recursive_grid' | 'ui_hint' | 'key_help' | 'window' | 'window_quick' | 'window_editor' | 'window_restore' | 'window_tab'
export type Appearance = 'dark' | 'light'
export type ControlKind = 'color' | 'number' | 'text' | 'boolean' | 'select' | 'ratios' | 'percentages' | 'offset'

export interface StyleField {
  path: string
  label: string
  kind: ControlKind
  min?: number
  max?: number
  step?: number
  options?: string[]
}

export interface ModeFields {
  colors: StyleField[]
  layout: StyleField[]
  advanced: StyleField[]
}

export const fields = {
  window: {
    colors: [
      { path: 'key_help.background_color', label: '面板背景（共用 key_help）', kind: 'color' },
      { path: 'key_help.text_color', label: '面板文字（共用 key_help）', kind: 'color' },
      { path: 'window.ui.border_color', label: '目标描边', kind: 'color' },
    ],
    layout: [
      { path: 'window.screens', label: '窗口范围（current 当前屏幕 / all 全部屏幕）', kind: 'select', options: ['current', 'all'] },
      { path: 'window.include_minimized', label: '包含最小化窗口', kind: 'boolean' },
      { path: 'window.enabled', label: '启用 Window', kind: 'boolean' },
      { path: 'window.number_timeout_ms', label: '歧义编号等待（毫秒）', kind: 'number', min: 100, max: 2000, step: 10 },
      { path: 'window.move_step', label: '移动步长', kind: 'number', min: 0, max: 10000, step: 1 },
      { path: 'window.resize_step', label: '缩放步长', kind: 'number', min: 0, max: 10000, step: 1 },
      { path: 'window.border_width', label: '目标描边宽度', kind: 'number', min: 0, max: 20, step: .5 },
    ],
    advanced: [
      { path: 'window.move_speed', label: '移动速度', kind: 'number', min: 0, max: 10000, step: 10 },
      { path: 'window.resize_speed', label: '缩放速度', kind: 'number', min: 0, max: 10000, step: 10 },
      { path: 'key_help.font_family', label: '面板字体（共用 key_help）', kind: 'text' },
      { path: 'key_help.font_size', label: '面板字号（共用 key_help）', kind: 'number', min: 1, max: 72, step: 1 },
    ],
  },
  key_help: {
    colors: [
      { path: 'key_help.background_color', label: '背景色', kind: 'color' },
      { path: 'key_help.text_color', label: '文字色', kind: 'color' },
      { path: 'key_help.border_color', label: '边框色', kind: 'color' },
    ],
    layout: [
  { path: 'key_help.mouse_key_help', label: '鼠标模式默认显示提示（? 可切换）', kind: 'boolean' },
  { path: 'key_help.window_key_help', label: '窗口模式默认显示提示（? 可切换）', kind: 'boolean' },
      { path: 'key_help.font_size', label: '字号', kind: 'number', min: 1, max: 72, step: 1 },
      { path: 'key_help.padding_x', label: '水平内边距', kind: 'number', min: 0, max: 100, step: 1 },
      { path: 'key_help.padding_y', label: '垂直内边距', kind: 'number', min: 0, max: 100, step: 1 },
    ],
    advanced: [
      { path: 'key_help.font_family', label: '字体（空值继承）', kind: 'text' },
      { path: 'key_help.border_width', label: '边框宽度', kind: 'number', min: 0, max: 20, step: 1 },
      { path: 'key_help.border_radius', label: '圆角', kind: 'number', min: 0, max: 100, step: 1 },
    ],
  },
  grid: {
    colors: [
      { path: 'grid.ui.background_color', label: '标签底色', kind: 'color' },
      { path: 'grid.ui.text_color', label: '文字色', kind: 'color' },
      { path: 'grid.ui.border_color', label: '标签边框', kind: 'color' },
      { path: 'grid.ui.matched_background_color', label: '选中填充', kind: 'color' },
      { path: 'grid.ui.matched_border_color', label: '网格线', kind: 'color' },
    ],
    layout: [
      { path: 'grid.grid_cols', label: '列数', kind: 'number', min: 1, max: 12, step: 1 },
      { path: 'grid.grid_rows', label: '行数', kind: 'number', min: 1, max: 12, step: 1 },
      { path: 'grid.keys', label: '网格键', kind: 'text' },
      { path: 'grid.ui.font_size', label: '字号', kind: 'number', min: 6, max: 72, step: 1 },
    ],
    advanced: [
      { path: 'grid.max_depth', label: '最大层数', kind: 'number', min: 1, max: 20, step: 1 },
      { path: 'grid.ui.font_family', label: '字体', kind: 'text' },
      { path: 'grid.ui.border_width', label: '线宽', kind: 'number', min: 0.5, max: 8, step: 0.5 },
    ],
  },
  recursive_grid: {
    colors: [
      { path: 'recursive_grid.ui.line_color', label: '网格线', kind: 'color' },
      { path: 'recursive_grid.ui.highlight_color', label: '高亮色', kind: 'color' },
      { path: 'recursive_grid.ui.label_background_color', label: '标签填充', kind: 'color' },
      { path: 'recursive_grid.ui.text_color', label: '文字色', kind: 'color' },
      { path: 'recursive_grid.ui.sub_key_preview_text_color', label: '小字母颜色', kind: 'color' },
    ],
    layout: [
      { path: 'recursive_grid.grid_cols', label: '列数', kind: 'number', min: 1, max: 8, step: 1 },
      { path: 'recursive_grid.grid_rows', label: '行数', kind: 'number', min: 1, max: 8, step: 1 },
      { path: 'recursive_grid.keys', label: '网格键', kind: 'text' },
      { path: 'recursive_grid.ui.font_size', label: '大字母字号（0 自动）', kind: 'number', min: 0, max: 200, step: 1 },
      { path: 'recursive_grid.ui.sub_key_preview_font_size', label: '小字母字号', kind: 'number', min: 4, max: 24, step: 1 },
    ],
    advanced: [
      { path: 'recursive_grid.ui.font_family', label: '字体', kind: 'text' },
      { path: 'recursive_grid.ui.label_min_font_size', label: '最小字号', kind: 'number', min: 1, max: 32, step: 1 },
      { path: 'recursive_grid.ui.line_width', label: '线宽', kind: 'number', min: 0.5, max: 8, step: 0.5 },
      { path: 'recursive_grid.ui.label_background', label: '标签底色', kind: 'boolean' },
      { path: 'recursive_grid.ui.label_char', label: '替代字符', kind: 'text' },
      { path: 'recursive_grid.ui.sub_key_preview', label: '下层预览', kind: 'boolean' },
    ],
  },
  ui_hint: {
    colors: [
      { path: 'ui_hint.ui.background_color', label: '标签底色', kind: 'color' },
      { path: 'ui_hint.ui.text_color', label: '文字色', kind: 'color' },
      { path: 'ui_hint.ui.matched_text_color', label: '匹配文字', kind: 'color' },
      { path: 'ui_hint.ui.border_color', label: '边框色', kind: 'color' },
      { path: 'ui_hint.boundary_highlight.background_color', label: '轮廓填充', kind: 'color' },
      { path: 'ui_hint.boundary_highlight.border_color', label: '轮廓颜色', kind: 'color' },
    ],
    layout: [
      { path: 'ui_hint.hint_characters', label: '提示键', kind: 'text' },
      { path: 'ui_hint.placement', label: '标签位置', kind: 'select', options: ['top', 'center', 'bottom'] },
      { path: 'ui_hint.ui.font_size', label: '字号', kind: 'number', min: 6, max: 72, step: 1 },
      { path: 'ui_hint.ui.border_width', label: '边框', kind: 'number', min: 0, max: 8, step: 0.5 },
    ],
    advanced: [
      { path: 'ui_hint.label_x_offset', label: '水平偏移', kind: 'number', min: -100, max: 100, step: 1 },
      { path: 'ui_hint.label_y_offset', label: '垂直偏移', kind: 'number', min: -100, max: 100, step: 1 },
      { path: 'ui_hint.ui.font_family', label: '字体', kind: 'text' },
      { path: 'ui_hint.ui.border_radius', label: '圆角', kind: 'number', min: -1, max: 32, step: 1 },
      { path: 'ui_hint.ui.padding_x', label: '水平内边距', kind: 'number', min: -1, max: 32, step: 1 },
      { path: 'ui_hint.ui.padding_y', label: '垂直内边距', kind: 'number', min: -1, max: 32, step: 1 },
      { path: 'ui_hint.boundary_highlight.enabled', label: '元素轮廓', kind: 'boolean' },
      { path: 'ui_hint.boundary_highlight.border_width', label: '轮廓线宽', kind: 'number', min: 0, max: 8, step: 0.5 },
    ],
  },
} as Record<TargetingMode, ModeFields>

fields.window.layout.push({ path: 'window.card.position_mode', label: '卡片定位（window 窗口 / screen 当前屏幕）', kind: 'select', options: ['window', 'screen'] })
fields.window.layout.push({ path: 'window.card.position', label: '上、右、下、左（四个百分比，逗号分隔）', kind: 'percentages' })
fields.window.colors.push({ path: 'window.card.border_color', label: '卡片边框颜色', kind: 'color' })
fields.window.colors.push({ path: 'window.card.guide_line_color', label: '引导线颜色（含透明度）', kind: 'color' })
fields.window.layout.push({ path: 'window.card.guide_line_enabled', label: '显示引导线', kind: 'boolean' })
fields.window.layout.push({ path: 'window.card.guide_line_width', label: '引导线宽度', kind: 'number', min: 0, max: 32, step: 0.5 })
fields.window.colors.push({ path: 'window.card.background_color', label: '卡片背景', kind: 'color' })
fields.window.colors.push({ path: 'window.card.number_color', label: '编号文字', kind: 'color' })
fields.window.colors.push({ path: 'window.card.app_color', label: '程序名颜色', kind: 'color' })
fields.window.colors.push({ path: 'window.card.title_color', label: '标题颜色', kind: 'color' })
fields.window.advanced.push({ path: 'window.ui.font_size', label: '编号字号', kind: 'number', min: 1, max: 256 })
fields.window.advanced.push({ path: 'window.ui.font_family', label: '编号字体', kind: 'text' })
fields.window.advanced.push({ path: 'window.ui.border_radius', label: '卡片圆角（-1 自动）', kind: 'number', min: -1, max: 256 })
fields.window.advanced.push({ path: 'window.ui.border_width', label: '卡片边框宽度', kind: 'number', min: 0, max: 20 })
fields.window.advanced.push({ path: 'window.card.app_font_size', label: '程序名字号（0 自动）', kind: 'number', min: 0, max: 256 })
fields.window.advanced.push({ path: 'window.card.title_font_size', label: '标题字号（0 自动）', kind: 'number', min: 0, max: 256 })
fields.window.advanced.push({ path: 'window.card.app_font_family', label: '程序名字体（空值继承）', kind: 'text' })
fields.window.advanced.push({ path: 'window.card.title_font_family', label: '标题字体（空值继承）', kind: 'text' })
fields.window.advanced.push({ path: 'window.card.app_bold', label: '程序名加粗', kind: 'boolean' })
fields.window.advanced.push({ path: 'window.card.title_bold', label: '标题加粗', kind: 'boolean' })
fields.window.advanced.push({ path: 'window.card.text_width', label: '每列文字宽度', kind: 'number', min: 1, max: 4096 })
fields.window.advanced.push({ path: 'window.card.padding_x', label: '文字水平内边距', kind: 'number', min: 0, max: 256 })
fields.window.advanced.push({ path: 'window.card.padding_y', label: '文字垂直内边距', kind: 'number', min: 0, max: 256 })
fields.window.advanced.push({ path: 'window.card.line_height', label: '文字行高倍数', kind: 'number', min: 1, max: 4, step: 0.1 })
fields.window.advanced.push({ path: 'window.card.min_height', label: '卡片最小高度', kind: 'number', min: 0, max: 4096 })
fields.window.advanced.push({ path: 'window.card.number_min_width', label: '编号最小宽度', kind: 'number', min: 0, max: 4096 })
fields.window.advanced.push({ path: 'window.ui.padding_x', label: '编号水平内边距（-1 自动）', kind: 'number', min: -1, max: 256 })
fields.window.advanced.push({ path: 'window.ui.padding_y', label: '编号垂直内边距（-1 自动）', kind: 'number', min: -1, max: 256 })

for (const mode of ['window_quick', 'window_editor', 'window_restore', 'window_tab'] as const) {
  const common = (items: StyleField[]) => items.filter(f => !/window\.(move_|resize_)/.test(f.path) && (mode === 'window_editor' || !f.path.startsWith('window.card.position'))).map(f => ({ ...f, path: f.path.replace(/^window\.(?!card\.(?!position))/, `${mode}.`) }))
  ;(fields as Record<string, ModeFields>)[mode] = { colors: common(fields.window.colors), layout: common(fields.window.layout), advanced: common(fields.window.advanced) }
  const own = (fields as Record<string, ModeFields>)[mode]
  if (mode === 'window_quick') own.layout.push({ path: `${mode}.split_ratios`, label: '比例（逗号分隔，支持分数）', kind: 'ratios' })
  if (mode !== 'window_tab') own.layout.push({ path: `${mode}.gap`, label: '布局间距', kind: 'number', min: 0, step: 1 })
  if (mode === 'window_editor') own.advanced.push({ path: `${mode}.resize_step`, label: '分割线步长', kind: 'number', min: 0 }, { path: `${mode}.resize_speed`, label: '分割线速度', kind: 'number', min: 0 })
  if (mode === 'window_restore') own.advanced.push({ path: `${mode}.lifecycle.after_finish`, label: '恢复成功后', kind: 'select', options: ['window_editor', 'window', 'window_restore', 'keep', 'normal', 'idle'] })
}

export const paletteFields = ['surface', 'accent', 'accent_alt', 'on_accent_alt', 'text'] as const
export const paletteLabels: Record<(typeof paletteFields)[number], string> = {
  surface: '表面',
  accent: '主色',
  accent_alt: '高亮',
  on_accent_alt: '高亮文字',
  text: '文字',
}

export const styleSearchFields = [...Object.values(fields).flatMap(group => [...group.colors, ...group.layout, ...group.advanced]), ...(['light', 'dark'] as const).flatMap(appearance => paletteFields.map(field => ({path: `theme.${appearance}.${field}`, label: paletteLabels[field]})))].map(field => ({...field, ...fieldLocation(field.path)}))


type FieldKind = 'number' | 'boolean' | 'select' | 'text'

export interface ConfigField {
  path: string
  label: string
  description: string
  kind: FieldKind
  min?: number
  max?: number
  step?: number
  options?: Array<{ value: string; label: string }>
}

export const frequentFields: ConfigField[] = [
  { path: 'pointer.initial_speed', label: '起始速度', description: '方向键刚按下时的像素/秒', kind: 'number', min: 0, max: 10000, step: 50 },
  { path: 'pointer.max_speed', label: '最高速度', description: '持续移动达到的像素/秒', kind: 'number', min: 0, max: 20000, step: 50 },
  { path: 'pointer.acceleration', label: '加速度', description: '每秒增加的速度；0 表示保持初速', kind: 'number', min: 0, max: 30000, step: 100 },
  { path: 'pointer.smooth_acceleration', label: '平滑加速', description: '开启 smootherstep S 曲线；关闭为线性', kind: 'boolean' },
  { path: 'normal.passthrough_unbound_keys', label: '未绑定键透传', description: '仅接管完整命中的 KeySteer 绑定；关闭后 Normal 键盘独占', kind: 'boolean' },
  { path: 'normal.long_press_toggle_ms', label: '长按切换', description: '点击键长按多少毫秒后切换持续按下；0 为关闭', kind: 'number', min: 0, max: 5000, step: 50 },
  { path: 'normal.auto_release_ms', label: '停止拖动后释放', description: '长按点击键并按住物理修饰键拖动；停止多少毫秒后释放；0 为关闭', kind: 'number', min: 0, max: 60000, step: 50 },
  { path: 'grid.max_depth', label: 'Grid 层数', description: '确认目标前需要输入的网格层数', kind: 'number', min: 1, max: 20, step: 1 },
  { path: 'recursive_grid.max_depth', label: '递归上限', description: 'Recursive Grid 最大递归次数', kind: 'number', min: 1, max: 20, step: 1 },
  { path: 'ui_hint.scan_scope', label: '扫描范围', description: '鼠标下窗口或鼠标所在的整块屏幕；跨屏自动重扫', kind: 'select', options: [
    { value: 'window', label: '鼠标下窗口（默认）' },
    { value: 'screen', label: '当前整屏' },
  ] },
  { path: 'ui_hint.strategy', label: 'UI 扫描', description: '视觉、辅助功能树，或两者并行合并', kind: 'select', options: [
    { value: 'vision', label: 'Vision' },
    { value: 'hybrid', label: 'Hybrid（默认）' },
    { value: 'axtree', label: 'Accessibility Tree' },
  ] },
]

export const advancedFields: ConfigField[] = [
  { path: 'pointer.tap_distance', label: '短按距离', description: '极短方向键操作仍移动的像素', kind: 'number', min: 0, max: 50, step: 0.1 },
  { path: 'pointer.precision_multiplier', label: 'Precision 倍率', description: 'precision 修饰键的速度倍率', kind: 'number', min: 0.01, max: 2, step: 0.01 },
  { path: 'pointer.slow_multiplier', label: 'Slow 倍率', description: 'slow 修饰键的速度倍率', kind: 'number', min: 0.01, max: 4, step: 0.05 },
  { path: 'pointer.fast_multiplier', label: 'Fast 倍率', description: 'fast 修饰键的速度倍率', kind: 'number', min: 0.1, max: 10, step: 0.1 },
  { path: 'scroll.scroll_step', label: '滚动步长', description: '普通滚动一次的像素距离', kind: 'number', min: 1, max: 5000, step: 10 },
  { path: 'scroll.scroll_step_half', label: '半页滚动', description: 'scroll_half_* 的像素距离', kind: 'number', min: 1, max: 50000, step: 50 },
  { path: 'grid.cursor_follow_selection', label: 'Grid 光标跟随', description: '每次选中后移到当前单元格中心', kind: 'boolean' },
  { path: 'recursive_grid.cursor_follow_selection', label: '递归光标跟随', description: '每次选中后移到当前单元格中心', kind: 'boolean' },
  { path: 'ui_hint.hint_characters', label: '提示字符', description: '用于生成 UI Hint 标签的字符集合', kind: 'text' },
  { path: 'ui_hint.placement', label: '标签位置', description: '标签相对目标元素的位置', kind: 'select', options: [
    { value: 'top', label: '上方' },
    { value: 'center', label: '居中' },
    { value: 'bottom', label: '下方' },
  ] },
]

export const commonSearchFields = [...frequentFields, ...advancedFields].map(field => ({ ...field, ...fieldLocation(field.path) }))

