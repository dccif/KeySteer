import { useStudioI18n } from './i18n'
import { fieldLocation, type SettingsTab } from './navigation'
import { cardPositionRatios } from '../simulator/window-card-position.ts'
import CardPositionEditor from './CardPositionEditor'
import CardStylePreview from './CardStylePreview'
import { parseSplitRatios } from '../simulator/window-ratios.ts'
import { computed, defineComponent } from 'vue'
import {
  cloneConfigDocument,
  deleteConfigPath,
  getConfigPath,
  setConfigPath,
  type ConfigDocument,
} from './document'

import { fields, paletteFields, paletteLabels, type TargetingMode, type Appearance, type StyleField } from './fields'
export default defineComponent({
  name: 'ModeStyleControls',
  props: {
    page: { type: String, required: true },
    tab: { type: String as () => SettingsTab, required: true },
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
    const { t } = useStudioI18n()
    const positionRoot = computed(() => props.mode === 'window_editor' ? 'window_editor.card' : 'window.card')
    const modeFields = computed(() => fields[props.mode])
    const isCardField = (field: StyleField) => props.mode.startsWith('window') &&
      ((field.path.startsWith('window.card.') || field.path.startsWith('window_editor.card.')) || /^window(?:_\w+)?\.ui\./.test(field.path))
    const fieldValue = (field: StyleField): unknown => {
      const configured = getConfigPath(props.effectiveDocument, field.path)
      if (configured !== undefined || !isCardField(field)) return configured
      if (field.path.endsWith('.ui.border_width')) return 1
      if (field.kind !== 'color') return undefined
      const ui = props.effectiveDocument[props.mode]?.ui ?? {}
      const key = field.path.split('.').at(-1)
      const source = key === 'background_color' ? 'surface' : key === 'border_color' ? 'accent' : 'text'
      const inherited = ui[key === 'background_color' ? 'background_color' : key === 'border_color' ? 'border_color' : 'text_color']
      return Object.fromEntries(['light', 'dark'].map(appearance => [appearance,
        (typeof inherited === 'string' ? inherited : inherited?.[appearance]) ?? props.effectiveDocument.theme?.[appearance]?.[source] ?? (appearance === 'dark' ? '#E8EEFFFF' : '#17327AFF')]))
    }

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
        {items.filter(field => { const location = fieldLocation(field.path); return location.page === props.page && location.tab === props.tab }).map((field) => (
          <StyleControl
            field={field}
            value={fieldValue(field)}
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
            <div><strong>{props.tab === 'behavior' ? t("行为参数") : t("外观设置")}</strong><span>{t("修改即时显示在右侧预览；默认值继续继承")}</span></div>
            <div class="ks-appearance-switch" aria-label={t("预览配色")}>
              {(['dark', 'light'] as Appearance[]).map((appearance) => (
                <button class={{ active: props.appearance === appearance }} onClick={() => emit('appearanceChange', appearance)}>
                  {appearance === 'dark' ? t("深色") : t("浅色")}
                </button>
              ))}
            </div>
          </div>
          {props.page === 'window_editor' && props.tab === 'appearance' && <p>{t("卡片颜色与字体继承窗口共用外观；此处只覆盖布局编辑的位置与当前模式标记。")}</p>}
          {props.mode.startsWith('window') && props.tab === 'appearance' && <div class="ks-style-section ks-card-editor">
            <CardStylePreview document={props.effectiveDocument} mode={props.mode} appearance={props.appearance} />
            <div class="ks-card-editor-controls">
              <strong>{t("颜色、透明度与边框")}</strong>
              {renderFields(modeFields.value.colors.filter(isCardField))}
              <details class="ks-style-advanced"><summary>{t("高级设置 · 字体、尺寸与间距")}</summary>
              {renderFields(modeFields.value.advanced.filter(isCardField))}</details>
            </div>
            <strong>{t("位置与排列")}</strong>
            {renderFields(modeFields.value.layout.filter(isCardField))}
            {(props.page === 'window_card' || props.page === 'window_editor') && <CardPositionEditor
              position={getConfigPath(props.effectiveDocument, positionRoot.value + '.position') as string[] ?? ['50%', '50%', '50%', '50%']}
              reference={String(getConfigPath(props.effectiveDocument, positionRoot.value + '.position_mode') ?? 'window')}
              onChange={value => update(positionRoot.value + '.position', value)} />}
          </div>}
          <div class="ks-style-section" hidden={props.page !== 'key_help'}>
            <span class="ks-style-section-label">{props.appearance === 'dark' ? t("深色主题") : t("浅色主题")}</span>
            {renderFields(palette)}
          </div>
          <div class="ks-style-section" hidden={props.tab !== 'appearance'}>
            <span class="ks-style-section-label">{t("模式颜色")}</span>
            {renderFields(modeFields.value.colors.filter(field => !isCardField(field)))}
          </div>
          <div class="ks-style-section">
            <span class="ks-style-section-label">{t("常用布局")}</span>
            {renderFields(modeFields.value.layout.filter(field => !isCardField(field)))}
          </div>
          <details class="ks-style-advanced">
            <summary>{t("高级设置")}</summary>
            {renderFields(modeFields.value.advanced.filter(field => !isCardField(field)))}
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
    const { t } = useStudioI18n()
    return () => {
      const field = props.field
      const source = props.value ?? fallback(field, props.appearance)
      const variants = field.kind === 'color' && source && typeof source === 'object' ? source as Record<string, string> : undefined
      const value = variants?.[props.appearance] ?? source
      const updateColor = (next: string) => props.onUpdate(variants ? { ...variants, [props.appearance]: next } : next)
      return (
        <label class={{ 'ks-style-control': true, toggle: field.kind === 'boolean' }} title={field.path} data-config-path={field.path}>
          <span>{t(field.label)}</span>
          {field.kind === 'boolean' ? (
            <button type="button" class={{ active: Boolean(value) }} onClick={() => props.onUpdate(!value)}><i />{value ? t("开启") : t("关闭")}</button>
          ) : field.kind === 'color' ? (
            <div class="ks-style-color">
              <input type="color" value={normalizeColor(value, props.appearance)} onInput={(event) => updateColor(withAlpha((event.target as HTMLInputElement).value, value))} />
              <input value={String(value)} onInput={(event) => updateColor((event.target as HTMLInputElement).value)} />
              <input class="ks-color-alpha" type="range" aria-label={t("{0}不透明度", [t(field.label)])} title={t("不透明度")} min="0" max="255"
                value={/^#[0-9a-f]{8}$/i.test(String(value)) ? parseInt(String(value).slice(7), 16) : 255}
                onInput={event => updateColor(`${normalizeColor(value, props.appearance)}${Number((event.target as HTMLInputElement).value).toString(16).padStart(2, '0')}`)} />
            </div>
          ) : field.kind === 'percentages' ? (
            <input value={Array.isArray(value) ? value.join(', ') : String(value)} onChange={event => {
              const input = event.target as HTMLInputElement
              const values = input.value.split(',').map(part => part.trim())
              try { cardPositionRatios(values); input.setCustomValidity(''); props.onUpdate(values) }
              catch (error) { input.setCustomValidity(String(error)); input.reportValidity() }
            }} />
          ) : field.kind === 'ratios' ? (
            <input value={Array.isArray(value) ? value.join(', ') : String(value)} onChange={event => {
              const input = event.target as HTMLInputElement
              const values = input.value.split(',').map(part => part.trim()).map(part => part.includes('/') ? part : Number(part))
              try { parseSplitRatios(values); input.setCustomValidity(''); props.onUpdate(values) }
              catch (error) { input.setCustomValidity(String(error)); input.reportValidity() }
            }} />
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
          >{props.inherited ? t("默认") : t("重置")}</button>
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
