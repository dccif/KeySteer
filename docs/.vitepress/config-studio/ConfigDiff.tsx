import { computed, defineComponent, onBeforeUnmount, onMounted, ref } from 'vue'
import { parse } from 'smol-toml'
import { diffToml } from '../simulator/config-diff'
import { useStudioI18n } from './i18n'

export default defineComponent({
  props: { before: { type: String, required: true }, after: { type: String, required: true }, beforeName: String, afterName: String, action: String },
  emits: ['close', 'confirm'],
  setup(props, { emit }) {
    const { t } = useStudioI18n()
    const dialog = ref<HTMLDialogElement>()
    const left = ref(props.before), right = ref(props.after)
    const leftName = ref(props.beforeName), rightName = ref(props.afterName)
    const error = ref(''), busy = ref(false)
    const result = computed(() => {
      try { return { changes: diffToml(left.value, right.value), error: '' } }
      catch (cause) { return { changes: [], error: String(cause) } }
    })
    const previousFocus = typeof document === 'undefined' ? null : document.activeElement as HTMLElement | null
    onMounted(() => dialog.value?.showModal())
    onBeforeUnmount(() => { dialog.value?.close(); previousFocus?.focus() })
    async function upload(event: Event, side: 'left' | 'right') {
      const input = event.target as HTMLInputElement, file = input.files?.[0]
      if (!file) return
      busy.value = true; error.value = ''
      try {
        if (file.size > 2 * 1024 * 1024) throw new Error(t('文件不能超过 2 MiB'))
        const source = await file.text()
        parse(source)
        if (side === 'left') { left.value = source; leftName.value = file.name }
        else { right.value = source; rightName.value = file.name }
      } catch (cause) { error.value = `${file.name}: ${String(cause)}` }
      finally { busy.value = false; input.value = '' }
    }
    return () => <dialog ref={dialog} class="ks-config-diff" aria-label={t('TOML 变更对比')} onCancel={event => { event.preventDefault(); emit('close') }}>
      <header><h2>{t('TOML 变更对比')}</h2><button class="ks-button" autofocus onClick={() => emit('close')}>{t('关闭对比')}</button></header>
      <p>{t('按配置值对比，忽略注释、空白和字段顺序；不补齐默认值。')}</p>
      {!props.action && <div class="ks-diff-tools"><button class="ks-button" disabled={busy.value} onClick={() => {
        left.value = props.before; right.value = props.after; leftName.value = props.beforeName; rightName.value = props.afterName; error.value = ''
      }}>{t('恢复导入与当前配置对比')}</button></div>}
      <div class="ks-diff-sources">{(['left', 'right'] as const).map(side => <label>
        <strong>{t(side === 'left' ? '变更前' : '变更后')}</strong><span>{side === 'left' ? leftName.value : rightName.value}</span>
        {!props.action && <input type="file" accept=".toml,text/plain" disabled={busy.value} aria-label={t(side === 'left' ? '上传变更前 TOML' : '上传变更后 TOML')} onChange={event => upload(event, side)} />}
      </label>)}</div>
      {(error.value || result.value.error) && <p role="alert">{error.value || result.value.error}</p>}
      <p role="status">{t('新增 {0} · 删除 {1} · 修改 {2}', ['added', 'removed', 'modified'].map(kind => result.value.changes.filter(change => change.kind === kind).length))}</p>
      <div class="ks-diff-scroll">
        {!result.value.error && !result.value.changes.length && <p>{t('配置值没有变化')}</p>}
        {result.value.changes.map(change => <article class={`ks-diff-change ${change.kind}`} key={change.path}>
          <h3><span>{t(({ added: '新增', removed: '删除', modified: '修改' })[change.kind])}</span> <code>{change.path}</code></h3>
          <div class="ks-diff-values"><pre class="ks-diff-before">{change.before === undefined ? '—' : `− ${change.before}`}</pre><pre class="ks-diff-after">{change.after === undefined ? '—' : `+ ${change.after}`}</pre></div>
        </article>)}
      </div>
      {props.action && <footer><button class="ks-button" onClick={() => emit('close')}>{t('取消')}</button><button class="ks-button ks-button-primary" disabled={!!result.value.error} onClick={() => emit('confirm')}>{t(props.action)}</button></footer>}
    </dialog>
  },
})
