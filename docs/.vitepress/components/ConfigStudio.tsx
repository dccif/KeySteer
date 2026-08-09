import { computed, defineComponent, onBeforeUnmount, onMounted, reactive, ref } from 'vue'
import { withBase } from 'vitepress'
import { stringify } from 'smol-toml'
import {
  MOVEMENT_ACTIONS,
  applyModeAction,
  createSimulatorState,
  movePointer,
  toggleButton,
} from '../simulator/state'
import { effectiveBindings, resolveBinding } from '../simulator/bindings'
import CommonConfigControls from '../config-studio/CommonConfigControls'
import ModeStyleControls from '../config-studio/ModeStyleControls'
import {
  cloneConfigDocument,
  parseConfigDocument,
  resolveConfigDocument,
  type ConfigDocument,
} from '../config-studio/document'

type EditorMode = 'hotkeys' | 'normal' | 'grid' | 'recursive_grid' | 'ui_hint'
type Modifier = 'primary' | 'shift' | 'alt'
type Appearance = 'dark' | 'light'

interface KeySpec {
  key: string
  label: string
  width?: number
}

interface GridKeySpec extends KeySpec {
  column: number
  row: number
  columnSpan?: number
  rowSpan?: number
}

interface ActionGroup {
  name: string
  actions: Array<{ value: string; label: string }>
}

interface KeyBindingInfo {
  text: string
  tone: string
}

const modes: Array<{ id: EditorMode; label: string }> = [
  { id: 'hotkeys', label: '全局启动键' },
  { id: 'normal', label: 'Normal' },
  { id: 'grid', label: 'Grid' },
  { id: 'recursive_grid', label: 'Recursive Grid' },
  { id: 'ui_hint', label: 'UI Hint' },
]

const actionGroups: ActionGroup[] = [
  {
    name: '移动',
    actions: [
      ['move_left', '向左移动'], ['move_down', '向下移动'],
      ['move_up', '向上移动'], ['move_right', '向右移动'],
      ['precision', '精确速度'], ['slow', '慢速'], ['fast', '快速'],
    ].map(([value, label]) => ({ value, label })),
  },
  {
    name: '点击',
    actions: [
      ['left_click', '左键点击'], ['right_click', '右键点击'],
      ['middle_click', '中键点击'], ['double_click', '双击'],
    ].map(([value, label]) => ({ value, label })),
  },
  {
    name: '按键状态',
    actions: [
      ['toggle', '切换按下状态'],
    ].map(([value, label]) => ({ value, label })),
  },
  {
    name: '滚动',
    actions: [
      ['wheel_up', '向上滚动'], ['wheel_down', '向下滚动'],
      ['scroll_left', '向左滚动'], ['scroll_right', '向右滚动'],
      ['scroll_half_up', '向上半页'], ['scroll_half_down', '向下半页'],
    ].map(([value, label]) => ({ value, label })),
  },
  {
    name: '模式',
    actions: [
      ['normal', 'Normal'], ['grid', 'Grid'],
      ['recursive_grid', 'Recursive Grid'], ['ui_hint', 'UI Hint'],
      ['idle', 'Idle'],
    ].map(([value, label]) => ({ value, label })),
  },
  {
    name: '生命周期',
    actions: [
      ['finish', '完成当前筛选'], ['restart_mode', '重新开始当前模式'],
      ['escape', '返回'], ['none', '禁用此键'],
    ].map(([value, label]) => ({ value, label })),
  },
]

const functionRow: KeySpec[] = [
  key('esc', 'Esc'), gap(), key('f1', 'F1'), key('f2', 'F2'), key('f3', 'F3'), key('f4', 'F4'),
  gap(), key('f5', 'F5'), key('f6', 'F6'), key('f7', 'F7'), key('f8', 'F8'), gap(),
  key('f9', 'F9'), key('f10', 'F10'), key('f11', 'F11'), key('f12', 'F12'), gap(),
  key('print_screen', 'PrtSc'), key('scroll_lock', 'Scroll'), key('pause', 'Pause'),
]

const mainRows: KeySpec[][] = [
  [
    key('`', '`'), ...digits(), key('-', '-'), key('=', '='), key('backspace', 'Backspace', 2),
  ],
  [
    key('tab', 'Tab', 1.5), ...letters('qwertyuiop'), key('[', '['), key(']', ']'), key('\\', '\\', 1.5),
  ],
  [
    key('caps_lock', 'Caps', 1.75), ...letters('asdfghjkl'), key(';', ';'), key("'", "'"), key('enter', 'Enter', 2.25),
  ],
  [
    key('left_shift', 'Shift', 2.25), ...letters('zxcvbnm'), key(',', ','), key('.', '.'), key('/', '/'), key('right_shift', 'Shift', 2.75),
  ],
  [
    key('left_ctrl', 'Ctrl', 1.25), key('left_cmd', 'Primary', 1.25), key('left_alt', 'Alt', 1.25),
    key('space', 'Space', 6.25), key('right_alt', 'Alt', 1.25), key('right_cmd', 'Primary', 1.25),
    key('menu', 'Menu', 1.25), key('right_ctrl', 'Ctrl', 1.25),
  ],
]

const navigationRows: KeySpec[][] = [
  [key('insert', 'Ins'), key('home', 'Home'), key('page_up', 'PgUp')],
  [key('delete', 'Del'), key('end', 'End'), key('page_down', 'PgDn')],
  [gap(1), gap(1), gap(1)],
  [gap(1), key('up', '↑'), gap(1)],
  [key('left', '←'), key('down', '↓'), key('right', '→')],
]

const numpadKeys: GridKeySpec[] = [
  gridKey('num_lock', 'Num', 1, 1), gridKey('numpad_divide', '/', 2, 1), gridKey('numpad_multiply', '*', 3, 1), gridKey('numpad_subtract', '−', 4, 1),
  gridKey('numpad_7', '7', 1, 2), gridKey('numpad_8', '8', 2, 2), gridKey('numpad_9', '9', 3, 2), gridKey('numpad_add', '+', 4, 2, 1, 2),
  gridKey('numpad_4', '4', 1, 3), gridKey('numpad_5', '5', 2, 3), gridKey('numpad_6', '6', 3, 3),
  gridKey('numpad_1', '1', 1, 4), gridKey('numpad_2', '2', 2, 4), gridKey('numpad_3', '3', 3, 4), gridKey('numpad_enter', 'Enter', 4, 4, 1, 2),
  gridKey('numpad_0', '0', 1, 5, 2), gridKey('numpad_decimal', '.', 3, 5),
]

export default defineComponent({
  name: 'ConfigStudio',
  setup() {
    const document = ref<ConfigDocument | null>(null)
    const defaultDocument = ref<ConfigDocument | null>(null)
    const defaultSource = ref('')
    const sourceName = ref('generated/keysteer.default.toml')
    const sourceStats = ref({ bytes: 0, sections: 0, values: 0 })
    const activeMode = ref<EditorMode>('normal')
    const appearance = ref<Appearance>('light')
    const modifiers = reactive<Record<Modifier, boolean>>({ primary: false, shift: false, alt: false })
    const selectedChord = ref('')
    const customAction = ref('')
    const message = ref('正在载入默认配置…')
    const importInput = ref<HTMLInputElement | null>(null)
    const simulator = reactive(createSimulatorState())
    const screen = ref<HTMLElement | null>(null)
    const simulatorArmed = ref(false)
    const heldActions = new Set<string>()
    const clickPulse = ref(0)
    const scrollPulse = ref('')
    const isMac = ref(false)
    let animationFrame = 0
    let previousFrame = 0

    const effectiveDocument = computed<ConfigDocument | null>(() => {
      if (!document.value || !defaultDocument.value) return document.value
      return resolveConfigDocument(defaultDocument.value, document.value)
    })

    const tomlPreview = computed(() => {
      if (!document.value) return ''
      try {
        return stringify(document.value)
      } catch (error) {
        return `# 无法生成 TOML：${formatError(error)}`
      }
    })

    const selectedAction = computed(() => {
      if (!effectiveDocument.value || !selectedChord.value) return ''
      const value = resolveBinding(effectiveDocument.value, activeMode.value, selectedChord.value)?.value
      return Array.isArray(value) ? value.join(' → ') : String(value ?? '')
    })

    const targetingVisual = computed(() => targetingAppearance(effectiveDocument.value, simulator.mode, appearance.value))
    const targetingSettings = computed(() => effectiveDocument.value?.[simulator.mode] ?? {})
    const editorVisual = computed(() => editorAppearance(effectiveDocument.value, appearance.value))

    async function loadDefault(): Promise<void> {
      try {
        const response = await fetch(withBase('/generated/keysteer.default.toml'))
        if (!response.ok) throw new Error(`HTTP ${response.status}`)
        defaultSource.value = await response.text()
        const parsed = parseConfigDocument(defaultSource.value)
        defaultDocument.value = parsed.document
        document.value = cloneConfigDocument(parsed.document)
        sourceName.value = 'generated/keysteer.default.toml'
        sourceStats.value = { bytes: parsed.bytes, sections: parsed.sections, values: parsed.values }
        message.value = '已载入默认配置'
      } catch (error) {
        message.value = `无法载入默认配置：${formatError(error)}`
      }
    }

    function selectKey(spec: KeySpec): void {
      if (!spec.key) return
      const parts: string[] = (Object.keys(modifiers) as Modifier[]).filter((modifier) => modifiers[modifier])
      if (!parts.includes(spec.key)) parts.push(spec.key)
      selectedChord.value = parts.join('+')
      customAction.value = selectedAction.value
    }

    function setAction(action: string): void {
      if (!document.value || !effectiveDocument.value || !selectedChord.value) return
      const table = bindingTable(document.value, activeMode.value, true, effectiveDocument.value)
      expandConfiguredBinding(table, selectedChord.value)
      table[selectedChord.value] = action
      document.value = { ...document.value }
      customAction.value = action
      message.value = `${selectedChord.value} → ${action}`
    }

    function removeBinding(): void {
      if (!document.value || !effectiveDocument.value || !selectedChord.value) return
      const table = bindingTable(document.value, activeMode.value, true, effectiveDocument.value)
      expandConfiguredBinding(table, selectedChord.value)
      delete table[selectedChord.value]
      document.value = { ...document.value }
      customAction.value = ''
      message.value = `已移除 ${selectedChord.value}`
    }

    function keyBindingInfo(spec: KeySpec): KeyBindingInfo | undefined {
      if (!effectiveDocument.value || !spec.key) return undefined
      const entries = [...effectiveBindings(effectiveDocument.value, activeMode.value)]
        .filter(([chord]) => chord === spec.key || chord.split('+').at(-1) === spec.key)
      if (entries.length === 0) return undefined
      const [chord, binding] = entries[0]
      const action = Array.isArray(binding.value) ? String(binding.value[0] ?? '') : String(binding.value)
      const sourcePrefix = binding.source === activeMode.value ? '' : `${shortMode(binding.source)} `
      const prefix = chord === spec.key ? sourcePrefix : `${sourcePrefix}${shortChord(chord)} `
      return { text: `${prefix}${shortAction(action)}`, tone: actionTone(action) }
    }

    function onImport(event: Event): void {
      const input = event.target as HTMLInputElement
      const file = input.files?.[0]
      if (!file) return
      file.text().then((source) => {
        try {
          const parsed = parseConfigDocument(source)
          document.value = parsed.document
          sourceName.value = file.name
          sourceStats.value = { bytes: parsed.bytes, sections: parsed.sections, values: parsed.values }
          message.value = `已导入 ${file.name}；配置值已解析，原注释不会写入生成文件`
        } catch (error) {
          message.value = `TOML 解析失败：${formatError(error)}`
        } finally {
          input.value = ''
        }
      })
    }

    function downloadConfig(): void {
      downloadText(tomlPreview.value, 'keysteer.user.toml')
      message.value = '已生成 keysteer.user.toml'
    }

    function downloadDefault(): void {
      if (defaultSource.value) downloadText(defaultSource.value, 'keysteer.default.toml')
    }

    function copyToml(): void {
      void navigator.clipboard.writeText(tomlPreview.value)
      message.value = 'TOML 已复制到剪贴板'
    }

    function resolveAction(chord: string): string[] {
      if (!effectiveDocument.value) return []
      const lookupMode = simulator.mode === 'idle' ? 'hotkeys' : simulator.mode
      const binding = resolveBinding(effectiveDocument.value, lookupMode, chord)?.value
      if (Array.isArray(binding)) return binding.map(String)
      return typeof binding === 'string' ? [binding] : []
    }

    function executeAction(action: string, continuous = false): void {
      if (MOVEMENT_ACTIONS.has(action)) {
        movePointer(simulator, action, continuous ? 0.45 : 2.5)
        return
      }
      if (applyModeAction(simulator, action)) return
      if (action === 'left_click' || action === 'double_click') {
        clickPulse.value += 1
        simulator.lastEvent = action === 'double_click' ? '双击' : '左键点击'
        return
      }
      if (action === 'right_click' || action === 'middle_click') {
        clickPulse.value += 1
        simulator.lastEvent = action === 'right_click' ? '右键点击' : '中键点击'
        return
      }
      if (action === 'toggle') {
        toggleButton(simulator, 'left')
        return
      }
      if (action.includes('wheel') || action.startsWith('scroll_')) {
        scrollPulse.value = action
        simulator.lastEvent = action
        window.setTimeout(() => { scrollPulse.value = '' }, 280)
        return
      }
      simulator.lastEvent = `${action}（首版暂不模拟）`
    }

    function onSimulatorKeyDown(event: KeyboardEvent): void {
      if (!simulatorArmed.value || event.repeat) return
      if (event.key === 'Escape') {
        simulatorArmed.value = false
        heldActions.clear()
        return
      }
      if (handleTargetingKey(browserKeyName(event.key, event.code))) {
        event.preventDefault()
        return
      }
      const chord = chordFromEvent(event, isMac.value)
      if (!chord) return
      const actions = resolveAction(chord)
      if (actions.length === 0) return
      event.preventDefault()
      actions.forEach((action) => {
        if (MOVEMENT_ACTIONS.has(action)) heldActions.add(action)
        else executeAction(action)
      })
    }

    function onSimulatorKeyUp(event: KeyboardEvent): void {
      const chord = chordFromEvent(event, isMac.value)
      if (!chord) {
        heldActions.clear()
        return
      }
      resolveAction(chord).forEach((action) => heldActions.delete(action))
    }

    function animate(timestamp: number): void {
      const delta = previousFrame ? Math.min(32, timestamp - previousFrame) : 16
      previousFrame = timestamp
      heldActions.forEach((action) => movePointer(simulator, action, delta * 0.028))
      animationFrame = requestAnimationFrame(animate)
    }

    function handleTargetingKey(keyName: string): boolean {
      if (simulator.mode !== 'grid' && simulator.mode !== 'recursive_grid') return false
      const settings = effectiveDocument.value?.[simulator.mode]
      if (!settings) return false
      const path = simulator.mode === 'grid'
        ? simulator.targeting.grid.path
        : simulator.targeting.recursiveGrid.path
      if (keyName === 'backspace') {
        if (path.length > 0) path.pop()
        simulator.lastEvent = '返回上一层网格'
        return true
      }
      const keys = String(settings.keys ?? '')
      if (!keys.includes(keyName)) return false
      const maxDepth = numberSetting(settings.max_depth, simulator.mode === 'grid' ? 3 : 10)
      if (path.length < maxDepth) path.push(keyName)
      simulator.lastEvent = `${simulator.mode}：${path.join(' → ')}`
      return true
    }

    function setPreviewMode(mode: 'normal' | 'grid' | 'recursive_grid' | 'ui_hint'): void {
      simulator.mode = mode
      simulator.lastEvent = `预览 ${mode}`
    }

    onMounted(() => {
      isMac.value = /Mac|iPhone|iPad/.test(navigator.platform)
      void loadDefault()
      animationFrame = requestAnimationFrame(animate)
    })
    onBeforeUnmount(() => {
      cancelAnimationFrame(animationFrame)
    })

    const keyboard = () => (
      <div class="ks-keyboard-scroll" aria-label="ANSI 104 键盘">
        <div class="ks-keyboard">
          <KeyboardRows rows={[functionRow]} selected={selectedChord.value} onKey={selectKey} bindingInfo={keyBindingInfo} />
          <div class="ks-keyboard-body">
            <KeyboardRows rows={mainRows} selected={selectedChord.value} onKey={selectKey} bindingInfo={keyBindingInfo} />
            <KeyboardRows rows={navigationRows} selected={selectedChord.value} onKey={selectKey} bindingInfo={keyBindingInfo} compact />
            <Numpad keys={numpadKeys} selected={selectedChord.value} onKey={selectKey} bindingInfo={keyBindingInfo} />
          </div>
        </div>
      </div>
    )

    return () => (
      <div class="ks-studio">
        <section class="ks-card ks-keyboard-card" style={editorVisual.value as any}>
          <div class="ks-toolbar ks-compact-toolbar">
            <div>
              <h2>键位编辑器</h2>
              <p>{message.value}</p>
            </div>
          </div>
          <div class="ks-keyboard-tools">
            <div class="ks-mode-tabs" role="tablist">
              {modes.map((mode) => (
                <button class={{ active: activeMode.value === mode.id }} onClick={() => { activeMode.value = mode.id; selectedChord.value = '' }}>
                  {mode.label}
                </button>
              ))}
            </div>
            <div class="ks-modifiers">
              <span>组合键</span>
              {(Object.keys(modifiers) as Modifier[]).map((modifier) => (
                <button class={{ active: modifiers[modifier] }} aria-pressed={modifiers[modifier]} onClick={() => { modifiers[modifier] = !modifiers[modifier] }}>
                  {modifier === 'primary' ? 'Primary' : modifier === 'shift' ? 'Shift' : 'Alt'}
                </button>
              ))}
              <code>{selectedChord.value || '点击一个键查看绑定'}</code>
            </div>
          </div>
          {keyboard()}
          <div class="ks-key-legend" aria-label="按键颜色分类">
            <span class="tone-move"><i />方向移动</span>
            <span class="tone-click"><i />鼠标点击</span>
            <span class="tone-speed"><i />速度控制</span>
            <span class="tone-state"><i />按键状态</span>
            <span class="tone-scroll"><i />滚动</span>
            <span class="tone-mode"><i />模式切换</span>
            <span class="tone-utility"><i />其他</span>
          </div>
          <details class="ks-binding-details" open={Boolean(selectedChord.value)}>
            <summary>
              <span>{selectedChord.value || '选择键位后编辑绑定'}</span>
              <code>{selectedAction.value || '未绑定'}</code>
            </summary>
            <div class="ks-binding-panel">
              <div class="ks-action-groups">
                {actionGroups.map((group) => (
                  <div class="ks-action-group">
                    <strong>{group.name}</strong>
                    <div>{group.actions.map((action) => (
                      <button class={{ active: selectedAction.value === action.value }} disabled={!selectedChord.value} onClick={() => setAction(action.value)}>{action.label}</button>
                    ))}</div>
                  </div>
                ))}
              </div>
              <div class="ks-custom-action">
                <input value={customAction.value} placeholder="自定义动作，例如 press shift" onInput={(event) => { customAction.value = (event.target as HTMLInputElement).value }} onKeydown={(event) => { if (event.key === 'Enter') setAction(customAction.value.trim()) }} />
                <button disabled={!selectedChord.value || !customAction.value.trim()} onClick={() => setAction(customAction.value.trim())}>应用</button>
                <button disabled={!selectedChord.value} onClick={removeBinding}>移除</button>
              </div>
            </div>
          </details>
        </section>

        <div class="ks-studio-workbench">
          <div class="ks-visual-column">
            <section class="ks-card ks-simulator-card">
              <div class="ks-toolbar ks-compact-toolbar">
                <div><h2>样式预览</h2><p>切换模式后调整颜色、网格和字体。</p></div>
                <span class={{ 'ks-status': true, armed: simulatorArmed.value }}>{simulatorArmed.value ? '键盘已捕获' : '点击预览可试按键'}</span>
              </div>
              <div class="ks-simulator-modes" aria-label="预览模式">
                {(['normal', 'grid', 'recursive_grid', 'ui_hint'] as const).map((mode) => (
                  <button class={{ active: simulator.mode === mode }} onClick={() => setPreviewMode(mode)}>{mode}</button>
                ))}
              </div>
              <div
                ref={screen}
                class={{ 'ks-screen': true, armed: simulatorArmed.value, [`mode-${simulator.mode}`]: true }}
                style={targetingVisual.value as any}
                tabindex="0"
                onFocus={() => { simulatorArmed.value = true }}
                onBlur={() => { simulatorArmed.value = false; heldActions.clear() }}
                onKeydown={onSimulatorKeyDown}
                onKeyup={onSimulatorKeyUp}
              >
                <div class="ks-screen-grid" />
                <DesktopBackdrop />
                <div class="ks-mode-badge">{simulator.mode}</div>
                {(simulator.mode === 'grid' || simulator.mode === 'recursive_grid') && <div class="ks-target-backdrop" />}
                {simulator.mode === 'grid' && <TargetGrid mode="grid" settings={targetingSettings.value} path={simulator.targeting.grid.path} />}
                {simulator.mode === 'recursive_grid' && <TargetGrid mode="recursive_grid" settings={targetingSettings.value} path={simulator.targeting.recursiveGrid.path} />}
                {simulator.mode === 'ui_hint' && <HintOverlay settings={targetingSettings.value} />}
                <div class={{ 'ks-pointer': true, pressed: simulator.pressedButtons.has('left') }} style={{ left: `${simulator.pointer.x}%`, top: `${simulator.pointer.y}%` }}>
                  <span key={clickPulse.value} class={clickPulse.value ? 'pulse' : ''} />
                </div>
                {scrollPulse.value && <div class="ks-scroll-pulse">{scrollPulse.value}</div>}
                <div class="ks-event-log">{simulator.lastEvent}</div>
              </div>
              {document.value && effectiveDocument.value && (simulator.mode === 'grid' || simulator.mode === 'recursive_grid' || simulator.mode === 'ui_hint') && (
                <ModeStyleControls
                  document={document.value}
                  effectiveDocument={effectiveDocument.value}
                  mode={simulator.mode}
                  appearance={appearance.value}
                  onChange={(next) => { document.value = next }}
                  onAppearanceChange={(next) => { appearance.value = next }}
                />
              )}
              {(simulator.mode === 'normal' || simulator.mode === 'idle') && <p class="ks-normal-note">Normal 没有网格覆盖层，请选择 Grid、Recursive Grid 或 UI Hint 调整样式。</p>}
            </section>
          </div>

          <section class="ks-card ks-preview-card">
            <div class="ks-toolbar ks-compact-toolbar ks-preview-toolbar">
              <div class="ks-toml-source">
                <span class="ks-source-kicker">配置源</span>
                <h2>{sourceName.value}</h2>
                <p>{message.value}</p>
                <div class="ks-source-metrics">
                  <span>{sourceStats.value.sections} 个顶层配置段</span>
                  <span>{sourceStats.value.values} 个值</span>
                  <span>{formatBytes(sourceStats.value.bytes)}</span>
                </div>
              </div>
              <div class="ks-source-actions">
                <button class="ks-button" onClick={() => importInput.value?.click()}>导入 TOML</button>
                <button class="ks-button" onClick={downloadDefault}>默认配置</button>
                <button class="ks-button ks-button-primary" onClick={downloadConfig}>下载用户配置</button>
                <button class="ks-button" onClick={copyToml}>复制</button>
                <input ref={importInput} class="ks-file-input" type="file" accept=".toml,text/plain" onChange={onImport} />
              </div>
            </div>
            <div class="ks-toml-sync-note">
              <strong>与程序默认配置同步</strong>
              <p>页面构建前会从仓库根目录复制 <code>keysteer.default.toml</code>。导入局部配置时，预览按 Rust 缺省规则补全，下载仍保持局部文件。</p>
              <small>浏览器会验证 TOML 结构，但不会替代 <code>keysteer --check</code>；解析后注释不会保留。</small>
            </div>
            <details class="ks-toml-details">
              <summary>查看并检查生成的 TOML</summary>
              <pre class="ks-toml"><code innerHTML={highlightToml(tomlPreview.value)} /></pre>
            </details>
          </section>
        </div>
        {document.value && effectiveDocument.value && (
          <CommonConfigControls
            document={document.value}
            effectiveDocument={effectiveDocument.value}
            onChange={(next) => { document.value = next }}
          />
        )}
      </div>
    )
  },
})

const TargetGrid = defineComponent({
  props: {
    mode: { type: String as () => 'grid' | 'recursive_grid', required: true },
    settings: { type: Object as () => ConfigDocument, required: true },
    path: { type: Array as () => string[], required: true },
  },
  setup(props) {
    return () => {
      const cols = numberSetting(props.settings.grid_cols, props.mode === 'grid' ? 5 : 3)
      const rows = numberSetting(props.settings.grid_rows, props.mode === 'grid' ? 4 : 3)
      const keys = String(props.settings.keys ?? '')
      const cells = Array.from({ length: cols * rows }, (_, index) => keys[index] ?? '·')
      const active = props.path.at(-1)
      const labelChar = String(props.settings.ui?.label_char ?? '')
      const previewSecondLayer = props.mode === 'grid'
        && props.path.length === 0
        && numberSetting(props.settings.max_depth, 3) > 1
      return (
        <div
          class={{
            'ks-target-grid': true,
            recursive: props.mode === 'recursive_grid',
            'with-second-layer-preview': previewSecondLayer,
          }}
          style={{
            gridTemplateColumns: `repeat(${cols}, minmax(0, 1fr))`,
            gridTemplateRows: `repeat(${rows}, minmax(0, 1fr))`,
          }}
        >
          {cells.map((label, index) => {
            const cellClass = {
              selected: label === active,
              previous: props.path.slice(0, -1).includes(label),
              'label-background': Boolean(labelChar || props.settings.ui?.label_background),
              'grid-preview-cell': previewSecondLayer,
            }
            if (previewSecondLayer) {
              return (
                <div class={cellClass}>
                  <div
                    class="ks-grid-second-layer"
                    style={{
                      gridTemplateColumns: `repeat(${cols}, minmax(0, 1fr))`,
                      gridTemplateRows: `repeat(${rows}, minmax(0, 1fr))`,
                    }}
                  >
                    {cells.map((suffix) => <i>{suffix}</i>)}
                  </div>
                  <b>{label}</b>
                </div>
              )
            }
            return (
              <div class={cellClass}>
                <span>{labelChar || label}</span>
                {props.mode === 'recursive_grid' && props.settings.ui?.sub_key_preview && (
                  <em>{String(props.settings.keys ?? '').slice(0, 4)}</em>
                )}
                {props.mode === 'recursive_grid' && index === 0 && props.path.length > 0 && (
                  <small>{props.path.join(' → ')}</small>
                )}
              </div>
            )
          })}
        </div>
      )
    }
  },
})

const HintOverlay = defineComponent({
  props: {
    settings: { type: Object as () => ConfigDocument, required: true },
  },
  setup(props) {
    const positions = [
      [17, 15], [30, 15], [48, 15], [66, 15], [83, 15],
      [17, 30], [39, 31], [59, 31], [79, 31],
      [17, 48], [31, 49], [48, 49], [66, 49], [83, 49],
      [17, 67], [36, 68], [57, 68], [78, 68],
      [28, 84], [51, 84], [76, 84],
    ]
    return () => {
      const characters = String(props.settings.hint_characters ?? 'asdfghjkl')
      const placement = String(props.settings.placement ?? 'bottom')
      return (
        <div class={{ 'ks-hint-overlay': true, [`placement-${placement}`]: true }}>
          {positions.map(([x, y], index) => {
            const label = hintLabel(characters, index)
            return (
              <div class={{ target: true, boundary: Boolean(props.settings.boundary_highlight?.enabled) }} style={{ left: `${x}%`, top: `${y}%` }}>
                <span>{label}</span>
              </div>
            )
          })}
        </div>
      )
    }
  },
})

const DesktopBackdrop = defineComponent({
  name: 'DesktopBackdrop',
  setup() {
    return () => (
      <div class="ks-desktop-backdrop" aria-hidden="true">
        <div class="ks-desktop-window">
          <div class="ks-desktop-titlebar">
            <div class="ks-window-controls"><i /><i /><i /></div>
            <div class="ks-address-bar">工作台 / 今日概览</div>
            <div class="ks-avatar">KS</div>
          </div>
          <div class="ks-desktop-body">
            <aside>
              <strong>KeySteer</strong>
              <span class="active">概览</span><span>项目</span><span>日历</span><span>收件箱</span><span>设置</span>
            </aside>
            <main>
              <header><div><strong>上午好</strong><small>这里是今天需要处理的内容</small></div><button>新建项目</button></header>
              <div class="ks-desktop-search">搜索项目、任务或联系人…</div>
              <div class="ks-desktop-stats"><div><small>进行中</small><b>12</b></div><div><small>本周完成</small><b>28</b></div><div><small>待处理</small><b>7</b></div></div>
              <section class="ks-desktop-content">
                <div class="ks-desktop-list"><strong>最近任务</strong><p><i />完成发布说明 <button>打开</button></p><p><i />检查界面标注 <button>查看</button></p><p><i />整理下周计划 <button>编辑</button></p></div>
                <div class="ks-desktop-panel"><strong>进度</strong><div class="ks-progress-ring">72%</div><small>本周目标</small></div>
              </section>
            </main>
          </div>
        </div>
      </div>
    )
  },
})

const KeyboardRows = defineComponent({
  props: {
    rows: { type: Array as () => KeySpec[][], required: true },
    selected: { type: String, default: '' },
    onKey: { type: Function as unknown as () => (spec: KeySpec) => void, required: true },
    bindingInfo: { type: Function as unknown as () => (spec: KeySpec) => KeyBindingInfo | undefined, required: true },
    compact: { type: Boolean, default: false },
  },
  setup(props) {
    return () => (
      <div class={{ 'ks-key-section': true, compact: props.compact }}>
        {props.rows.map((row) => (
          <div class="ks-key-row">
            {row.map((spec) => spec.key ? (() => {
              const binding = props.bindingInfo(spec)
              return <button
                style={{ '--key-width': String(spec.width ?? 1) }}
                class={{
                  selected: props.selected.split('+').at(-1) === spec.key,
                  bound: Boolean(binding),
                  [`tone-${binding?.tone ?? 'none'}`]: Boolean(binding),
                }}
                title={binding ? `${spec.key}: ${binding.text}` : spec.key}
                onClick={() => props.onKey(spec)}
              >
                <span>{spec.label}</span>
                {binding && <small>{binding.text}</small>}
              </button>
            })() : (
              <span class="ks-key-gap" style={{ '--key-width': String(spec.width ?? 0.5) }} />
            ))}
          </div>
        ))}
      </div>
    )
  },
})

const Numpad = defineComponent({
  props: {
    keys: { type: Array as () => GridKeySpec[], required: true },
    selected: { type: String, default: '' },
    onKey: { type: Function as unknown as () => (spec: KeySpec) => void, required: true },
    bindingInfo: { type: Function as unknown as () => (spec: KeySpec) => KeyBindingInfo | undefined, required: true },
  },
  setup(props) {
    return () => (
      <div class="ks-numpad" aria-label="数字小键盘">
        {props.keys.map((spec) => {
          const binding = props.bindingInfo(spec)
          return (
            <button
              style={{
                gridColumn: `${spec.column} / span ${spec.columnSpan ?? 1}`,
                gridRow: `${spec.row} / span ${spec.rowSpan ?? 1}`,
              }}
              class={{
                selected: props.selected.split('+').at(-1) === spec.key,
                bound: Boolean(binding),
                [`tone-${binding?.tone ?? 'none'}`]: Boolean(binding),
              }}
              title={binding ? `${spec.key}: ${binding.text}` : spec.key}
              onClick={() => props.onKey(spec)}
            >
              <span>{spec.label}</span>
              {binding && <small>{binding.text}</small>}
            </button>
          )
        })}
      </div>
    )
  },
})

function key(keyName: string, label: string, width = 1): KeySpec {
  return { key: keyName, label, width }
}

function gap(width = 0.55): KeySpec {
  return { key: '', label: '', width }
}

function gridKey(keyName: string, label: string, column: number, row: number, columnSpan = 1, rowSpan = 1): GridKeySpec {
  return { key: keyName, label, column, row, columnSpan, rowSpan }
}

function letters(value: string): KeySpec[] {
  return [...value].map((letter) => key(letter, letter.toUpperCase()))
}

function digits(): KeySpec[] {
  return [...'1234567890'].map((digit) => key(digit, digit))
}

function hintLabel(characters: string, index: number): string {
  const keys = [...characters]
  if (keys.length === 0) return String(index + 1)
  if (index < keys.length) return keys[index]
  const offset = index - keys.length
  return `${keys[Math.floor(offset / keys.length) % keys.length]}${keys[offset % keys.length]}`
}

function bindingTable(
  document: ConfigDocument,
  mode: EditorMode,
  create = true,
  effectiveDocument?: ConfigDocument,
): Record<string, any> {
  if (mode === 'hotkeys') {
    if (create && !document.hotkeys) document.hotkeys = structuredClone(effectiveDocument?.hotkeys ?? {})
    return document.hotkeys ?? {}
  }
  if (create && !document[mode]) document[mode] = {}
  if (create && !document[mode].bindings) {
    document[mode].bindings = structuredClone(effectiveDocument?.[mode]?.bindings ?? {})
  }
  return document[mode]?.bindings ?? {}
}

function expandConfiguredBinding(table: Record<string, any>, chord: string): void {
  const configuredKey = Object.keys(table).find((key) => key.split(/\s+/).includes(chord))
  if (!configuredKey || configuredKey === chord) return
  const value = table[configuredKey]
  delete table[configuredKey]
  for (const key of configuredKey.split(/\s+/).filter(Boolean)) table[key] = value
}

function chordFromEvent(event: KeyboardEvent, isMac: boolean): string {
  const keyName = browserKeyName(event.key, event.code)
  if (!keyName || ['shift', 'ctrl', 'alt', 'cmd'].includes(keyName)) return ''
  const parts: string[] = []
  const primary = isMac ? event.metaKey : event.ctrlKey
  if (primary) parts.push('primary')
  if (event.shiftKey) parts.push('shift')
  if (event.altKey) parts.push('alt')
  if (isMac && event.ctrlKey) parts.push('ctrl')
  if (!isMac && event.metaKey) parts.push('cmd')
  parts.push(keyName)
  return parts.join('+')
}

function browserKeyName(value: string, code: string): string {
  const aliases: Record<string, string> = {
    ' ': 'space', Escape: 'esc', Enter: 'enter', Backspace: 'backspace', Tab: 'tab',
    Delete: 'delete', Insert: 'insert', ArrowUp: 'up', ArrowDown: 'down',
    ArrowLeft: 'left', ArrowRight: 'right', PageUp: 'page_up', PageDown: 'page_down',
    Home: 'home', End: 'end', Control: 'ctrl', Meta: 'cmd', Alt: 'alt', Shift: 'shift',
  }
  if (aliases[value]) return aliases[value]
  if (code.startsWith('Numpad') && /^\d$/.test(value)) return `numpad_${value}`
  return value.length === 1 ? value.toLowerCase() : value.toLowerCase()
}

function editorAppearance(document: ConfigDocument | null, appearance: Appearance): Record<string, string> {
  const theme = document?.theme?.[appearance] ?? {}
  return {
    '--ks-config-accent': colorSetting(theme.accent, appearance === 'dark' ? '#6E82D6FF' : '#465FBCFF'),
    '--ks-config-accent-alt': colorSetting(theme.accent_alt, appearance === 'dark' ? '#8FA2F0FF' : '#6477D4FF'),
    '--ks-config-surface': colorSetting(theme.surface, appearance === 'dark' ? '#0A1338FF' : '#EEF2FFFF'),
    '--ks-config-text': colorSetting(theme.text, appearance === 'dark' ? '#E8EEFFFF' : '#10172DFF'),
  }
}

function targetingAppearance(document: ConfigDocument | null, mode: string, appearance: Appearance): Record<string, string> {
  const theme = document?.theme?.[appearance] ?? {}
  const settings = document?.[mode] ?? {}
  const ui = settings.ui ?? {}
  const boundaries = settings.boundary_highlight ?? {}
  const themedColor = (value: unknown, fallback: string) => colorSetting(value, fallback, appearance)
  const surface = themedColor(theme.surface, appearance === 'dark' ? '#0A1338FF' : '#EEF2FFFF')
  const accent = themedColor(theme.accent, appearance === 'dark' ? '#6E82D6FF' : '#465FBCFF')
  const accentAlt = themedColor(theme.accent_alt, appearance === 'dark' ? '#8FA2F0FF' : '#6477D4FF')
  const text = themedColor(theme.text, appearance === 'dark' ? '#E8EEFFFF' : '#10172DFF')
  const configuredBackground = themedColor(ui.background_color, surface)
  const labelBackground = mode === 'ui_hint'
    ? themedColor(ui.background_color, translucent(surface, 95))
    : configuredBackground
  const borderOverride = ui.line_color ?? ui.border_color ?? ui.matched_border_color
  const configuredBorder = themedColor(borderOverride, accent)
  const border = borderOverride ? configuredBorder : translucent(accent, 60)
  return {
    '--ks-target-accent': border,
    '--ks-target-preview-border': translucent(configuredBorder, 35),
    '--ks-target-highlight': themedColor(ui.highlight_color ?? ui.matched_background_color, accentAlt),
    '--ks-target-surface': labelBackground,
    '--ks-target-grid-fill': translucent(configuredBackground, 55),
    '--ks-target-text': themedColor(ui.text_color, text),
    '--ks-target-matched-text': themedColor(ui.matched_text_color, accentAlt),
    '--ks-target-boundary': themedColor(boundaries.border_color, translucent(accent, 60)),
    '--ks-target-boundary-fill': themedColor(boundaries.background_color, 'transparent'),
    '--ks-target-line-width': `${numberSetting(ui.line_width ?? ui.border_width, 1)}px`,
    '--ks-target-boundary-width': `${numberSetting(boundaries.border_width, 1)}px`,
    '--ks-target-boundary-radius': `${autoSetting(boundaries.border_radius, 2)}px`,
    '--ks-target-font-size': `${numberSetting(ui.font_size, mode === 'ui_hint' ? 12 : 20)}px`,
    '--ks-target-font-family': String(ui.font_family || 'var(--vp-font-family-mono)'),
    '--ks-target-radius': `${autoSetting(ui.border_radius, numberSetting(ui.font_size, 12) * 0.5)}px`,
    '--ks-target-padding-x': `${autoSetting(ui.padding_x, numberSetting(ui.font_size, 12) * 0.5)}px`,
    '--ks-target-padding-y': `${autoSetting(ui.padding_y, numberSetting(ui.font_size, 12) * 0.34)}px`,
    '--ks-target-offset-x': `${finiteSetting(settings.label_x_offset, 0)}px`,
    '--ks-target-offset-y': `${finiteSetting(settings.label_y_offset, 0)}px`,
    '--ks-target-label-background': themedColor(ui.label_background_color, themedColor(theme.surface, surface)),
    '--ks-target-sub-key': themedColor(ui.sub_key_preview_text_color, themedColor(theme.accent_alt, accentAlt)),
    '--ks-target-sub-key-size': `${numberSetting(ui.sub_key_preview_font_size, 8)}px`,
  }
}

function translucent(color: string, opacity: number): string {
  return `color-mix(in srgb, ${color} ${opacity}%, transparent)`
}

function colorSetting(value: unknown, fallback: string, appearance: Appearance = 'light'): string {
  if (typeof value === 'string' && value) return value
  if (value && typeof value === 'object') {
    const variants = value as Record<string, unknown>
    if (typeof variants[appearance] === 'string') return variants[appearance] as string
    if (typeof variants.dark === 'string') return variants.dark
    if (typeof variants.light === 'string') return variants.light
  }
  return fallback
}

function numberSetting(value: unknown, fallback: number): number {
  return typeof value === 'number' && Number.isFinite(value) && value > 0 ? value : fallback
}

function finiteSetting(value: unknown, fallback: number): number {
  return typeof value === 'number' && Number.isFinite(value) ? value : fallback
}

function autoSetting(value: unknown, fallback: number): number {
  return typeof value === 'number' && Number.isFinite(value) && value >= 0 ? value : fallback
}

function actionTone(action: string): string {
  if (action.startsWith('move_')) return 'move'
  if (['precision', 'slow', 'fast'].includes(action)) return 'speed'
  if (action.includes('click')) return 'click'
  if (action === 'toggle' || action.startsWith('press') || action.startsWith('release')) return 'state'
  if (action.includes('scroll') || action.includes('wheel')) return 'scroll'
  if (['grid', 'recursive_grid', 'ui_hint', 'normal', 'idle'].includes(action)) return 'mode'
  return 'utility'
}

function shortAction(action: string): string {
  const names: Record<string, string> = {
    move_left: '← 移动', move_down: '↓ 移动', move_up: '↑ 移动', move_right: '→ 移动',
    wheel_up: '↑ 滚动', wheel_down: '↓ 滚动', scroll_left: '← 滚动', scroll_right: '→ 滚动',
    left_click: '左键', right_click: '右键', middle_click: '中键', double_click: '双击',
    grid: 'Grid', recursive_grid: '递归 Grid', ui_hint: 'UI Hint', normal: 'Normal', idle: 'Idle',
    precision: '精确', slow: '慢速', fast: '快速', finish: '完成', restart_mode: '重启', escape: '返回',
  }
  return names[action] ?? action.replace(/_/g, ' ')
}

function shortChord(chord: string): string {
  return chord.replace('primary', 'P').replace('shift', 'S').replace('alt', 'A').replace('+', '+')
}

function shortMode(mode: string): string {
  return mode === 'normal' ? 'N' : mode === 'hotkeys' ? 'H' : `${mode.slice(0, 1).toUpperCase()}:`
}

function highlightToml(source: string): string {
  return source.split('\n').map((raw, index) => {
    const line = raw.trim()
    let content: string
    if (!line) {
      content = '&nbsp;'
    } else if (line.startsWith('#')) {
      content = `<span class="toml-comment">${escapeHtml(raw)}</span>`
    } else if (line.startsWith('[')) {
      content = `<span class="toml-section">${escapeHtml(raw)}</span>`
    } else {
      const equal = raw.indexOf('=')
      if (equal < 0) {
        content = escapeHtml(raw)
      } else {
        const key = escapeHtml(raw.slice(0, equal))
        const value = raw.slice(equal + 1)
        const trimmed = value.trim()
        const kind = /^"/.test(trimmed) ? 'string'
          : /^(true|false)$/.test(trimmed) ? 'boolean'
            : /^[+-]?[\d.]+$/.test(trimmed) ? 'number' : 'value'
        content = `<span class="toml-key">${key}</span><span class="toml-equals">=</span><span class="toml-${kind}">${escapeHtml(value)}</span>`
      }
    }
    return `<span class="toml-line"><i>${index + 1}</i><span>${content}</span></span>`
  }).join('')
}

function escapeHtml(value: string): string {
  return value.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;')
}

function downloadText(source: string, fileName: string): void {
  const url = URL.createObjectURL(new Blob([source], { type: 'text/plain;charset=utf-8' }))
  const anchor = document.createElement('a')
  anchor.href = url
  anchor.download = fileName
  anchor.click()
  URL.revokeObjectURL(url)
}

function formatError(error: unknown): string {
  return error instanceof Error ? error.message : String(error)
}

function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`
  return `${(bytes / 1024).toFixed(1)} KiB`
}
