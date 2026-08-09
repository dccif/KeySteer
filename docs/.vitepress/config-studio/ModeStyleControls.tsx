import { computed, defineComponent } from 'vue'
import {
  cloneConfigDocument,
  deleteConfigPath,
  getConfigPath,
  setConfigPath,
  type ConfigDocument,
} from './document'

type TargetingMode = 'grid' | 'recursive_grid' | 'ui_hint'
type Appearance = 'dark' | 'light'
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

interface ModeFields {
  colors: StyleField[]
  layout: StyleField[]
  advanced: StyleField[]
}

const fields: Record<TargetingMode, ModeFields> = {
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
      { path: 'recursive_grid.ui.font_size', label: '大字母字号', kind: 'number', min: 6, max: 72, step: 1 },
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
}

const paletteFields = ['surface', 'accent', 'accent_alt', 'on_accent_alt', 'text'] as const
const paletteLabels: Record<(typeof paletteFields)[number], string> = {
  surface: '表面',
  accent: '主色',
  accent_alt: '高亮',
  on_accent_alt: '高亮文字',
  text: '文字',
}

export default defineComponent({
  name: 'ModeStyleControls',
  props: {
    document: { type: Object as () => ConfigDocument, required: true },
    effectiveDocument: { type: Object as () => ConfigDocument, required: true },
    mode: { type: String as () => TargetingMode, required: true },
    appearance: { type: String as () => Appearance, required: true },
  },
  emits: {
    change: (_document: ConfigDocument) => true,
    appearanceChange: (_appearance: Appearance) => true,
  },
  setup(props, { emit }) {
    const modeFields = computed(() => fields[props.mode])

    function update(path: string, value: unknown): void {
      const next = cloneConfigDocument(props.document)
      setConfigPath(next, path, value)
      emit('change', next)
    }

    function reset(path: string): void {
      const next = cloneConfigDocument(props.document)
      deleteConfigPath(next, path)
      emit('change', next)
    }

    const renderFields = (items: StyleField[]) => (
      <div class="ks-style-fields">
        {items.map((field) => (
          <StyleControl
            field={field}
            value={getConfigPath(props.effectiveDocument, field.path)}
            appearance={props.appearance}
            inherited={getConfigPath(props.document, field.path) === undefined}
            onUpdate={(value) => update(field.path, value)}
            onReset={() => reset(field.path)}
          />
        ))}
      </div>
    )

    return () => {
      const palette = paletteFields.map<StyleField>((field) => ({
        path: `theme.${props.appearance}.${field}`,
        label: paletteLabels[field],
        kind: 'color',
      }))
      return (
        <div class="ks-style-controls">
          <div class="ks-style-heading">
            <div><strong>颜色与 {modeLabel(props.mode)} 样式</strong><span>配置值会立即写入 TOML 并显示在上方预览中</span></div>
            <div class="ks-appearance-switch" aria-label="预览配色">
              {(['dark', 'light'] as Appearance[]).map((appearance) => (
                <button class={{ active: props.appearance === appearance }} onClick={() => emit('appearanceChange', appearance)}>
                  {appearance === 'dark' ? '深色' : '浅色'}
                </button>
              ))}
            </div>
          </div>
          <div class="ks-style-section">
            <span class="ks-style-section-label">{props.appearance === 'dark' ? '深色主题' : '浅色主题'}</span>
            {renderFields(palette)}
          </div>
          <div class="ks-style-section">
            <span class="ks-style-section-label">模式颜色</span>
            {renderFields(modeFields.value.colors)}
          </div>
          <div class="ks-style-section">
            <span class="ks-style-section-label">常用布局</span>
            {renderFields(modeFields.value.layout)}
          </div>
          <details class="ks-style-advanced">
            <summary>高级样式</summary>
            {renderFields(modeFields.value.advanced)}
          </details>
        </div>
      )
    }
  },
})

const StyleControl = defineComponent({
  props: {
    field: { type: Object as () => StyleField, required: true },
    value: { required: false },
    appearance: { type: String as () => Appearance, required: true },
    inherited: { type: Boolean, required: true },
    onUpdate: { type: Function as unknown as () => (value: unknown) => void, required: true },
    onReset: { type: Function as unknown as () => () => void, required: true },
  },
  setup(props) {
    return () => {
      const field = props.field
      const value = props.value ?? fallback(field, props.appearance)
      return (
        <label class={{ 'ks-style-control': true, toggle: field.kind === 'boolean' }} title={field.path}>
          <span>{field.label}</span>
          {field.kind === 'boolean' ? (
            <button type="button" class={{ active: Boolean(value) }} onClick={() => props.onUpdate(!value)}><i />{value ? '开启' : '关闭'}</button>
          ) : field.kind === 'color' ? (
            <div class="ks-style-color">
              <input type="color" value={normalizeColor(value, props.appearance)} onInput={(event) => props.onUpdate(withAlpha((event.target as HTMLInputElement).value, value))} />
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
          <button
            type="button"
            class={{ 'ks-style-reset': true, inherited: props.inherited }}
            disabled={props.inherited}
            onClick={(event) => { event.preventDefault(); props.onReset() }}
          >{props.inherited ? '默认' : '重置'}</button>
        </label>
      )
    }
  },
})

function fallback(field: StyleField, appearance: Appearance): unknown {
  if (field.kind === 'boolean') return false
  if (field.kind === 'number') return field.min === -1 ? -1 : field.min ?? 0
  if (field.kind === 'color') return appearance === 'light' ? '#6477D4FF' : '#6E82D6FF'
  return field.options?.[0] ?? ''
}

function normalizeColor(value: unknown, appearance: Appearance): string {
  const fallback = appearance === 'light' ? '#6477D4' : '#6E82D6'
  const source = String(value ?? fallback)
  return /^#[0-9a-f]{6}/i.test(source) ? source.slice(0, 7) : fallback
}

function withAlpha(next: string, previous: unknown): string {
  const source = String(previous ?? '')
  return `${next.toUpperCase()}${/^#[0-9a-f]{8}$/i.test(source) ? source.slice(7, 9).toUpperCase() : 'FF'}`
}

function modeLabel(mode: TargetingMode): string {
  return mode === 'grid' ? 'Grid' : mode === 'recursive_grid' ? 'Recursive Grid' : 'UI Hint'
}
