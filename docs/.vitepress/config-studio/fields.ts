import { fieldLocation } from './navigation.ts'
export type TargetingMode = 'grid' | 'recursive_grid' | 'ui_hint' | 'key_help' | 'window' | 'window_quick' | 'window_editor' | 'window_restore' | 'window_tab'
export type Appearance = 'dark' | 'light'
export type ControlKind = 'color' | 'number' | 'text' | 'boolean' | 'select' | 'ratios' | 'percentages' | 'offset' | 'choices' | 'binding' | 'chords' | 'field-modes'

export interface StyleField {
  path: string
  label: string
  kind: ControlKind
  min?: number
  max?: number
  step?: number
  options?: string[]
  requireAll?: boolean
  default?: unknown
  action?: string
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
      { path: 'window.target', label: '入口优先目标（active 激活窗口 / mouse 鼠标下窗口；无目标时互相兜底）', kind: 'select', options: ['', 'active', 'mouse'] },
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

fields.ui_hint.advanced.push(
  { path: 'ui_hint.search_copy_keys', label: '四个条目的复制键（按顺序，逗号分隔）', kind: 'chords', default: ['ctrl+1', 'ctrl+2', 'ctrl+3', 'ctrl+4'] },
  { path: 'ui_hint.search_bindings', label: '点位调整切换键（单击）', kind: 'binding', action: 'point_toggle', default: 'ctrl' },
  { path: 'ui_hint.search_bindings', label: '搜索结果／多点位置切换键', kind: 'binding', action: 'point_next', default: 'tab' },
  { path: 'ui_hint.search_match_priority', label: '同匹配程度时的优先级（pinyin 简拼 / text 文字／辅助信息／类型 / label 标签）', kind: 'choices', options: ['pinyin', 'text', 'label'], requireAll: true, default: ['pinyin', 'text', 'label'] },
  { path: 'ui_hint.search_point.field_modes', label: '多点信息栏展示方式（1–4）', kind: 'field-modes', default: ['concat', 'concat', 'switch', 'switch'] },
  { path: 'ui_hint.search_bindings', label: '颜色格式切换键（自动进入调整）', kind: 'binding', action: 'color_next', default: 'ctrl+shift+4' },
  { path: 'ui_hint.search_point.color_formats', label: '颜色格式顺序（首项为默认）', kind: 'choices', options: ['hex', 'rgb', 'hsl'], default: ['hex', 'rgb', 'hsl'] },
)
fields.ui_hint.colors.push(
  { path: 'ui_hint.search_point.marker_color', label: '点位标记颜色', kind: 'color' },
  { path: 'ui_hint.search_point.input_background_color', label: 'Point 输入框背景色', kind: 'color' },
  { path: 'ui_hint.search_point.input_border_color', label: 'Point 输入框边框色（默认沿用搜索框）', kind: 'color' },
)
fields.ui_hint.layout.push(
  { path: 'ui_hint.search_point.color_preview.enabled', label: '显示颜色预览色块', kind: 'boolean', default: true },
  { path: 'ui_hint.search_point.color_preview.width', label: '色块宽度', kind: 'number', min: 1, max: 64, step: 1, default: 16 },
  { path: 'ui_hint.search_point.color_preview.height', label: '色块高度', kind: 'number', min: 1, max: 64, step: 1, default: 16 },
  { path: 'ui_hint.search_point.color_preview.x_offset', label: '色块水平偏移', kind: 'number', min: -200, max: 200, step: 1, default: 4 },
  { path: 'ui_hint.search_point.color_preview.y_offset', label: '色块垂直偏移', kind: 'number', min: -200, max: 200, step: 1, default: 0 },
  { path: 'ui_hint.search_point.color_preview.border_width', label: '色块边框线宽', kind: 'number', min: 0, max: 10, step: 1, default: 1 },
  { path: 'ui_hint.search_point.marker_radius', label: '点位标记半径', kind: 'number', min: 2, max: 40, step: 1, default: 6 },
  { path: 'ui_hint.search_point.marker_width', label: '点位标记线宽', kind: 'number', min: 0, max: 10, step: 1, default: 2 },
)

for (const [action, label, keys] of [
  ['paste', '搜索粘贴快捷键', 'primary+v'], ['copy', '搜索复制选区快捷键', 'primary+c'], ['cut', '搜索剪切快捷键', 'primary+x'],
  ['select_all', '搜索全选快捷键', 'primary+a'], ['accept', '结束搜索快捷键（多个用空格分隔）', 'enter / primary+q'], ['cancel', '取消搜索快捷键', 'esc'],
  ['left', '搜索光标左移', 'left'], ['right', '搜索光标右移', 'right'], ['home', '搜索光标开头', 'home'], ['end', '搜索光标末尾', 'end'],
  ['select_left', '搜索向左选择', 'shift+left'], ['select_right', '搜索向右选择', 'shift+right'], ['select_home', '搜索选择到开头', 'shift+home'], ['select_end', '搜索选择到末尾', 'shift+end'],
  ['backspace', '搜索退格', 'backspace'], ['delete', '搜索删除', 'delete'],
] as const) fields.ui_hint.advanced.push({ path: 'ui_hint.search_edit_keys', label, kind: 'binding', action, default: keys })

fields.window.layout.push({ path: 'window.card.position_mode', label: '卡片定位（window 窗口 / screen 当前屏幕）', kind: 'select', options: ['window', 'screen'] })
for (const [block, title] of [['search_input_ui', '搜索框'], ['search_info_ui', '搜索信息']] as const) {
  const prefix = `ui_hint.${block}`
  fields.ui_hint.advanced.push(
    { path: `${prefix}.position_mode`, label: `${title}定位`, kind: 'select', options: block === 'search_input_ui' ? ['screen', 'window'] : ['search_input', 'screen', 'window'] },
    { path: `${prefix}.position`, label: `${title}四边百分比`, kind: 'percentages' },
    { path: `${prefix}.font_family`, label: `${title}字体`, kind: 'text' },
  )
  for (const [key, label, min, max] of [
    ['width', '宽度', 80, 2000], ['font_size', '字号', 6, 72],
    ['border_radius', '圆角', -1, 80], ['border_width', '边框', 0, 12],
    ['padding_x', '水平内边距', -1, 100], ['padding_y', '垂直内边距', -1, 100],
    ['x_offset', '水平偏移', -2000, 2000], ['y_offset', '垂直偏移', -2000, 2000],
  ] as const) fields.ui_hint.advanced.push({ path: `${prefix}.${key}`, label: `${title}${label}`, kind: 'number', min, max, step: 1 })
  for (const [key, label] of [['background_color', '底色'], ['text_color', '文字色'], ['border_color', '边框色']] as const) {
    fields.ui_hint.colors.push({ path: `${prefix}.${key}`, label: `${title}${label}`, kind: 'color' })
  }
}
fields.window.layout.push({ path: 'window.card.position', label: '上、右、下、左（四个百分比，逗号分隔）', kind: 'percentages' })
fields.window.colors.push({ path: 'window.card.border_color', label: '卡片边框颜色', kind: 'color' })
fields.window.colors.push({ path: 'window.card.guide_line_color', label: '引导线颜色（含透明度）', kind: 'color' })
fields.window.layout.push({ path: 'window.card.guide_line_enabled', label: '显示引导线', kind: 'boolean' })
fields.window.layout.push({ path: 'window.card.guide_line_width', label: '引导线宽度', kind: 'number', min: 0, max: 32, step: 0.5 })
fields.window.colors.push({ path: 'window.card.background_color', label: '卡片背景', kind: 'color' })
fields.window.colors.push({ path: 'window.card.selected_background_color', label: '选中卡片背景', kind: 'color' })
fields.window.colors.push({ path: 'window.card.selected_border_color', label: '选中卡片边框颜色', kind: 'color' })
fields.window.layout.push({ path: 'window.card.selected_border_width', label: '选中卡片边框宽度', kind: 'number', min: 0, max: 20, step: 0.5 })
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
}

export const lifecycleModes = ['grid', 'recursive_grid', 'ui_hint', 'window', 'window_quick', 'window_editor', 'window_restore', 'window_tab'] as const
export const lifecycleClickActions = ['left_click', 'right_click', 'middle_click', 'double_click']
export function lifecycleOptions(event: 'after_finish' | 'after_click', plugins: string[] = []): string[] {
  return ['keep', 'restart', 'return', ...(event === 'after_finish' ? lifecycleClickActions : ['finish']),
    'idle', 'normal', 'text_input', ...lifecycleModes, ...plugins]
}
for (const mode of lifecycleModes) {
  fields[mode].advanced.push(
    { path: `${mode}.lifecycle.after_finish`, label: '完成后动作', kind: 'select', options: lifecycleOptions('after_finish') },
    { path: `${mode}.lifecycle.after_click`, label: '点击后动作', kind: 'select', options: lifecycleOptions('after_click') },
  )
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
  { path: 'scroll.speed', label: '长按滚动速度（像素/秒）', description: '独立于短按距离；0 表示仅短按，速度修饰键继续生效', kind: 'number', min: 0, step: 50 },
  { path: 'pointer.smooth_acceleration', label: '平滑加速', description: '开启 smootherstep S 曲线；关闭为线性', kind: 'boolean' },
  { path: 'normal.passthrough_unbound_keys', label: '未绑定键透传', description: '仅接管完整命中的 KeySteer 绑定；关闭后 Normal 键盘独占', kind: 'boolean' },
  { path: 'normal.long_press_toggle_ms', label: '长按切换', description: '点击键长按多少毫秒后切换持续按下；0 为关闭', kind: 'number', min: 0, max: 5000, step: 50 },
  { path: 'normal.auto_release_ms', label: '停止拖动后释放', description: '长按点击键并按住物理修饰键拖动；停止多少毫秒后释放；0 为关闭', kind: 'number', min: 0, max: 60000, step: 50 },
  { path: 'grid.max_depth', label: 'Grid 层数', description: '确认目标前需要输入的网格层数', kind: 'number', min: 1, max: 20, step: 1 },
  { path: 'recursive_grid.max_depth', label: '递归上限', description: 'Recursive Grid 最大递归次数', kind: 'number', min: 1, max: 20, step: 1 },
  { path: 'ui_hint.scan_scope', label: '扫描范围', description: '窗口不可用时按优先级选择范围，只扫描选定范围；鼠标窗口优先时尝试激活', kind: 'select', options: [
    { value: 'window', label: '鼠标下窗口（默认）' },
    { value: 'active', label: '当前激活窗口优先' },
    { value: 'screen', label: '当前整屏' },
  ] },
  { path: 'ui_hint.strategy', label: 'UI 扫描', description: '辅助功能、OCR、轮廓检测，或全部并行合并', kind: 'select', options: [
    { value: 'vision', label: 'Vision' },
    { value: 'contour', label: 'Contour' },
    { value: 'hybrid', label: 'Hybrid（默认）' },
    { value: 'axtree', label: 'Accessibility Tree' },
  ] },
  { path: 'platform.macos.scroll.invert_horizontal', label: 'macOS 横向滚动反转', description: '仅 macOS 生效；反转水平滚轮方向，默认关闭', kind: 'boolean' },
  { path: 'platform.macos.scroll.invert_vertical', label: 'macOS 纵向滚动反转', description: '仅 macOS 生效；反转垂直滚轮方向，默认开启', kind: 'boolean' },
]

export const advancedFields: ConfigField[] = [
  { path: 'pointer.tap_distance', label: '短按距离', description: '极短方向键操作仍移动的像素', kind: 'number', min: 0, max: 50, step: 0.1 },
  { path: 'pointer.precision_multiplier', label: 'Precision 倍率', description: 'precision 修饰键的速度倍率', kind: 'number', min: 0.01, max: 2, step: 0.01 },
  { path: 'pointer.slow_multiplier', label: 'Slow 倍率', description: 'slow 修饰键的速度倍率', kind: 'number', min: 0.01, max: 4, step: 0.05 },
  { path: 'pointer.fast_multiplier', label: 'Fast 倍率', description: 'fast 修饰键的速度倍率', kind: 'number', min: 0.1, max: 10, step: 0.1 },
  { path: 'scroll.scroll_step', label: '滚动步长（像素）', description: '普通滚动短按一次的距离，不影响长按速度', kind: 'number', min: 1, max: 5000, step: 10 },
  { path: 'scroll.scroll_step_half', label: '半页滚动', description: 'scroll_half_* 的像素距离', kind: 'number', min: 1, max: 50000, step: 50 },
  { path: 'scroll.scroll_step_full', label: '整页滚动', description: 'scroll_full_* 的像素距离', kind: 'number', min: 1, max: 2147483647, step: 1000 },
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
