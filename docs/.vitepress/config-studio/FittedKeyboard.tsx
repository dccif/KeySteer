import { defineComponent, onBeforeUnmount, onMounted, ref } from 'vue'

/** Fit the complete keyboard without clipping keys or adding a nested scroller. */
export default defineComponent({
  setup(_, { slots }) {
    const viewport = ref<HTMLElement>()
    const content = ref<HTMLElement>()
    const scale = ref(1)
    const height = ref(220)
    let observer: ResizeObserver | undefined
    function measure() {
      if (!viewport.value || !content.value) return
      const width = content.value.scrollWidth
      if (!width || !viewport.value.clientWidth) return
      scale.value = Math.min(1, viewport.value.clientWidth / width)
      height.value = content.value.offsetHeight * scale.value
    }
    onMounted(() => {
      observer = new ResizeObserver(measure)
      observer.observe(viewport.value!)
      observer.observe(content.value!)
      measure()
    })
    onBeforeUnmount(() => observer?.disconnect())
    return () => <div ref={viewport} class="ks-keyboard-fit" style={{ height: `${height.value}px` }}>
      <div ref={content} class="ks-keyboard-fit-content" style={{ transform: `scale(${scale.value})` }}>{slots.default?.()}</div>
    </div>
  },
})
