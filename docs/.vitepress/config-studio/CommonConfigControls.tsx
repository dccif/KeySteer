import { computed, defineComponent } from 'vue'
import {
  cloneConfigDocument,
  deleteConfigPath,
  getConfigPath,
  setConfigPath,
  type ConfigDocument,
} from './document'

type FieldKind = 'number' | 'boolean' | 'select' | 'text'

interface ConfigField {
  path: string
  label: string
  description: string
  kind: FieldKind
  min?: number
  max?: number
  step?: number
  options?: Array<{ value: string; label: string }>
}

const frequentFields: ConfigField[] = [
  { path: 'pointer.initial_speed', label: '起始速度', description: '方向键刚按下时的像素/秒', kind: 'number', min: 0, max: 10000, step: 50 },
  { path: 'pointer.max_speed', label: '最高速度', description: '持续移动达到的像素/秒', kind: 'number', min: 0, max: 20000, step: 50 },
  { path: 'pointer.acceleration', label: '加速度', description: '每秒增加的速度；0 表示保持初速', kind: 'number', min: 0, max: 30000, step: 100 },
  { path: 'pointer.smooth_acceleration', label: '平滑加速', description: '开启 smootherstep S 曲线；关闭为线性', kind: 'boolean' },
  { path: 'normal.long_press_toggle_ms', label: '长按切换', description: '点击键长按多少毫秒后切换持续按下；0 为关闭', kind: 'number', min: 0, max: 5000, step: 50 },
  { path: 'grid.max_depth', label: 'Grid 层数', description: '确认目标前需要输入的网格层数', kind: 'number', min: 1, max: 20, step: 1 },
  { path: 'recursive_grid.max_depth', label: '递归上限', description: 'Recursive Grid 最大递归次数', kind: 'number', min: 1, max: 20, step: 1 },
  { path: 'ui_hint.strategy', label: 'UI 扫描', description: 'macOS 可使用视觉或辅助功能树；Windows 自动回退 UIA', kind: 'select', options: [
    { value: 'vision', label: 'Vision（默认）' },
    { value: 'hybrid', label: 'Hybrid' },
    { value: 'axtree', label: 'Accessibility Tree' },
  ] },
]

const advancedFields: ConfigField[] = [
  { path: 'pointer.tap_distance', label: '短按距离', description: '极短方向键操作仍移动的像素', kind: 'number', min: 0, max: 50, step: 0.1 },
  { path: 'pointer.precision_multiplier', label: 'Precision 倍率', description: 'precision 修饰键的速度倍率', kind: 'number', min: 0.01, max: 2, step: 0.01 },
  { path: 'pointer.slow_multiplier', label: 'Slow 倍率', description: 'slow 修饰键的速度倍率', kind: 'number', min: 0.01, max: 4, step: 0.05 },
  { path: 'pointer.fast_multiplier', label: 'Fast 倍率', description: 'fast 修饰键的速度倍率', kind: 'number', min: 0.1, max: 10, step: 0.1 },
  { path: 'scroll.scroll_step', label: '滚动步长', description: '普通滚动一次的像素距离', kind: 'number', min: 1, max: 5000, step: 10 },
  { path: 'scroll.scroll_step_half', label: '半页滚动', description: 'scroll_half_* 的像素距离', kind: 'number', min: 1, max: 50000, step: 50 },
  { path: 'grid.cursor_follow_selection', label: 'Grid 光标跟随', description: '每次选中后移到当前单元格中心', kind: 'boolean' },
  { path: 'recursive_grid.cursor_follow_selection', label: '递归光标跟随', description: '每次选中后移到当前单元格中心', kind: 'boolean' },
  { path: 'ui_hint.hint_characters', label: '提示字符', description: '用于生成 UI Hint 标签的字符集合', kind: 'text' },
  { path: 'ui_hint.placement', label: '标签位置', description: '标签相对目标元素的位置', kind: 'select', options: [
    { value: 'top', label: '上方' },
    { value: 'center', label: '居中' },
    { value: 'bottom', label: '下方' },
  ] },
]

export default defineComponent({
  name: 'CommonConfigControls',
  props: {
    document: { type: Object as () => ConfigDocument, required: true },
    effectiveDocument: { type: Object as () => ConfigDocument, required: true },
  },
  emits: {
    change: (_document: ConfigDocument) => true,
  },
  setup(props, { emit }) {
    const inheritedCount = computed(() => frequentFields
      .filter((field) => getConfigPath(props.document, field.path) === undefined).length)

    function update(path: string, value: unknown): void {
      const next = cloneConfigDocument(props.document)
      setConfigPath(next, path, value)
      emit('change', next)
    }

    function reset(path: string): void {
      const next = cloneConfigDocument(props.document)
      deleteConfigPath(next, path)
      emit('change', next)
    }

    const renderFields = (fields: ConfigField[]) => (
      <div class="ks-common-grid">
        {fields.map((field) => (
          <ConfigControl
            field={field}
            value={getConfigPath(props.effectiveDocument, field.path)}
            inherited={getConfigPath(props.document, field.path) === undefined}
            onUpdate={(value) => update(field.path, value)}
            onReset={() => reset(field.path)}
          />
        ))}
      </div>
    )

    return () => (
      <section class="ks-card ks-common-card">
        <div class="ks-toolbar ks-compact-toolbar">
          <div>
            <h2>常用设置</h2>
            <p>优先保留会频繁调整的移动、点击与定位参数；{inheritedCount.value} 项正在继承内置默认值。</p>
          </div>
        </div>
        <div class="ks-common-body">
          {renderFields(frequentFields)}
          <details class="ks-advanced-settings">
            <summary>速度倍率、滚动与定位细节</summary>
            <p>这些设置通常无需修改；展开后仍会写回同一份 TOML。</p>
            {renderFields(advancedFields)}
          </details>
        </div>
      </section>
    )
  },
})

const ConfigControl = defineComponent({
  props: {
    field: { type: Object as () => ConfigField, required: true },
    value: { required: false },
    inherited: { type: Boolean, required: true },
    onUpdate: { type: Function as unknown as () => (value: unknown) => void, required: true },
    onReset: { type: Function as unknown as () => () => void, required: true },
  },
  setup(props) {
    return () => {
      const field = props.field
      const control = field.kind === 'boolean' ? (
        <button type="button" class={{ 'ks-setting-toggle': true, active: Boolean(props.value) }} onClick={() => props.onUpdate(!props.value)}>
          <i />{props.value ? '开启' : '关闭'}
        </button>
      ) : field.kind === 'select' ? (
        <select value={String(props.value ?? '')} onChange={(event) => props.onUpdate((event.target as HTMLSelectElement).value)}>
          {field.options?.map((option) => <option value={option.value}>{option.label}</option>)}
        </select>
      ) : (
        <input
          type={field.kind === 'number' ? 'number' : 'text'}
          value={String(props.value ?? '')}
          min={field.min}
          max={field.max}
          step={field.step}
          onInput={(event) => props.onUpdate(field.kind === 'number'
            ? Number((event.target as HTMLInputElement).value)
            : (event.target as HTMLInputElement).value)}
        />
      )
      return (
        <label class="ks-common-control" title={field.path}>
          <span class="ks-common-copy">
            <strong>{field.label}</strong>
            <small>{field.description}</small>
          </span>
          <span class="ks-common-input">{control}</span>
          <button
            type="button"
            class={{ 'ks-inherit-button': true, inherited: props.inherited }}
            disabled={props.inherited}
            onClick={(event) => { event.preventDefault(); props.onReset() }}
          >
            {props.inherited ? '内置默认' : '恢复默认'}
          </button>
        </label>
      )
    }
  },
})
