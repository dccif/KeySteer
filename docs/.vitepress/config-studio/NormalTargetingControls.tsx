import { defineComponent } from 'vue'
import { useStudioI18n } from './i18n.ts'
import { cloneConfigDocument, type ConfigDocument } from './document.ts'
import { targetingConfig, targetingKeys, targetingLayout, targetingMethod, targetingSingleLevel } from '../simulator/normal-targeting.ts'
import { effectiveBindings } from '../simulator/bindings.ts'

const overrides = [
  ['grid_cols', '列数'], ['grid_rows', '行数'], ['keys', '定位键'], ['max_depth', '最大层数'],
  ['min_size_width', '最小宽度'], ['min_size_height', '最小高度'],
] as const

export default defineComponent({
  name: 'NormalTargetingControls',
  props: {
    document: { type: Object as () => ConfigDocument, required: true },
    effectiveDocument: { type: Object as () => ConfigDocument, required: true },
  },
  emits: { change: (_document: ConfigDocument) => true },
  setup(props, { emit }) {
    const { t } = useStudioI18n()
    function update(change: (settings: ConfigDocument) => void): void {
      const next = cloneConfigDocument(props.document)
      next.normal ??= {}
      next.normal.targeting ??= {}
      change(next.normal.targeting)
      emit('change', next)
    }
    function toggle(enabled: boolean): void {
      const next = cloneConfigDocument(props.document)
      next.normal ??= {}
      if (enabled) next.normal.targeting ??= {}
      else delete next.normal.targeting
      emit('change', next)
    }
    return () => {
      const configured = targetingConfig(props.document)
      const effective = targetingConfig(props.effectiveDocument)
      const method = targetingMethod(effective ?? {})
      const source = props.effectiveDocument[method] ?? {}
      const inheritedLayers = source.layers ?? []
      const layers = effective?.layers ?? inheritedLayers
      const keys = targetingKeys(props.effectiveDocument)
      const conflicts = [...effectiveBindings(props.effectiveDocument, 'normal')]
        .filter(([chord, binding]) => keys.has(chord) && binding.value !== 'none')
        .map(([chord, binding]) => `${chord} → ${binding.value}`)
      const resetOn: string[] = effective?.reset_on ?? ['move', 'click']
      const singleLevel = targetingSingleLevel(props.effectiveDocument)
      return <section class="ks-card ks-common-card ks-normal-targeting" data-config-path="normal.targeting">
        <div class="ks-toolbar ks-compact-toolbar"><div><h2>{t('Normal 盲操定位')}</h2>
          <p>{t('直接用网格键定位鼠标，不显示网格；细调仍使用 Normal 移动键。')}</p></div>
          <label class="ks-targeting-switch"><input type="checkbox" checked={Boolean(configured)} onChange={e => toggle((e.target as HTMLInputElement).checked)} />{t('启用')}</label>
        </div>
        {configured && <div class="ks-common-body">
          <div class="ks-common-grid">
            <label class="ks-common-control" data-config-path="normal.targeting.method"><span class="ks-common-copy"><strong>{t('定位方式')}</strong><small>{t('未覆盖的参数继承所选模式')}</small></span>
              <span class="ks-common-input"><select value={method} onChange={e => update(settings => { settings.method = (e.target as HTMLSelectElement).value })}><option value="grid">Grid</option><option value="recursive_grid">Recursive Grid</option></select></span></label>
            {overrides.filter(([name]) => method === 'recursive_grid' || !name.startsWith('min_size_')).map(([name, label]) => {
              const inherited = source[name]
              const value = configured[name]
              return <label class="ks-common-control" data-config-path={`normal.targeting.${name}`}><span class="ks-common-copy"><strong>{t(label)}</strong><small>{t('默认继承')} {method}.{name}: {String(inherited ?? '')}</small></span>
                <span class="ks-common-input"><input type={name === 'keys' ? 'text' : 'number'} min={name.startsWith('min_size_') ? 0 : 1} max={name === 'max_depth' ? 20 : undefined}
                  value={String(value ?? '')} placeholder={String(inherited ?? '')} onChange={e => {
                    const raw = (e.target as HTMLInputElement).value
                    if (raw && name !== 'keys' && (!Number.isInteger(Number(raw)) || Number(raw) < (name.startsWith('min_size_') ? 0 : 1))) return
                    update(settings => { if (raw) settings[name] = name === 'keys' ? raw : Number(raw); else delete settings[name] })
                  }} /></span></label>
            })}
          </div>
          {singleLevel ? <p>{t('单层只使用根层定位键，每次从整屏定位；Esc、Enter、Tab、Backspace、Space 保留 Normal 绑定，无需重置。')}</p> : <div class="ks-targeting-reset" data-config-path="normal.targeting.reset_on"><strong>{t('重新开始定位')}</strong>
            {(['move', 'click'] as const).map(reason => <label><input type="checkbox" checked={resetOn.includes(reason)} onChange={e => update(settings => {
              settings.reset_on = (e.target as HTMLInputElement).checked ? [...new Set([...resetOn, reason])] : resetOn.filter(item => item !== reason)
            })} />{reason === 'move' ? t('细调后') : t('点击后')}</label>)}
          </div>}
          {method === 'recursive_grid' && <details class="ks-advanced-settings"><summary>{t('按层覆盖')} ({Array.isArray(layers) ? layers.length : 0})</summary>
            <p>{t('不设置时继承 Recursive Grid 的层；启用覆盖后整组替换。')}</p>
            <button type="button" onClick={() => update(settings => { settings.layers = configured.layers === undefined ? cloneConfigDocument({ layers: inheritedLayers }).layers : undefined })}>{configured.layers === undefined ? t('覆盖层配置') : t('恢复继承层')}</button>
            {configured.layers !== undefined && <><button type="button" onClick={() => update(settings => { settings.layers = [...settings.layers, { depth: settings.layers.length }] })}>{t('添加层')}</button>
              {configured.layers.map((layer: ConfigDocument, index: number) => <div class="ks-targeting-layer" data-config-path={`normal.targeting.layers.${index}`}>
                <label>{t('深度')}<input type="number" min="0" max="20" value={layer.depth} onChange={e => update(settings => { settings.layers[index].depth = Number((e.target as HTMLInputElement).value) })} /></label>
                {(['grid_cols', 'grid_rows', 'keys'] as const).map(field => <label>{t(({ grid_cols: '列数', grid_rows: '行数', keys: '定位键' } as const)[field])}<input type={field === 'keys' ? 'text' : 'number'} min="1" value={layer[field] ?? ''} placeholder={String(targetingLayout(props.effectiveDocument, -1)?.[field] ?? '')} onChange={e => update(settings => { const raw = (e.target as HTMLInputElement).value; if (raw) settings.layers[index][field] = field === 'keys' ? raw : Number(raw); else delete settings.layers[index][field] })} /></label>)}
                <button type="button" onClick={() => update(settings => { settings.layers.splice(index, 1) })}>{t('移除')}</button>
              </div>)}</>}
          </details>}
          {conflicts.length > 0 && <p class="ks-targeting-conflict" role="status">{t('键位冲突：请在 Normal 按键页移除或改绑')} {conflicts.join('，')}。<code>keysteer --check</code> {t('会拒绝这些冲突。')}</p>}
        </div>}
      </section>
    }
  },
})
