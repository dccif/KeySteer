import { computed, defineComponent } from 'vue'
import { cloneConfigDocument } from './document'

type ConfigDocument = Record<string, any>
type TargetingMode = 'grid' | 'recursive_grid' | 'ui_hint'
type ControlKind = 'color' | 'number' | 'text' | 'boolean' | 'select'

interface StyleField {
  path: string
  label: string
  kind: ControlKind
  min?: number
  max?: number
  step?: number
  options?: string[]
}

const fields: Record<TargetingMode, StyleField[]> = {
  grid: [
    { path: 'grid.grid_cols', label: '列数', kind: 'number', min: 1, max: 12, step: 1 },
    { path: 'grid.grid_rows', label: '行数', kind: 'number', min: 1, max: 12, step: 1 },
    { path: 'grid.keys', label: '网格键', kind: 'text' },
    { path: 'grid.max_depth', label: '最大层数', kind: 'number', min: 1, max: 20, step: 1 },
    { path: 'grid.ui.font_size', label: '字号', kind: 'number', min: 6, max: 72, step: 1 },
    { path: 'grid.ui.font_family', label: '字体', kind: 'text' },
    { path: 'grid.ui.border_width', label: '线宽', kind: 'number', min: 0.5, max: 8, step: 0.5 },
    { path: 'grid.ui.background_color', label: '标签底色', kind: 'color' },
    { path: 'grid.ui.text_color', label: '文字色', kind: 'color' },
    { path: 'grid.ui.border_color', label: '标签边框', kind: 'color' },
    { path: 'grid.ui.matched_background_color', label: '选中填充', kind: 'color' },
    { path: 'grid.ui.matched_border_color', label: '网格线', kind: 'color' },
  ],
  recursive_grid: [
    { path: 'recursive_grid.grid_cols', label: '列数', kind: 'number', min: 1, max: 8, step: 1 },
    { path: 'recursive_grid.grid_rows', label: '行数', kind: 'number', min: 1, max: 8, step: 1 },
    { path: 'recursive_grid.keys', label: '网格键', kind: 'text' },
    { path: 'recursive_grid.ui.font_size', label: '字号', kind: 'number', min: 6, max: 72, step: 1 },
    { path: 'recursive_grid.ui.font_family', label: '字体', kind: 'text' },
    { path: 'recursive_grid.ui.label_min_font_size', label: '最小字号', kind: 'number', min: 1, max: 32, step: 1 },
    { path: 'recursive_grid.ui.line_width', label: '线宽', kind: 'number', min: 0.5, max: 8, step: 0.5 },
    { path: 'recursive_grid.ui.line_color', label: '网格线', kind: 'color' },
    { path: 'recursive_grid.ui.highlight_color', label: '高亮色', kind: 'color' },
    { path: 'recursive_grid.ui.label_background', label: '标签底色', kind: 'boolean' },
    { path: 'recursive_grid.ui.label_background_color', label: '标签填充', kind: 'color' },
    { path: 'recursive_grid.ui.text_color', label: '文字色', kind: 'color' },
    { path: 'recursive_grid.ui.label_char', label: '替代字符', kind: 'text' },
    { path: 'recursive_grid.ui.sub_key_preview', label: '下层预览', kind: 'boolean' },
    { path: 'recursive_grid.ui.sub_key_preview_font_size', label: '预览字号', kind: 'number', min: 4, max: 24, step: 1 },
    { path: 'recursive_grid.ui.sub_key_preview_text_color', label: '预览文字', kind: 'color' },
  ],
  ui_hint: [
    { path: 'ui_hint.hint_characters', label: '提示键', kind: 'text' },
    { path: 'ui_hint.placement', label: '标签位置', kind: 'select', options: ['top', 'center', 'bottom'] },
    { path: 'ui_hint.label_x_offset', label: '水平偏移', kind: 'number', min: -100, max: 100, step: 1 },
    { path: 'ui_hint.label_y_offset', label: '垂直偏移', kind: 'number', min: -100, max: 100, step: 1 },
    { path: 'ui_hint.ui.font_size', label: '字号', kind: 'number', min: 6, max: 72, step: 1 },
    { path: 'ui_hint.ui.font_family', label: '字体', kind: 'text' },
    { path: 'ui_hint.ui.border_width', label: '边框', kind: 'number', min: 0, max: 8, step: 0.5 },
    { path: 'ui_hint.ui.border_radius', label: '圆角', kind: 'number', min: -1, max: 32, step: 1 },
    { path: 'ui_hint.ui.padding_x', label: '水平内边距', kind: 'number', min: -1, max: 32, step: 1 },
    { path: 'ui_hint.ui.padding_y', label: '垂直内边距', kind: 'number', min: -1, max: 32, step: 1 },
    { path: 'ui_hint.ui.background_color', label: '标签底色', kind: 'color' },
    { path: 'ui_hint.ui.text_color', label: '文字色', kind: 'color' },
    { path: 'ui_hint.ui.matched_text_color', label: '匹配文字', kind: 'color' },
    { path: 'ui_hint.ui.border_color', label: '边框色', kind: 'color' },
    { path: 'ui_hint.boundary_highlight.enabled', label: '元素轮廓', kind: 'boolean' },
    { path: 'ui_hint.boundary_highlight.border_width', label: '轮廓线宽', kind: 'number', min: 0, max: 8, step: 0.5 },
    { path: 'ui_hint.boundary_highlight.background_color', label: '轮廓填充', kind: 'color' },
    { path: 'ui_hint.boundary_highlight.border_color', label: '轮廓颜色', kind: 'color' },
  ],
}

const themeFields: StyleField[] = [
  { path: 'theme.dark.surface', label: '表面', kind: 'color' },
  { path: 'theme.dark.accent', label: '主色', kind: 'color' },
  { path: 'theme.dark.accent_alt', label: '高亮', kind: 'color' },
  { path: 'theme.dark.text', label: '文字', kind: 'color' },
]

export default defineComponent({
  name: 'ModeStyleControls',
  props: {
    document: { type: Object as () => ConfigDocument, required: true },
    mode: { type: String as () => TargetingMode, required: true },
  },
  emits: {
    change: (_document: ConfigDocument) => true,
  },
  setup(props, { emit }) {
    const modeFields = computed(() => fields[props.mode])

    function update(path: string, value: unknown): void {
      const next = cloneConfigDocument(props.document)
      setPath(next, path, value)
      emit('change', next)
    }

    return () => (
      <div class="ks-style-controls">
        <div class="ks-style-heading">
          <div><strong>{modeLabel(props.mode)} 样式</strong><span>修改后立即显示在上方预览中</span></div>
        </div>
        <div class="ks-style-section">
          <span class="ks-style-section-label">主题</span>
          <div class="ks-style-fields">
            {themeFields.map((field) => <StyleControl field={field} value={getPath(props.document, field.path)} onUpdate={(value) => update(field.path, value)} />)}
          </div>
        </div>
        <div class="ks-style-section">
          <span class="ks-style-section-label">当前模式</span>
          <div class="ks-style-fields">
            {modeFields.value.map((field) => <StyleControl field={field} value={getPath(props.document, field.path)} onUpdate={(value) => update(field.path, value)} />)}
          </div>
        </div>
      </div>
    )
  },
})

const StyleControl = defineComponent({
  props: {
    field: { type: Object as () => StyleField, required: true },
    value: { required: false },
    onUpdate: { type: Function as unknown as () => (value: unknown) => void, required: true },
  },
  setup(props) {
    return () => {
      const field = props.field
      const value = props.value ?? fallback(field)
      return (
        <label class={{ 'ks-style-control': true, toggle: field.kind === 'boolean' }} title={field.path}>
          <span>{field.label}</span>
          {field.kind === 'boolean' ? (
            <button type="button" class={{ active: Boolean(value) }} onClick={() => props.onUpdate(!value)}><i />{value ? '开启' : '关闭'}</button>
          ) : field.kind === 'color' ? (
            <div class="ks-style-color">
              <input type="color" value={normalizeColor(value)} onInput={(event) => props.onUpdate(withAlpha((event.target as HTMLInputElement).value, value))} />
              <input value={String(value)} onInput={(event) => props.onUpdate((event.target as HTMLInputElement).value)} />
            </div>
          ) : field.kind === 'select' ? (
            <select value={String(value)} onChange={(event) => props.onUpdate((event.target as HTMLSelectElement).value)}>
              {field.options?.map((option) => <option value={option}>{option}</option>)}
            </select>
          ) : (
            <input
              type={field.kind === 'number' ? 'number' : 'text'}
              value={String(value)}
              min={field.min}
              max={field.max}
              step={field.step}
              onInput={(event) => props.onUpdate(field.kind === 'number' ? Number((event.target as HTMLInputElement).value) : (event.target as HTMLInputElement).value)}
            />
          )}
        </label>
      )
    }
  },
})

function getPath(document: ConfigDocument, path: string): unknown {
  return path.split('.').reduce<unknown>((value, part) => value && typeof value === 'object' ? (value as ConfigDocument)[part] : undefined, document)
}

function setPath(document: ConfigDocument, path: string, value: unknown): void {
  const parts = path.split('.')
  let target = document
  for (const part of parts.slice(0, -1)) target = target[part] ??= {}
  target[parts.at(-1)!] = value
}

function fallback(field: StyleField): unknown {
  if (field.kind === 'boolean') return false
  if (field.kind === 'number') return field.min === -1 ? -1 : field.min ?? 0
  if (field.kind === 'color') return '#6E82D6FF'
  return field.options?.[0] ?? ''
}

function normalizeColor(value: unknown): string {
  const source = String(value ?? '#6E82D6')
  return /^#[0-9a-f]{6}/i.test(source) ? source.slice(0, 7) : '#6E82D6'
}

function withAlpha(next: string, previous: unknown): string {
  const source = String(previous ?? '')
  return `${next.toUpperCase()}${/^#[0-9a-f]{8}$/i.test(source) ? source.slice(7, 9).toUpperCase() : 'FF'}`
}

function modeLabel(mode: TargetingMode): string {
  return mode === 'grid' ? 'Grid' : mode === 'recursive_grid' ? 'Recursive Grid' : 'UI Hint'
}
