import { fieldLocation, type SettingsTab } from './navigation'
import { defineComponent } from 'vue'
import {
  cloneConfigDocument,
  deleteConfigPath,
  getConfigPath,
  setConfigPath,
  type ConfigDocument,
} from './document'

import { frequentFields, advancedFields, type ConfigField } from './fields'
export default defineComponent({
  name: 'CommonConfigControls',
  props: {
    tab: { type: String as () => SettingsTab, required: true },
    page: { type: String, required: true },
    document: { type: Object as () => ConfigDocument, required: true },
    effectiveDocument: { type: Object as () => ConfigDocument, required: true },
  },
  emits: {
    change: (_document: ConfigDocument) => true,
  },
  setup(props, { emit }) {

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
        {fields.filter(field => fieldLocation(field.path).page === props.page && fieldLocation(field.path).tab === props.tab).map((field) => (
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
            <h2>{props.tab === 'appearance' ? '常用外观' : '常用参数'}</h2>
            <p>调整当前功能的行为；未修改的参数继续继承内置默认值。</p>
          </div>
        </div>
        <div class="ks-common-body">
          {renderFields(frequentFields)}
          <details class="ks-advanced-settings">
            <summary>高级设置</summary>
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
        <label class="ks-common-control" title={field.path} data-config-path={field.path}>
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
