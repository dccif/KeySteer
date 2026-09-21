import { defineComponent, ref } from 'vue'

/** Presentation widths never modify the configuration document. */
export default defineComponent({
  props: { navigation: Boolean, value: { type: Number, required: true }, disabled: Boolean },
  emits: { change: (_value: number) => true, start: () => true },
  setup(props, { emit }) {
    const dragging = ref(false)
    let origin = 0, initial = 0, span = 1
    const min = () => props.navigation ? 180 : 35
    const max = () => props.navigation ? 320 : 70
    const update = (value: number) => emit('change', Math.max(min(), Math.min(max(), value)))
    function start(event: PointerEvent) {
      if (event.button !== 0 || props.disabled) return
      const handle = event.currentTarget as HTMLElement
      const workspace = handle.parentElement!
      span = Math.max(1, workspace.querySelector('.ks-settings-pane')!.getBoundingClientRect().width + workspace.querySelector('.ks-preview-pane')!.getBoundingClientRect().width)
      origin = event.clientX
      initial = props.value
      dragging.value = true
      handle.setPointerCapture(event.pointerId)
      handle.focus()
      emit('start')
      event.preventDefault()
    }
    return () => <div
      class={{ 'ks-pane-divider': true, 'ks-nav-divider': props.navigation, 'is-dragging': dragging.value }}
      role="separator" tabindex={props.disabled ? -1 : 0} aria-orientation="vertical"
      aria-label={props.navigation ? '调整分类导航宽度' : '调整设置与预览宽度'}
      aria-valuemin={min()} aria-valuemax={max()} aria-valuenow={Math.round(props.value)}
      aria-valuetext={props.navigation ? `${Math.round(props.value)} 像素` : `设置占 ${Math.round(props.value)}%`}
      title="拖动调整宽度 · 方向键微调 · 双击恢复默认"
      onPointerdown={start}
      onPointermove={event => { if (dragging.value) update(initial + (event.clientX - origin) * (props.navigation ? 1 : 100 / span)) }}
      onPointerup={event => { dragging.value = false; (event.currentTarget as HTMLElement).releasePointerCapture(event.pointerId) }}
      onPointercancel={() => dragging.value = false}
      onLostpointercapture={() => dragging.value = false}
      onDblclick={() => { if (!props.disabled) update(props.navigation ? 240 : 55) }}
      onKeydown={event => {
        if (props.disabled || !['ArrowLeft', 'ArrowRight', 'Home', 'End'].includes(event.key)) return
        event.preventDefault()
        emit('start')
        update(event.key === 'Home' ? min() : event.key === 'End' ? max() : props.value + (event.key === 'ArrowLeft' ? -1 : 1) * (props.navigation ? 16 : 2))
      }}
    ><span /></div>
  },
})
