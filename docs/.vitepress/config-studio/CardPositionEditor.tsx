import { useStudioI18n } from './i18n'
import { computed, defineComponent, ref } from 'vue'
import { cardPositionRatios, positionFromPoints } from '../simulator/window-card-position.ts'

export default defineComponent({
  name: 'CardPositionEditor',
  props: {
    position: { type: Array as () => string[], required: true },
    reference: { type: String, default: 'window' },
  },
  emits: { change: (_value: string[]) => true },
  setup(props, { emit }) {
    const { t } = useStudioI18n()
    const range = ref(false)
    const dragging = ref(false)
    const insets = computed(() => cardPositionRatios(props.position))
    let start = { x: 0, y: 0 }
    let previous: string[] = []
    let pointer: number | undefined
    const point = (event: PointerEvent) => {
      const bounds = (event.currentTarget as HTMLElement).getBoundingClientRect()
      return { x: Math.max(0, Math.min(1, (event.clientX - bounds.left) / bounds.width)),
        y: Math.max(0, Math.min(1, (event.clientY - bounds.top) / bounds.height)) }
    }
    const draw = (event: PointerEvent) => {
      const end = point(event)
      emit('change', positionFromPoints(range.value ? start : end, end))
    }
    const presets: Array<[string, string[]]> = [
      ['居中', ['50%', '50%', '50%', '50%']], ['顶部横排', ['0%', '0%', '100%', '0%']],
      ['底部横排', ['100%', '0%', '0%', '0%']], ['左侧', ['0%', '100%', '0%', '0%']],
      ['右侧', ['0%', '0%', '0%', '100%']], ['整个区域', ['0%', '0%', '0%', '0%']],
    ]
    return () => {
      const [top, right, bottom, left] = insets.value
      const width = Math.max(0, 1 - left - right), height = Math.max(0, 1 - top - bottom)
      return <div class="ks-position-editor">
        <div class="ks-position-heading"><strong>{t("可视化定位 · ")}{props.reference === 'screen' ? t("当前屏幕") : t("窗口")}</strong>
          <div class="ks-position-tools">
            <button type="button" aria-pressed={!range.value} onClick={() => { range.value = false }}>{t("定位点")}</button>
            <button type="button" aria-pressed={range.value} onClick={() => { range.value = true }}>{t("绘制范围")}</button>
          </div>
        </div>
        <p>{range.value ? t("在示意图上拖出定位范围；点击可收为一个点。") : t("点击或拖动，设置卡片的首选中心点。")}{t(" 实际卡片仍会避让编号和帮助面板。")}</p>
        <div class={{ 'ks-position-canvas': true, dragging: dragging.value }} aria-label={t("位置示意图，支持鼠标或触摸拖动；也可使用下方边距滑块")}
          onPointerdown={event => {
            if (!event.isPrimary || event.button !== 0 || dragging.value) return
            event.preventDefault()
            previous = [...props.position]
            start = point(event)
            pointer = event.pointerId
            dragging.value = true
            ;(event.currentTarget as HTMLElement).setPointerCapture(event.pointerId)
            draw(event)
          }} onPointermove={event => { if (dragging.value && event.pointerId === pointer) draw(event) }}
          onPointerup={event => {
            if (event.pointerId !== pointer) return
            draw(event); dragging.value = false; pointer = undefined
          }} onPointercancel={() => { if (dragging.value) emit('change', previous); dragging.value = false; pointer = undefined }}
          onLostpointercapture={() => { dragging.value = false; pointer = undefined }}>
          <span class="ks-position-origin">{props.reference === 'screen' ? t("当前屏幕可用区域") : t("窗口 / 布局区域")}</span>
          <div class="ks-position-selection" style={{ left: `${left * 100}%`, top: `${top * 100}%`, width: `${width * 100}%`, height: `${height * 100}%` }} />
          <i class="ks-position-point" style={{ left: `${(left + width / 2) * 100}%`, top: `${(top + height / 2) * 100}%` }} />
        </div>
        <div class="ks-position-presets">{presets.map(([name, value]) => <button type="button" onClick={() => emit('change', [...value])}>{t(name)}</button>)}</div>
        <div class="ks-position-sliders">{[t("上"), t("右"), t("下"), t("左")].map((name, index) => <label>
          <span>{t(name)} <output>{props.position[index]}</output></span>
          <input type="range" aria-label={t("{0}边距百分比", [name])} min="0" max="100" step="0.1" value={insets.value[index] * 100} onInput={event => {
            const next = [...props.position]
            const value = Number((event.target as HTMLInputElement).value)
            const opposite = (index + 2) % 4
            next[index] = `${value}%`
            if (value + insets.value[opposite] * 100 > 100) next[opposite] = `${Math.round((100 - value) * 10) / 10}%`
            emit('change', next)
          }} />
        </label>)}</div>
      </div>
    }
  },
})
