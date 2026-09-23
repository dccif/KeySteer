import { defineComponent } from 'vue'
import { StyleControl } from './ModeStyleControls'
import { indicatorFields } from './indicator-fields'
import { resolveModeIndicator, modeIndicatorPreview } from '../simulator/mode-indicator'
import { cloneConfigDocument, deleteConfigPath, getConfigPath, setConfigPath, type ConfigDocument } from './document'
import { useStudioI18n } from './i18n'
import IndicatorPositionEditor from './IndicatorPositionEditor'

export default defineComponent({
  props: {
    mode: String,
    document: { type: Object as () => ConfigDocument, required: true },
    effectiveDocument: { type: Object as () => ConfigDocument, required: true },
    appearance: { type: String as () => 'light' | 'dark', required: true },
  },
  emits: { change: (_document: ConfigDocument) => true },
  setup(props, { emit }) {
    const { t } = useStudioI18n()
    function update(path: string, value?: unknown) {
      const next = cloneConfigDocument(props.document)
      if (value === undefined) deleteConfigPath(next, path)
      else setConfigPath(next, path, value)
      emit('change', next)
    }
    return () => {
      const mode = props.mode ?? 'normal'
      const resolved = resolveModeIndicator(props.effectiveDocument, props.mode ?? '')
      const previewDocument = props.mode ? props.effectiveDocument : { ...props.effectiveDocument, mode_indicator: { ...props.effectiveDocument.mode_indicator, modes: {} } }
      const preview = modeIndicatorPreview(previewDocument, mode, props.appearance, { x: 50, y: 50 })
      const controls = <div class="ks-style-controls">
        <strong>{t('模式标识符')}</strong>
        <p>{t(props.mode ? '位置相对模拟鼠标；正 X 向右，正 Y 向下。重置后继承全局样式。' : '所有模式共用此样式；单个模式可在外观页覆盖。-1 表示自动尺寸。')}</p>
        <IndicatorPositionEditor document={previewDocument} mode={mode} appearance={props.appearance} onChange={value => update(`mode_indicator${props.mode ? `.modes.${props.mode}` : ''}.ui.indicator_offset`, value)} />
        <div class="ks-style-fields">{indicatorFields(props.mode).map(field => {
          const key = field.path.split('.').at(-1)!
          const value = key === 'indicator_offset' ? resolved.ui.indicator_offset : key === 'enabled' ? resolved.enabled : key === 'text' ? resolveModeIndicator(props.effectiveDocument, mode).text
            : resolved.ui[key] ?? (key === 'background_color' ? preview.style.background : key === 'text_color' ? preview.style.color : props.effectiveDocument.theme?.[props.appearance]?.accent)
          return <StyleControl field={field} value={value} appearance={props.appearance}
            inherited={getConfigPath(props.document, field.path) === undefined}
            onUpdate={value => update(field.path, value)} onReset={() => update(field.path)} />
        })}</div>
      </div>
      return props.mode ? <details class="ks-style-advanced"><summary>{t('模式标识符 · 单独覆盖（可选）')}</summary>{controls}</details> : controls
    }
  },
})
