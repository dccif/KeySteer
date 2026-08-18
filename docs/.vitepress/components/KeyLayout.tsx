import { computed, defineComponent } from 'vue'
import { useData } from 'vitepress'

const tones = [
  ['move', '移动'],
  ['click', '点击'],
  ['speed', '速度'],
  ['scroll', '滚动'],
  ['state', '按住 / 拖拽'],
  ['navigation', '应用导航'],
  ['mode', '模式切换'],
  ['target', '定位键'],
] as const

type Tone = (typeof tones)[number][0]

export default defineComponent({
  name: 'KeyLayout',
  props: {
    layout: { type: String, required: true },
    label: { type: String, default: '键位布局' },
    hint: { type: String, default: '键盘左侧 · 从左到右、从上到下' },
    move: { type: String, default: '' },
    click: { type: String, default: '' },
    speed: { type: String, default: '' },
    scroll: { type: String, default: '' },
    state: { type: String, default: '' },
    navigation: { type: String, default: '' },
    mode: { type: String, default: '' },
    target: { type: String, default: '' },
  },
  setup(props) {
    const { lang } = useData()
    const toneLabel = (tone: Tone, chineseLabel: string) => {
      if (lang.value !== 'en-US') return chineseLabel
      return {
        move: 'Move', click: 'Click', speed: 'Speed', scroll: 'Scroll',
        state: 'Hold / drag', navigation: 'Application navigation', mode: 'Mode switch', target: 'Target keys',
      }[tone]
    }
    const rows = computed(() => props.layout
      .split(/[\/\n]/)
      .map((row) => row.trim())
      .filter(Boolean)
      .map((row) => row.includes(' ') ? row.split(/\s+/) : [...row]))
    const toneMap = computed(() => {
      const result = new Map<string, Tone>()
      for (const [tone] of tones) {
        for (const key of props[tone].split(/\s+/).filter(Boolean)) result.set(normalizeKey(key), tone)
      }
      return result
    })
    const activeTones = computed(() => tones.filter(([tone]) => props[tone].trim()))
    const wide = computed(() => Math.max(0, ...rows.value.map((row) => row.length)) > 6)

    return () => (
      <figure class={{ 'key-layout': true, wide: wide.value }} aria-label={props.label}>
        <figcaption>
          <span>{props.label}</span>
          <small>{props.hint}</small>
        </figcaption>
        <div class="key-layout-board">
          {rows.value.map((row, rowIndex) => (
            <div key={rowIndex} class="key-layout-row" style={{ '--key-row': rowIndex } as any}>
              {row.map((key) => {
                const tone = toneMap.value.get(normalizeKey(key))
                return <kbd key={key} class={tone ? `tone-${tone}` : undefined}>{key}</kbd>
              })}
            </div>
          ))}
        </div>
        {activeTones.value.length > 0 && (
          <ul class="key-layout-legend" aria-label={lang.value === 'en-US' ? 'Key colour legend' : '键位颜色图例'}>
            {activeTones.value.map(([tone, label]) => <li class={`tone-${tone}`}><i />{toneLabel(tone, label)}</li>)}
          </ul>
        )}
      </figure>
    )
  },
})

function normalizeKey(key: string): string {
  return key.trim().toLowerCase()
}
