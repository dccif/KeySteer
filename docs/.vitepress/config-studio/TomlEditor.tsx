import { computed, defineComponent, ref } from 'vue'
import { highlightTomlCode } from './toml-highlight'

/** A native textarea retains selection, paste and IME; a non-interactive mirror adds color. */
export default defineComponent({
  props: { value: { type: String, required: true }, label: String, placeholder: String, readonly: Boolean, disabled: Boolean },
  emits: { change: (_value: string) => true },
  setup(props, { emit }) {
    const scroll = ref({ x: 0, y: 0 })
    const highlighted = computed(() => highlightTomlCode(props.value) + '\n')
    const lines = computed(() => props.value.split('\n').length)
    return () => <div class="ks-toml-editor">
      <div class="ks-editor-mirror" aria-hidden="true">
        <pre style={{ transform: `translate(${-scroll.value.x}px, ${-scroll.value.y}px)` }}><code innerHTML={highlighted.value} /></pre>
        <div class="ks-editor-gutter" style={{ transform: `translateY(${-scroll.value.y}px)` }}>{Array.from({ length: lines.value }, (_, index) => <span>{index + 1}</span>)}</div>
      </div>
      <textarea aria-label={props.label} placeholder={props.placeholder} spellcheck={false} autocapitalize="off" autocomplete="off" wrap="off"
        readonly={props.readonly} disabled={props.disabled} value={props.value}
        onInput={event => emit('change', (event.target as HTMLTextAreaElement).value)}
        onScroll={event => { const input = event.target as HTMLTextAreaElement; scroll.value = { x: input.scrollLeft, y: input.scrollTop } }} />
    </div>
  },
})
