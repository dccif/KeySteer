import { computed, defineComponent, ref, watch } from 'vue'
import { parse } from 'smol-toml'
import { diffToml } from '../simulator/config-diff'
import { useStudioI18n } from './i18n'
import TomlEditor from './TomlEditor'
import { highlightTomlCode } from './toml-highlight'

export default defineComponent({
  props: { before: { type: String, required: true }, after: { type: String, required: true }, beforeName: String, afterName: String, action: String },
  emits: ['close', 'confirm'],
  setup(props, { emit }) {
    const { t } = useStudioI18n()
    const left = ref(props.before), right = ref(props.after)
    const leftName = ref(props.beforeName), rightName = ref(props.afterName)
    const error = ref(''), busy = ref(false)
    const custom = ref(false)
    const fileInputs: Partial<Record<'left' | 'right', HTMLInputElement>> = {}
    function reset() {
      left.value = props.before; right.value = props.after
      leftName.value = props.beforeName; rightName.value = props.afterName
      error.value = ''; custom.value = false
    }
    watch(() => [props.before, props.after, props.action], (next, previous) => {
      if (!custom.value || props.action || next[2] !== previous[2]) reset()
    })
    const result = computed(() => {
      try { return { changes: diffToml(left.value, right.value), error: '' } }
      catch (cause) { return { changes: [], error: String(cause) } }
    })
    async function upload(event: Event, side: 'left' | 'right') {
      const input = event.target as HTMLInputElement, file = input.files?.[0]
      if (!file) return
      busy.value = true; error.value = ''
      try {
        if (file.size > 2 * 1024 * 1024) throw new Error(t('文件不能超过 2 MiB'))
        const source = await file.text()
        parse(source)
        custom.value = true
        if (side === 'left') { left.value = source; leftName.value = file.name }
        else { right.value = source; rightName.value = file.name }
      } catch (cause) { error.value = `${file.name}: ${String(cause)}` }
      finally { busy.value = false; input.value = '' }
    }
    return () => <section class="ks-config-diff" aria-label={t('TOML 变更对比')}>
      <div class="ks-diff-toolbar"><div><strong>{t('TOML 变更对比')}</strong><p>{t('粘贴或上传配置，实时查看字段变更。')}</p></div>
        {!props.action && <button class="ks-button" disabled={busy.value} onClick={reset} title={t('恢复导入与当前配置对比')}>↺ {t('恢复默认对比')}</button>}
      </div>
      {!result.value.error && <div class="ks-diff-summary" role="status">{(['added', 'removed', 'modified'] as const).map(kind => <span class={kind}><b>{({ added: '+', removed: '−', modified: '~' })[kind]}</b>{t(({ added: '新增', removed: '删除', modified: '修改' })[kind])}<strong>{result.value.changes.filter(change => change.kind === kind).length}</strong></span>)}<small>{t('忽略注释与格式差异')}</small></div>}
      {props.action && <footer><span>{t('正在预览待确认的配置；取消后可粘贴任意 TOML 对比。')}</span><button class="ks-button" onClick={() => emit('close')}>{t('取消')}</button><button class="ks-button ks-button-primary" disabled={!!result.value.error} onClick={() => emit('confirm')}>{t(props.action)}</button></footer>}
      <div class="ks-diff-sources">{(['left', 'right'] as const).map(side => <div class={`ks-diff-pane ${side}`}>
        <header><span class="ks-diff-side">{side === 'left' ? 'A' : 'B'}</span><div><strong>{t(side === 'left' ? '变更前' : '变更后')}</strong><small title={side === 'left' ? leftName.value : rightName.value}>{side === 'left' ? leftName.value : rightName.value}</small></div>
          {!props.action && <button class="ks-button" disabled={busy.value} onClick={() => fileInputs[side]?.click()}>{t('选择文件')}</button>}
          <input ref={element => { if (element) fileInputs[side] = element as HTMLInputElement }} hidden type="file" accept=".toml,text/plain" disabled={busy.value || !!props.action} aria-label={t(side === 'left' ? '上传变更前 TOML' : '上传变更后 TOML')} onChange={event => upload(event, side)} />
        </header>
        <TomlEditor label={t(side === 'left' ? '变更前 TOML' : '变更后 TOML')} placeholder={t('在此粘贴 TOML')} readonly={!!props.action} disabled={busy.value}
          value={side === 'left' ? left.value : right.value} onChange={source => {
            custom.value = true; error.value = ''
            if (side === 'left') { left.value = source; leftName.value = t('粘贴的配置') }
            else { right.value = source; rightName.value = t('粘贴的配置') }
          }} />
      </div>)}</div>
      <p class="ks-diff-note">{t('按配置值对比，忽略注释、空白和字段顺序；不补齐默认值。')}</p>
      {(error.value || result.value.error) && <p role="alert">{error.value || result.value.error}</p>}
      <div class="ks-diff-scroll">
        {!result.value.error && !result.value.changes.length && <div class="ks-diff-empty"><span>✓</span><div><strong>{t('配置值没有变化')}</strong><p>{t('两份配置的字段值一致，注释和排版差异不会计入变更。')}</p></div></div>}
        {result.value.changes.map(change => <article class={`ks-diff-change ${change.kind}`} key={change.path}>
          <h3><span>{t(({ added: '新增', removed: '删除', modified: '修改' })[change.kind])}</span> <code>{change.path}</code></h3>
          <div class="ks-diff-values"><pre class="ks-diff-before"><i>−</i><code innerHTML={highlightTomlCode(change.before ?? '—')} /></pre><pre class="ks-diff-after"><i>+</i><code innerHTML={highlightTomlCode(change.after ?? '—')} /></pre></div>
        </article>)}
      </div>
    </section>
  },
})
