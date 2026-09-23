import { computed, defineComponent, ref, onMounted, onBeforeUnmount } from 'vue'
import { useStudioI18n } from './i18n'
import { modeIndicatorPreview } from '../simulator/mode-indicator'
import type { ConfigDocument } from './document'

export default defineComponent({
  props: {
    document: { type: Object as () => ConfigDocument, required: true },
    mode: { type: String, required: true },
    appearance: { type: String, required: true },
  },
  emits: { change: (_offset: number[]) => true },
  setup(props, { emit }) {
    const { t } = useStudioI18n()
    const dragging = ref(false)
    const host = ref<HTMLElement>()
    const scale = ref(1)
    let observer: ResizeObserver | undefined
    onMounted(() => { observer = new ResizeObserver(entries => { scale.value = entries[0].contentRect.width / 360 }); if (host.value) observer.observe(host.value) })
    onBeforeUnmount(() => observer?.disconnect())
    let pointer: number | undefined
    let previous: number[] = []
    const resolvedBadge = computed(() => modeIndicatorPreview(props.document, props.mode, props.appearance, { x: 50, y: 50 }, { width: 360, height: 240 }))
    const badge = () => resolvedBadge.value
    const offset = () => [...badge().ui.indicator_offset]
    function draw(event: PointerEvent) {
      const element = event.currentTarget as HTMLElement
      const bounds = element.getBoundingClientRect()
      const pair = [(event.clientX - bounds.left - element.clientLeft) / scale.value - 180,
        (event.clientY - bounds.top - element.clientTop) / scale.value - 120]
      emit('change', pair.map(value => Math.max(-32768, Math.min(32767, Math.round(value)))))
    }
    return () => <div class="ks-indicator-position">
      <p>{t('拖动右上角锚点；圆心是鼠标热点。方向键微调，Shift 加速。')}</p>
      <div ref={host} class="ks-indicator-position-canvas" style={{ height: `${240 * scale.value}px` }} tabindex="0" role="group" aria-label={t('标识符拖动定位')}
        onPointerdown={event => {
          if (!event.isPrimary || event.button !== 0) return
          event.preventDefault(); previous = offset(); pointer = event.pointerId; dragging.value = true
          const element = event.currentTarget as HTMLElement
          element.focus(); element.setPointerCapture(event.pointerId); draw(event)
        }} onPointermove={event => { if (dragging.value && event.pointerId === pointer) draw(event) }}
        onPointerup={event => { if (event.pointerId === pointer) { draw(event); dragging.value = false; pointer = undefined } }}
        onPointercancel={() => { if (dragging.value) emit('change', previous); dragging.value = false; pointer = undefined }}
        onLostpointercapture={() => { dragging.value = false; pointer = undefined }}
        onKeydown={event => {
          if (!['ArrowLeft', 'ArrowRight', 'ArrowUp', 'ArrowDown'].includes(event.key)) return
          event.preventDefault(); const pair = offset(), step = event.shiftKey ? 10 : 1
          const axis = event.key === 'ArrowLeft' || event.key === 'ArrowRight' ? 0 : 1
          pair[axis] = Math.max(-32768, Math.min(32767, pair[axis] + (['ArrowLeft', 'ArrowUp'].includes(event.key) ? -step : step)))
          emit('change', pair)
        }}>
        <div class="ks-indicator-position-content" style={{ width: '360px', height: '240px', transformOrigin: 'top left', transform: `scale(${scale.value})` }}>
          <i class="ks-indicator-origin" />
          <div style={badge().style}>{badge().text}<i class="ks-indicator-anchor" /></div>
        </div>
      </div>
    </div>
  },
})
