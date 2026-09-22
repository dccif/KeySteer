import { useStudioI18n } from './i18n'
import { defineComponent } from 'vue'
import type { ConfigDocument } from './document'

export default defineComponent({
  props: {
    document: { type: Object as () => ConfigDocument, required: true },
    mode: { type: String, required: true },
    appearance: { type: String, required: true },
  },
  setup(props) {
    const { t } = useStudioI18n()
    return () => {
      const card = props.document.window?.card ?? {}, ui = props.document[props.mode]?.ui ?? {}
      const theme = props.document.theme?.[props.appearance] ?? {}
      const color = (value: any, fallback: string): string => typeof value === 'string' ? value : value?.[props.appearance] ?? fallback
      const text = color(ui.text_color, color(theme.text, props.appearance === 'dark' ? '#E8EEFFFF' : '#10172DFF'))
      const border = color(card.border_color, color(ui.border_color, color(theme.accent, '#6477D4FF')))
      const number = Number(ui.font_size ?? 28)
      const auto = (value: unknown, fallback: number) => value === undefined || Number(value) === -1 ? fallback : Number(value)
      const appSize = Number(card.app_font_size) || Math.max(number * .6, 14)
      const titleSize = Number(card.title_font_size) || Math.max(number * .45, 12)
      const row = Math.max(appSize, titleSize) * Number(card.line_height ?? 1.4)
      const radius = auto(ui.border_radius, Math.round(number * .35))
      const line = `${ui.border_width ?? 1}px solid ${border}`
      const label = { overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap', lineHeight: `${row}px` } as const
      return <div class="ks-card-style-preview">
        <div class="ks-card-preview-caption"><strong>{t("卡片外观 · 实时预览")}</strong><span>{t("位置范围共用；编号字号和边框尺寸使用当前模式的 ui。")}</span></div>
        <div class="ks-card-preview-stage">
          {card.guide_line_enabled !== false && <div style={{ width: '70px', margin: '0 auto 8px',
            borderTop: `${card.guide_line_width ?? 3}px solid ${color(card.guide_line_color, border)}` }} />}
          <div class="ks-card-preview-sample" style={{ border: line, borderRadius: `${radius}px`,
            background: color(card.background_color, color(ui.background_color, color(theme.surface, props.appearance === 'dark' ? '#0A1338FF' : '#EEF2FFFF'))),
            minHeight: `${card.min_height ?? 44}px`, fontFamily: ui.font_family || 'var(--vp-font-family-base)' }}>
            <b style={{ display: 'flex', alignItems: 'center', justifyContent: 'center', flexShrink: 0,
              fontSize: `${number}px`, color: color(card.number_color, text), border: line, borderRadius: `${radius}px`,
              minWidth: `${Math.max(Number(card.number_min_width ?? 38), number * .75 + auto(ui.padding_x, Math.round(number * .4)) * 2)}px`,
              padding: `${auto(ui.padding_y, Math.round(number * .2))}px 0`, lineHeight: '1.4' }}>1</b>
            <div style={{ width: `${card.text_width ?? 260}px`, boxSizing: 'content-box', minWidth: 0,
              padding: `${card.padding_y ?? 4}px ${card.padding_x ?? 9}px` }}>
              <div style={{ ...label, fontSize: `${appSize}px`, color: color(card.app_color, text), fontWeight: card.app_bold === false ? 400 : 700, fontFamily: card.app_font_family || 'inherit' }}>KeySteer</div>
              <div style={{ ...label, fontSize: `${titleSize}px`, color: color(card.title_color, text), fontWeight: card.title_bold ? 700 : 400, fontFamily: card.title_font_family || 'inherit' }}>{t("窗口标题 — Window title")}</div>
            </div>
          </div>
        </div>
      </div>
    }
  },
})
