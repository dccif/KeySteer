import { useStudioI18n } from './i18n'
import { defineComponent } from 'vue'

export default defineComponent({
  props: {
    rows: { type: Array as () => string[], required: true },
    settings: { type: Object, required: true },
    appearance: { type: String, required: true },
    pointer: { type: Object as () => { x: number; y: number }, required: true },
    windowCenter: { type: Object as () => { x: number; y: number }, default: () => ({ x: 50, y: 50 }) },
    onSelect: { type: Function as unknown as () => (mode: string) => void, required: true },
  },
  setup(props) {
    const { t } = useStudioI18n()
    return () => {
      const ui = props.settings.ui ?? {}
      const color = (value: any, fallback: string) => typeof value === 'string' ? value : value?.[props.appearance] ?? fallback
      const auto = (value: any, fallback: number) => value == null || Number(value) < 0 ? fallback : Number(value)
      const size = auto(ui.font_size, 28)
      const mouse = (props.settings.position ?? 'mouse') === 'mouse'
      const height = props.rows.length * size * 1.8 + auto(ui.padding_y, size * .2) * 2 + 2
      const width = Math.max(...props.rows.map(row => row.length), 12) * size * .65 + size * 1.3 + auto(ui.padding_x, 10) * 2
      const center = props.settings.position === 'window' ? props.windowCenter : { x: 50, y: 50 }
      return <div class="ks-quick-preview" style={{
        left: mouse ? `clamp(8px, calc(${props.pointer.x}% + 12px), calc(100% - ${width}px - 8px))` : `clamp(${width / 2}px, ${center.x}%, calc(100% - ${width / 2}px))`,
        top: mouse ? `clamp(8px, calc(${props.pointer.y}% + 24px), calc(100% - ${height}px - 8px))` : `clamp(${Math.min(height / 2, 250)}px, ${center.y}%, calc(100% - ${Math.min(height / 2, 250)}px))`,
        transform: mouse ? 'none' : 'translate(-50%, -50%)',
        fontSize: `${size}px`, fontFamily: ui.font_family || 'inherit',
        color: color(ui.text_color, props.appearance === 'dark' ? '#e8eeff' : '#1a3479'),
        background: color(ui.background_color, props.appearance === 'dark' ? '#182444' : '#eef2ff'),
        border: `${auto(ui.border_width, 1)}px solid ${color(ui.border_color, '#8b9cda')}`,
        borderRadius: `${auto(ui.border_radius, size * .35)}px`,
        padding: `${auto(ui.padding_y, size * .2)}px ${auto(ui.padding_x, 10)}px`,
        paddingLeft: `${auto(ui.padding_x, 10) + size * .55}px`,
        paddingRight: `${auto(ui.padding_x, 10) * .5}px`,
      }}>
        {props.rows.length ? props.rows.map((mode, index) => <button type="button" onMousedown={event => event.preventDefault()} onClick={() => props.onSelect(mode)}>
          <kbd>{index + 1}</kbd><span>{mode}</span>
        </button>) : <span>{t("没有可切换的模式")}</span>}
      </div>
    }
  },
})
