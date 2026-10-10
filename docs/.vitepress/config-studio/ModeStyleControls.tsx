import { useStudioI18n } from './i18n'
import { fieldLocation, type SettingsTab } from './navigation'
import { cardPositionRatios } from '../simulator/window-card-position.ts'
import CardPositionEditor from './CardPositionEditor'
import CardStylePreview from './CardStylePreview'
import { selectedCardStyle } from '../simulator/window-card-style'
import { searchPanelColors } from '../simulator/search-input-style'
import { parseSplitRatios } from '../simulator/window-ratios.ts'
import { computed, defineComponent } from 'vue'
import {
  cloneConfigDocument,
  deleteConfigPath,
  getConfigPath,
  setConfigPath,
  type ConfigDocument,
} from './document'

import { lifecycleOptions, fields, paletteFields, paletteLabels, type TargetingMode, type Appearance, type StyleField } from './fields'
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
    const colorPreview = computed(() => ({ enabled: true, width: 16, height: 16, x_offset: 4, y_offset: 0, border_width: 1, ...getConfigPath(props.effectiveDocument, 'ui_hint.search_point.color_preview') as Record<string, number | boolean> }))
    const inputColors = computed(() => searchPanelColors(props.effectiveDocument, props.appearance))
    const previewPanelStyle = (block: 'search_input_ui' | 'search_info_ui') => {
      const ui = { width: block === 'search_input_ui' ? 280 : 520, font_size: 14,
        padding_y: block === 'search_input_ui' ? 6 : 16, ...props.effectiveDocument.ui_hint?.[block] }
      const colors = searchPanelColors(props.effectiveDocument, props.appearance, block)
      const font = Number(ui.font_size)
      const paddingY = ui.padding_y >= 0 ? ui.padding_y : Math.round(font * .2)
      return { boxSizing: 'border-box' as const, width: `${ui.width}px`, maxWidth: '100%',
        textAlign: 'left' as const, lineHeight: '1.8', fontSize: `${font}px`, fontWeight: 400, fontFamily: ui.font_family || 'system-ui, sans-serif',
        borderStyle: 'solid', borderWidth: `${Math.max(0, ui.border_width ?? 1)}px`,
        borderRadius: `${ui.border_radius >= 0 ? ui.border_radius : Math.round(font * .35)}px`,
        height: block === 'search_input_ui' ? `${font * 1.8 + paddingY * 2}px` : undefined,
        padding: `${block === 'search_input_ui' ? 0 : paddingY}px ${Math.max(block === 'search_info_ui' ? 10 : 0, ui.padding_x >= 0 ? ui.padding_x : Math.round(font * .4))}px`,
        color: colors.text_color, background: colors.background_color, borderColor: colors.border_color }
    }
    const inputPreviewStyle = computed(() => previewPanelStyle('search_input_ui'))
    const infoPreviewStyle = computed(() => previewPanelStyle('search_info_ui'))
    const modeFields = computed(() => fields[props.mode])
    const isSearchInputField = (field: StyleField) => field.path.startsWith('ui_hint.search_input_ui.') || field.path.startsWith('ui_hint.search_point.input_')
    const isColorPreviewField = (field: StyleField) => field.path.startsWith('ui_hint.search_point.color_preview.')
    const isCardField = (field: StyleField) => props.mode.startsWith('window') &&
      ((field.path.startsWith('window.card.') || field.path.startsWith('window_editor.card.')) || /^window(?:_\w+)?\.ui\./.test(field.path))
    const fieldValue = (field: StyleField): unknown => {
      const configured = getConfigPath(props.effectiveDocument, field.path)
      if (configured === undefined && field.kind === 'color' && /^ui_hint\.(search_(input|info)_ui\.|search_point\.input_)/.test(field.path)) {
        const block = field.path.includes('.search_info_ui.') ? 'search_info_ui' : 'search_input_ui'
        const key = field.path.includes('.search_point.') ? field.path.split('.').at(-1)!.replace('input_', 'point_') : field.path.split('.').at(-1)!
        return Object.fromEntries(['light', 'dark'].map(appearance => [appearance, searchPanelColors(props.effectiveDocument, appearance, block)[key as keyof ReturnType<typeof searchPanelColors>]]))
      }
      if (field.kind === 'binding') return Object.entries(configured as Record<string, string> ?? {}).filter(([, action]) => action === field.action).map(([key]) => key).join(' ')
      if (configured === undefined && field.path === 'ui_hint.search_point.marker_color') {
        return Object.fromEntries(['light', 'dark'].map(appearance => [appearance, props.effectiveDocument.theme?.[appearance]?.accent ?? (appearance === 'dark' ? '#7F9AFFFF' : '#4965D9FF')]))
      }
      if (field.path.includes('.lifecycle.')) return configured ?? 'keep'
      if (configured !== undefined || !isCardField(field)) return configured
      if (field.path.endsWith('.ui.border_width')) return 1
      if (field.path === 'window.card.selected_border_width') return 1.5
      if (field.kind !== 'color') return undefined
      if (field.path === 'window.card.selected_background_color' || field.path === 'window.card.selected_border_color') {
        const key = field.path.endsWith('background_color') ? 'background' : 'border'
        return Object.fromEntries(['light', 'dark'].map(appearance => [appearance, selectedCardStyle({}, appearance)[key]]))
      }
      const ui = props.effectiveDocument[props.mode]?.ui ?? {}
      const key = field.path.split('.').at(-1)
      const source = key === 'background_color' ? 'surface' : key === 'border_color' ? 'accent' : 'text'
      const inherited = ui[key === 'background_color' ? 'background_color' : key === 'border_color' ? 'border_color' : 'text_color']
      return Object.fromEntries(['light', 'dark'].map(appearance => [appearance,
        (typeof inherited === 'string' ? inherited : inherited?.[appearance]) ?? props.effectiveDocument.theme?.[appearance]?.[source] ?? (appearance === 'dark' ? '#E8EEFFFF' : '#17327AFF')]))
    }

    function update(path: string, value: unknown): void {
      const next = cloneConfigDocument(props.document)
      if (/^window(?:_quick|_editor|_restore|_tab)?\.target$/.test(path) && value === '') deleteConfigPath(next, path)
      else setConfigPath(next, path, value)
      emit('change', next)
    }

    function reset(path: string): void {
      const next = cloneConfigDocument(props.document)
      deleteConfigPath(next, path)
      emit('change', next)
    }

    function updateBinding(field: StyleField, value: unknown): void {
      const table = { ...getConfigPath(props.effectiveDocument, field.path) as Record<string, string> }
      for (const [key, action] of Object.entries(table)) if (action === field.action) delete table[key]
      for (const key of String(value).trim().split(/\s+/).filter(Boolean)) table[key] = field.action!
      if (field.path === 'ui_hint.search_edit_keys' && !String(value).trim()) {
        let key = ''
        while (key in table) key += ' '
        table[key] = field.action!
      }
      update(field.path, table)
    }

    const renderFields = (items: StyleField[]) => (
      <div class="ks-style-fields">
        {items.filter(field => { const location = fieldLocation(field.path); return location.page === props.page && location.tab === props.tab }).map((field) => (
          <StyleControl
            field={field.path.includes('.lifecycle.') ? { ...field, options: lifecycleOptions(field.path.endsWith('after_finish') ? 'after_finish' : 'after_click', Object.keys(props.effectiveDocument.plugin_modes ?? {})) } : field}
            value={fieldValue(field)}
            appearance={props.appearance}
            inherited={getConfigPath(props.document, field.path) === undefined}
            onUpdate={(value) => {
              if (field.kind !== 'binding') { update(field.path, value); return }
              updateBinding(field, value)
            }}
            onReset={() => field.kind === 'binding' ? updateBinding(field, field.default) : reset(field.path)}
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
            <div><h2>{props.tab === 'behavior' ? t("行为参数") : t("外观设置")}</h2><span>{t("修改即时显示在右侧预览；默认值继续继承")}</span></div>
            <div class="ks-appearance-switch" aria-label={t("预览配色")}>
              {(['dark', 'light'] as Appearance[]).map((appearance) => (
                <button class={{ active: props.appearance === appearance }} onClick={() => emit('appearanceChange', appearance)}>
                  {appearance === 'dark' ? t("深色") : t("浅色")}
                </button>
              ))}
            </div>
          </div>
          {props.page === 'window_editor' && props.tab === 'appearance' && <p>{t("卡片颜色与字体继承窗口共用外观；此处只覆盖布局编辑的位置与当前模式标记。")}</p>}
          {props.page === 'ui_hint' && props.tab === 'appearance' && <section class="ks-style-section ks-settings-section ks-search-preview-section" aria-label={t('输入框状态预览')}>
            <header class="ks-settings-section-heading">
              <h2>{t('输入框状态预览')}</h2>
              <p>{t('对比搜索输入与点位调整；颜色、字体和尺寸修改即时显示。')}</p>
            </header>
            <div class="ks-settings-section-body">
              <div class="ks-search-input-samples">
                <figure>
                  <figcaption>{t('搜索输入')}</figcaption>
                  <div class="ks-search-preview-input" aria-label={t('普通搜索输入框')} style={inputPreviewStyle.value}>
                    <span class="ks-search-preview-query">ajh akl<i class="ks-search-preview-caret" aria-hidden="true" /></span>
                  </div>
                </figure>
                <figure>
                  <figcaption>{t('点位调整')}</figcaption>
                  <div class="ks-search-preview-input" aria-label={t('Point 输入框')} style={{ ...inputPreviewStyle.value, background: inputColors.value.point_background_color, borderColor: inputColors.value.point_border_color }}>
                    <span class="ks-search-preview-query">ajh akl</span>
                    <span class="ks-search-preview-counter" style={{ color: props.appearance === 'dark' ? '#9CA3AF' : '#6B7280' }}>0/2</span>
                    <i class="ks-search-preview-dot" aria-hidden="true" style={{ background: props.appearance === 'dark' ? '#68D9B1' : '#16856B' }} />
                  </div>
                </figure>
              </div>
              <div class="ks-search-input-controls">{renderFields(modeFields.value.colors.filter(isSearchInputField))}</div>
              <details class="ks-style-advanced">
                <summary>{t('输入框字体、尺寸与位置')}</summary>
                {renderFields(modeFields.value.advanced.filter(isSearchInputField))}
              </details>
            </div>
          </section>}
          {props.page === 'ui_hint' && props.tab === 'appearance' && <section class="ks-style-section ks-settings-section ks-search-preview-section" aria-label={t('颜色色块预览（示例颜色）')}>
            <header class="ks-settings-section-heading">
              <h2>{t('颜色色块预览（示例颜色）')}</h2>
              <p>{t('色块紧邻 Color 标题；在下方调整开关、尺寸、偏移和边框。')}</p>
            </header>
            <div class="ks-settings-section-body">
              <div class="ks-search-color-sample" style={infoPreviewStyle.value}>
                <div class="ks-search-color-title"><kbd class="ks-search-preview-key">4</kbd><span>Color</span>
                  {colorPreview.value.enabled && <span data-color-swatch style={{ flexShrink: 0, boxSizing: 'border-box', width: `${colorPreview.value.width}px`, height: `${colorPreview.value.height}px`, marginLeft: `${colorPreview.value.x_offset}px`, transform: `translateY(${colorPreview.value.y_offset}px)`, border: `${colorPreview.value.border_width}px solid currentColor`, background: '#49A98A' }} />}
                </div>
                <div class="ks-search-color-value">#49A98A</div>
              </div>
              <div class="ks-search-color-controls">{renderFields(modeFields.value.layout.filter(isColorPreviewField))}</div>
            </div>
          </section>}
          {props.mode.startsWith('window') && props.tab === 'appearance' && <div class="ks-style-section ks-card-editor">
            <CardStylePreview document={props.effectiveDocument} mode={props.mode} appearance={props.appearance} />
            <div class="ks-card-editor-controls">
              <strong>{t("颜色、透明度与边框")}</strong>
              {renderFields(modeFields.value.colors.filter(isCardField))}
              {renderFields(modeFields.value.layout.filter(field => field.path === 'window.card.selected_border_width'))}
              <details class="ks-style-advanced"><summary>{t("高级设置 · 字体、尺寸与间距")}</summary>
              {renderFields(modeFields.value.advanced.filter(isCardField))}</details>
            </div>
            <strong>{t("位置与排列")}</strong>
            {renderFields(modeFields.value.layout.filter(field => isCardField(field) && field.path !== 'window.card.selected_border_width'))}
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
            {renderFields(modeFields.value.colors.filter(field => !isCardField(field) && !isSearchInputField(field)))}
          </div>
          <div class="ks-style-section">
            <span class="ks-style-section-label">{t("常用布局")}</span>
            {renderFields(modeFields.value.layout.filter(field => !isCardField(field) && !isColorPreviewField(field)))}
          </div>
          {props.tab === 'behavior' && modeFields.value.advanced.some(field => field.path.includes('.lifecycle.')) && <section class="ks-style-section ks-settings-section">
            <header class="ks-settings-section-heading">
              <h2>{t('生命周期')}</h2>
              <p>{t('keep 保留当前状态；restart 重新开始；return 返回上层模式。点击后仅响应成功的模拟点击。')}</p>
              <p>{t('生命周期写入 TOML；网页预览暂不完整模拟这些动作。')}</p>
            </header>
            <div class="ks-settings-section-body">
              {renderFields(modeFields.value.advanced.filter(field => field.path.includes('.lifecycle.')))}
            </div>
          </section>}
          <details class="ks-style-advanced">
            <summary>{t("高级设置")}</summary>
            {renderFields(modeFields.value.advanced.filter(field => !isCardField(field) && !isSearchInputField(field) && !field.path.includes('.lifecycle.')))}
          </details>
        </div>
      )
    }
  },
})

export const StyleControl = defineComponent({
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
      const source = props.value ?? field.default ?? fallback(field, props.appearance)
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
          ) : field.kind === 'offset' ? (
            <input value={JSON.stringify(value)} onChange={event => {
              const input = event.target as HTMLInputElement
              try {
                const pair = JSON.parse(input.value)
                if (!Array.isArray(pair) || pair.length !== 2 || !pair.every(v => Number.isInteger(v) && v >= -32768 && v <= 32767)) throw new Error()
                input.setCustomValidity(''); props.onUpdate(pair)
              } catch { input.setCustomValidity(t('请输入两个整数，例如 [-12, 18]')); input.reportValidity() }
            }} />
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
          ) : field.kind === 'chords' ? (
            <input value={Array.isArray(value) ? value.join(', ') : String(value)} onChange={event => {
              const input = event.target as HTMLInputElement
              const values = input.value.split(',').map(part => part.trim())
              if (values.length !== 4 || new Set(values).size !== 4 || values.some(value => !value.includes('+'))) {
                input.setCustomValidity(t('请输入四个不同的复制组合键')); input.reportValidity(); return
              }
              input.setCustomValidity(''); props.onUpdate(values)
            }} />
          ) : field.kind === 'choices' ? (
            <input value={Array.isArray(value) ? value.join(', ') : String(value)} onChange={event => {
              const input = event.target as HTMLInputElement
              const values = input.value.split(',').map(part => part.trim().toLowerCase())
              if (!values.length || new Set(values).size !== values.length || values.some(value => !field.options?.includes(value)) || field.requireAll && values.length !== field.options?.length) {
                input.setCustomValidity(field.requireAll ? t('请按优先级填写全部选项且不重复：{0}', [field.options?.join(', ') ?? '']) : t('请输入不重复的颜色格式：hex, rgb, hsl')); input.reportValidity(); return
              }
              input.setCustomValidity(''); props.onUpdate(values)
            }} />
          ) : field.kind === 'field-modes' ? (
            <div style={{ display: 'grid', gridTemplateColumns: '1fr 1fr', gap: '6px' }}>
              {['1 OCR', '2 Accessibility', '3 Coordinates', '4 Color'].map((title, index) => <span>
                {title}<select aria-label={t('{0}显示方式', [title])} value={(value as string[])[index]} onChange={event => {
                  const next = [...value as string[]]; next[index] = (event.target as HTMLSelectElement).value; props.onUpdate(next)
                }}><option value="concat">{t('拼接全部')}</option><option value="switch">{t('跟随当前点')}</option></select>
              </span>)}
            </div>
          ) : field.kind === 'select' ? (
            <select value={String(value)} onChange={(event) => props.onUpdate((event.target as HTMLSelectElement).value)}>
              {field.options?.map((option) => <option value={option}>{option || t('保留现有行为')}</option>)}
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
