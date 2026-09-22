import { useStudioI18n } from '../config-studio/i18n'
import SettingsNavigation from '../config-studio/SettingsNavigation'
import PaneDivider from '../config-studio/PaneDivider'
import FittedKeyboard from '../config-studio/FittedKeyboard'
import { gridRegion } from '../simulator/grid-region'
import { commonSearchFields, styleSearchFields } from '../config-studio/fields'
import { pages, tabLabels, searchSettings, utilitySearchFields, type SettingsTab, type SearchEntry } from '../config-studio/navigation'
import { cardPositionRatios, packedCardCenters } from '../simulator/window-card-position.ts'
import QuickSwitchPreview from '../config-studio/QuickSwitchPreview'
import { computed, defineComponent, nextTick, onBeforeUnmount, onMounted, reactive, ref, watch } from 'vue'
import { withBase } from 'vitepress'
import { stringify } from 'smol-toml'
import {
  MOVEMENT_ACTIONS,
  applyModeAction,
  applyKeyHelpAction,
  createSimulatorState,
  movePointer,
  toggleButton,
} from '../simulator/state'
import { effectiveBindings, resolveBinding, resolveLayeredPhysicalBinding, resolvePhysicalBinding, shortcutCaption, temporaryPhysicalKeys } from '../simulator/bindings'
import { applyWindowAction, chooseWindowNumber, switchWindowMode, isWindowMode, finishWindowNumber, hasWindowPresetChanges, replaceWindowPresets, restoreWindowPreset, saveWindowPreset, setDemoWindowCount, temporaryWindow, windowActionAvailable, windowDetail, windowInputStatus, windowSelectionKey, windowTarget, WINDOW_AREA, WINDOW_MOTION } from '../simulator/window'
import { decodeWorkspaceDocument, encodeWorkspaceFile, WORKSPACE_FILE_NAME, WORKSPACE_STORAGE_KEY, readSavedPresets, presetName, type ModeUsage } from '../simulator/window-presets'
import type { WindowState } from '../simulator/window'
import { availableWindowPresets } from '../simulator/window'
import { activateTab, chooseTabTarget, containingTab, activeTabWindow, tabFrame } from '../simulator/window-tabs'
import { layoutRect, quickRect, treeSlots } from '../simulator/window-layout'
import { quickRulerPlan } from '../simulator/window-ratios'
import { windowHelpSections, windowHelpGrid, windowHelpActionSupported, type HelpEntry } from '../simulator/window-help'
import { consumeConfigHandoff } from '../simulator/config-handoff'
import CommonConfigControls from '../config-studio/CommonConfigControls'
import ModeStyleControls from '../config-studio/ModeStyleControls'
import {
  cloneConfigDocument,
  parseConfigDocument,
  resolveConfigDocument,
  type ConfigDocument,
} from '../config-studio/document'

type EditorMode = 'hotkeys' | 'normal' | 'grid' | 'recursive_grid' | 'ui_hint' | 'window' | 'window_quick' | 'window_editor' | 'window_restore' | 'window_tab'
type Modifier = 'primary' | 'shift' | 'alt'
type Appearance = 'dark' | 'light'

interface KeySpec {
  key: string
  label: string
  width?: number
  shifted?: string
  literal?: boolean
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
  { id: 'window', label: 'Window' },
  ...(['window_quick', 'window_editor', 'window_restore', 'window_tab'] as const).map(id => ({ id, label: id })),
]

const actionGroups: ActionGroup[] = [
  { name: 'Tabs', actions: [
    ['window_tab', '标签组合模式'], ['window_tab_end', '结束本轮分组'], ['window_tab_group', '输入组编号'],
    ['window_number_end', '编号分隔符'], ['window_tab_remove', '移出活动成员'], ['window_tab_dissolve', '解散组合'],
    ['window_tab_next', '下一个标签'], ['window_tab_previous', '上一个标签'],
    ['window_tab_move_left', '标签左移'], ['window_tab_move_right', '标签右移'],
  ].map(([value, label]) => ({ value, label })) },
  { name: 'Window', actions: [
    ['window_left', '窗口向左'], ['window_down', '窗口向下'], ['window_up', '窗口向上'], ['window_right', '窗口向右'],
    ['window_size', '移动／缩放'], ['window_quick', '快速布局'], ['window_editor', '编辑布局树'], ['window_tile', '直接平铺'],
    ['window_screen_next', '循环切屏'],
    ['window_maximize', '最大化／还原'], ['window_minimize', '最小化／还原'], ['size_cycle', '旧三态循环'], ['window_center', '窗口居中'], ['window_close', '关闭窗口'], ['window_volume_down', '降低应用音量'], ['window_volume_up', '提高应用音量'], ['window_volume_mute', '切换应用静音'], ['window_audio_previous', '上一个应用输出设备'], ['window_audio_next', '下一个应用输出设备'], ['window_system_volume_down', '降低系统音量'], ['window_system_volume_up', '提高系统音量'], ['window_system_volume_mute', '切换系统静音'], ['window_system_audio_previous', '上一个系统输出设备'], ['window_system_audio_next', '下一个系统输出设备'], ['window_select', '切换下一个窗口'], ['window_select_previous', '切换上一个窗口'],
    ['window_undo', '撤销窗口调整'], ['window_redo', '重做窗口调整'], ['window_reset_initial', '恢复原始状态'], ['window_remove_region', '删除当前区域'], ['window_restore', '保存的布局'], ['window_save_layout', '保存布局'], ['window_confirm', '确认'],
  ].map(([value, label]) => ({ value, label })) },
  { name: '窗口布局方向', actions: ['left', 'down', 'up', 'right'].flatMap(direction => [
    { value: `window_layout_${direction}`, label: `布局方向 ${direction}` },
    { value: `window_split_${direction}`, label: `切分 ${direction}` },
    { value: `window_ratio_${direction}`, label: `移动分割线 ${direction}` },
  ]) },
  { name: '按键提示', actions: [
    { value: 'key_help', label: '切换按键提示' },
  ] },
  {
    name: '移动',
    actions: [
      ['move_left', '向左移动'], ['move_down', '向下移动'],
      ['move_up', '向上移动'], ['move_right', '向右移动'],
      ['precision', '精确速度'], ['slow', '慢速'], ['fast', '快速'],
      ['precision_toggle', '切换精确速度'], ['slow_toggle', '切换慢速'], ['fast_toggle', '切换快速'],
    ].map(([value, label]) => ({ value, label })),
  },
  {
    name: '窗口移动',
    actions: [
      ['window_overlap_next', '优先切换下一个相交窗口'], ['window_overlap_previous', '优先切换上一个相交窗口'],
      ['window_activate_next', '激活下一个窗口并居中鼠标'], ['window_activate_previous', '激活上一个窗口并居中鼠标'],
      ['move_window previous', '窗口移到上一屏'], ['move_window next', '窗口移到下一屏'],
    ].map(([value, label]) => ({ value, label })),
  },
  {
    name: '点击',
    actions: [
      ['left_click', '左键点击'], ['right_click', '右键点击'],
      ['middle_click', '中键点击'], ['double_click', '双击'],
      ['mouse_x1', '侧键 1 点击'], ['mouse_x2', '侧键 2 点击'],
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
      ['idle', 'Idle'], ['window', 'Window'], ['window_quick', 'Quick'], ['window_editor', 'Editor'], ['window_restore', 'Restore'], ['window_tab', 'Tabs'],
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
    const { t, locale } = useStudioI18n()
    const document = ref<ConfigDocument | null>(null)
    const defaultDocument = ref<ConfigDocument | null>(null)
    const defaultSource = ref('')
    const sourceName = ref('generated/keysteer.default.toml')
    const sourceStats = ref({ bytes: 0, sections: 0, values: 0 })
    const activeMode = ref<EditorMode>('normal')
    const pageId = ref('normal')
    const navigationWidth = ref(240)
    const settingsShare = ref(55)
    const selectedTab = ref<SettingsTab>('keys')
    const currentPage = computed(() => pages.find(page => page.id === pageId.value)!)
    const searchQuery = ref('')
    const mobileView = ref('settings')
    const expanded = ref(false)
    const settingsPane = ref<HTMLElement | null>(null)
    const previewPane = ref<HTMLElement | null>(null)
    let previewReturnFocus: HTMLElement | null = null
    const searchEntries = [...new Map([...commonSearchFields, ...styleSearchFields,
      ...pages.map(page => ({ path: page.mode ? `${page.mode}.bindings` : page.id, label: page.label, page: page.id, tab: page.tabs[0] ?? 'behavior' as SettingsTab })),
      ...utilitySearchFields,
    ].map(field => [field.path, field])).values()]
    const searchResults = computed(() => searchSettings(searchEntries, searchQuery.value))
    function releasePreview(): void {
      quickStarted = 0; quickVisible.value = false; simulatorArmed.value = false
      heldActions.clear(); heldCharacterActions.clear(); physicalKeys.clear(); temporaryEntryKeys.clear()
      simulator.window.gesture = false
      temporaryWindow(simulator.window, false)
    }
    watch(locale, () => releasePreview())
    function selectPage(id: string, tab?: SettingsTab): void {
      releasePreview()
      pageId.value = id
      selectedTab.value = tab ?? (currentPage.value.tabs.includes(selectedTab.value) ? selectedTab.value : currentPage.value.tabs[0] ?? 'behavior')
      const mode = currentPage.value.mode
      if (mode) activeMode.value = mode as EditorMode
      selectedChord.value = ''
      pendingAction.value = ''
      actionQuery.value = ''
      actionCategory.value = ''
      if (mode && mode !== 'hotkeys') setPreviewMode(mode as Exclude<EditorMode, 'hotkeys'>)
      if (id === 'window_card') setPreviewMode('window')
      mobileView.value = 'settings'
      nextTick(() => settingsPane.value?.scrollTo({ top: 0 }))
    }
    async function locateField(entry: SearchEntry): Promise<void> {
      selectPage(entry.page, entry.tab); searchQuery.value = ''
      await nextTick()
      const field = settingsPane.value?.querySelector<HTMLElement>(`[data-config-path="${entry.path}"]`)
      for (let node = field?.parentElement; node && node !== settingsPane.value; node = node.parentElement) {
        if (node instanceof HTMLDetailsElement) node.open = true
      }
      field?.scrollIntoView({ block: 'center', behavior: 'smooth' })
      ;(field?.querySelector<HTMLElement>('input,select,button') ?? settingsPane.value)?.focus({ preventScroll: true })
    }
    async function toggleExpanded(): Promise<void> {
      releasePreview()
      if (!expanded.value) previewReturnFocus = globalThis.document.activeElement as HTMLElement
      expanded.value = !expanded.value
      await nextTick()
      if (expanded.value) previewPane.value?.querySelector<HTMLElement>('.ks-simulator-card button')?.focus()
      else previewReturnFocus?.focus({ preventScroll: true })
    }
    function previewDialogKey(event: KeyboardEvent): void {
      if (!expanded.value) return
      if (event.key === 'Escape') { event.preventDefault(); event.stopPropagation(); void toggleExpanded(); return }
      if (event.key === 'Tab' && event.target !== screen.value) {
        const items = Array.from(previewPane.value?.querySelectorAll<HTMLElement>('button,input,select,[tabindex="0"]') ?? []).filter(item => item.offsetParent !== null)
        const first = items[0], last = items.at(-1)
        if (event.shiftKey && event.target === first) { event.preventDefault(); last?.focus() }
        else if (!event.shiftKey && event.target === last) { event.preventDefault(); first?.focus() }
      }
    }
    const appearance = ref<Appearance>('light')
    const modifiers = reactive<Record<Modifier, boolean>>({ primary: false, shift: false, alt: false })
    const selectedChord = ref('')
    const pendingAction = ref('')
    const draggedAction = ref('')
    const dragTarget = ref('')
    const actionQuery = ref('')
    const actionCategory = ref('')
    const fullKeyboard = ref(true)
    let actionDragStart: { x: number; y: number; action: string } | undefined
    let suppressActionClick = false
    function actionPointerDown(event: PointerEvent, action: string): void {
      if (event.button !== 0) return
      suppressActionClick = false
      actionDragStart = { x: event.clientX, y: event.clientY, action }
      ;(event.currentTarget as HTMLElement).setPointerCapture(event.pointerId)
    }
    function actionPointerMove(event: PointerEvent): void {
      if (!actionDragStart || Math.hypot(event.clientX - actionDragStart.x, event.clientY - actionDragStart.y) < 6) return
      draggedAction.value = actionDragStart.action
      const target = (event.currentTarget as HTMLElement).ownerDocument.elementFromPoint(event.clientX, event.clientY)?.closest<HTMLElement>('[data-map-key]')
      dragTarget.value = target?.dataset.mapKey ?? ''
    }
    function actionPointerUp(event: PointerEvent): void {
      if (draggedAction.value) {
        suppressActionClick = true
        const target = (event.currentTarget as HTMLElement).ownerDocument.elementFromPoint(event.clientX, event.clientY)?.closest<HTMLElement>('[data-map-key]')
        if (target) {
          pendingAction.value = ''
          selectKey({ key: target.dataset.mapKey!, label: target.dataset.mapKey!, literal: target.dataset.literal === 'true' })
          setAction(draggedAction.value)
        }
      }
      actionDragStart = undefined
      draggedAction.value = ''; dragTarget.value = ''
    }
    const filteredActionGroups = computed(() => actionGroups
      .filter(group => !actionCategory.value || group.name === actionCategory.value)
      .toSorted((a, b) => {
        const priority = activeMode.value.startsWith('window') ? ['Window', '窗口布局方向', 'Tabs', '模式'] : ['移动', '点击', '滚动', '模式', '按键状态']
        const rank = (name: string) => priority.includes(name) ? priority.indexOf(name) : priority.length
        return rank(a.name) - rank(b.name)
      })
      .map(group => ({ ...group, actions: group.actions.filter(action => `${action.label} ${t(action.label)} ${action.value}`.toLowerCase().includes(actionQuery.value.trim().toLowerCase())) }))
      .filter(group => group.actions.length))
    const customAction = ref('')
    const windowScreen = ref('1')
    const validWindowScreen = computed(() => /^\d+$/.test(windowScreen.value)
      && Number.isSafeInteger(Number(windowScreen.value)) && Number(windowScreen.value) >= 1)
    const message = ref('正在载入默认配置…')
    const importInput = ref<HTMLInputElement | null>(null)
    const simulator = reactive(createSimulatorState())
    const screen = ref<HTMLElement | null>(null)
    const simulatorArmed = ref(false)
    const canvasScale = ref(1)
    let previewObserver: ResizeObserver | undefined
    const layoutNote = ref('')
    const layoutNoteInput = ref<HTMLInputElement>()
    const layoutStorageError = ref('')
    const layoutFileInput = ref<HTMLInputElement>()
    const modeUsage = ref<ModeUsage>({})
    const quickVisible = ref(false)
    const quickRows = ref<string[]>([])
    let quickStarted = 0
    let quickUsed = false
    const quickCandidates = computed(() => modes.map(mode => mode.id).filter(mode => mode !== 'hotkeys' && mode !== simulator.mode && effectiveDocument.value?.[mode]?.enabled !== false && !(effectiveDocument.value?.quick_switch?.blacklist ?? ['idle']).includes(mode)).sort((a, b) => {
      const x = BigInt(modeUsage.value[a] ?? '0'), y = BigInt(modeUsage.value[b] ?? '0')
      return x === y ? a.localeCompare(b) : x > y ? -1 : 1
    }).slice(0, 9))
    function previewQuickSwitch(): void {
      quickRows.value = [...quickCandidates.value]; quickVisible.value = true
      mobileView.value = 'preview'
      nextTick(() => screen.value?.focus())
    }
    function chooseQuickMode(mode: string): void {
      quickUsed = true; quickVisible.value = false
      if (mode !== simulator.mode) setPreviewMode(mode as Exclude<EditorMode, 'hotkeys'>)
    }
    const usageRanking = computed(() => Object.entries(modeUsage.value).sort(([a, x], [b, y]) => BigInt(x) === BigInt(y) ? a.localeCompare(b) : BigInt(x) > BigInt(y) ? -1 : 1))
    function persistLayoutLibrary(): void {
      if (layoutStorageError.value) return
      try {
        localStorage.setItem(WORKSPACE_STORAGE_KEY, JSON.stringify(simulator.window.presets))
        localStorage.setItem(`${WORKSPACE_STORAGE_KEY}.usage`, JSON.stringify(modeUsage.value))
      }
      catch (error) { simulator.lastEvent = `浏览器无法保留布局，请下载文件：${formatError(error)}` }
    }
    async function importLayoutFile(event: Event): Promise<void> {
      const input = event.target as HTMLInputElement, file = input.files?.[0]
      if (!file) return
      try {
        if (file.size > 1024 * 1024) throw new Error('工作区文件超过 1 MiB')
        const workspace = decodeWorkspaceDocument(new Uint8Array(await file.arrayBuffer()))
        const layouts = workspace.presets
        modeUsage.value = workspace.usage
        replaceWindowPresets(simulator, layouts); layoutStorageError.value = ''; persistLayoutLibrary()
        switchWindowMode(simulator, 'window_restore', effectiveDocument.value?.window_restore ?? {}); simulator.lastEvent = `已导入 ${layouts.length} 个预设`
      } catch (error) { simulator.lastEvent = `导入失败：${formatError(error)}` }
      finally { input.value = '' }
    }
    function downloadLayouts(): void {
      try {
        if (hasWindowPresetChanges(simulator.window)) {
          const note = simulator.window.presets.find(p => p.id === simulator.window.editingPresetId)?.note ?? ''
          saveWindowPreset(simulator, note); persistLayoutLibrary()
        }
        downloadLayoutBinary(encodeWorkspaceFile(simulator.window.presets, modeUsage.value))
        simulator.lastEvent = '已下载 workspace.ksw · 替换程序同名文件后，按 R 重新读取'
      } catch (error) { simulator.lastEvent = `下载失败：${formatError(error)}` }
    }
    function finishLayoutNote(save: boolean): void {
      if (!save) { simulator.window.noteOpen = false; return }
      const before = simulator.window.presets
      try {
        saveWindowPreset(simulator, layoutNote.value)
        persistLayoutLibrary()
      } catch (error) {
        simulator.window.presets = before; simulator.window.noteOpen = true
        simulator.lastEvent = `保存失败：${formatError(error)}`
      }
    }
    const heldActions = new Set<string>()
    watch(() => simulator.window.noteOpen, async open => {
      heldActions.clear(); heldCharacterActions.clear(); physicalKeys.clear()
      if (open) layoutNote.value = simulator.window.presets.find(p => p.id === simulator.window.editingPresetId)?.note ?? ''
      await nextTick()
      if (open) layoutNoteInput.value?.focus(); else screen.value?.focus()
    })
    const clickPulse = ref(0)
    const scrollPulse = ref('')
    const isMac = ref(false)
    const primaryCaption = computed(() => shortcutCaption(effectiveDocument.value ?? {}, 'primary', isMac.value)
      .replaceAll('LEFT ', locale.value === 'en' ? 'Left ' : '左 ').replaceAll('RIGHT ', locale.value === 'en' ? 'Right ' : '右 ')
      .replaceAll('CMD', '⌘ Cmd').replaceAll('CTRL', isMac.value ? '⌃ Ctrl' : 'Ctrl')
      .replaceAll('ALT', isMac.value ? '⌥ Option' : 'Alt').replaceAll('SHIFT', '⇧ Shift'))
    function platformKey(spec: KeySpec): KeySpec {
      if (spec.literal) return spec
      const physical = spec.key.replace(/^(left|right)_/, '')
      const label = physical === 'cmd' ? (isMac.value ? '⌘ Cmd' : '⊞ Win')
        : physical === 'alt' ? (isMac.value ? '⌥ Opt' : 'Alt')
        : physical === 'ctrl' ? (isMac.value ? '⌃ Ctrl' : 'Ctrl')
        : physical === 'shift' ? '⇧ Shift' : spec.label
      return { ...spec, label }
    }
    function selectPlatform(mac: boolean): void {
      releasePreview()
      isMac.value = mac
    }
    let animationFrame = 0
    let previousFrame = 0

    const effectiveDocument = computed<ConfigDocument | null>(() => {
      if (!document.value) return null
      const resolved = defaultDocument.value ? resolveConfigDocument(defaultDocument.value, document.value) : cloneConfigDocument(document.value)
      return resolved
    })

    const tomlPreview = computed(() => {
      if (!document.value) return ''
      try {
        return stringify(document.value)
      } catch (error) {
        return `# 无法生成 TOML：${formatError(error)}`
      }
    })

    watch(() => effectiveDocument.value?.key_help?.mouse_key_help, value => {
      if (simulator.mode !== 'idle') simulator.keyHelpVisible = value === true
    }, { immediate: true })
    watch(() => simulator.mode, (mode, previous) => {
      if (mode === 'idle') simulator.keyHelpVisible = false
      else if (!isWindowMode(mode) && (previous === 'idle' || isWindowMode(previous))) {
        simulator.keyHelpVisible = effectiveDocument.value?.key_help?.mouse_key_help === true
      }
    })

    const selectedAction = computed(() => {
      if (!effectiveDocument.value || !selectedChord.value) return ''
      const value = resolveBinding(effectiveDocument.value, activeMode.value, selectedChord.value)?.value
      return Array.isArray(value) ? value.join(' → ') : String(value ?? '')
    })

    const targetingVisual = computed(() => targetingAppearance(effectiveDocument.value, simulator.mode, appearance.value))
    const targetingSettings = computed(() => effectiveDocument.value?.[simulator.mode] ?? {})
    const temporaryMode = computed(() => String(targetingSettings.value.temporary_mode ?? 'normal'))
    let windowNumberTimer: ReturnType<typeof setTimeout> | undefined
    watch(() => isWindowMode(simulator.mode) && !simulator.window.temporary ? simulator.window.numberDeadline : null, deadline => {
      clearTimeout(windowNumberTimer)
      if (deadline !== null) windowNumberTimer = setTimeout(() => finishWindowNumber(simulator, effectiveDocument.value?.[simulator.mode] ?? {}), Math.max(0, deadline - Date.now()))
    })
    onBeforeUnmount(() => clearTimeout(windowNumberTimer))
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

    function importSource(source: string, name: string, successMessage: string): void {
      const parsed = parseConfigDocument(source)
      document.value = parsed.document
      sourceName.value = name
      sourceStats.value = { bytes: parsed.bytes, sections: parsed.sections, values: parsed.values }
      message.value = successMessage
    }

    async function initialize(): Promise<void> {
      // Capture and erase the fragment synchronously before the first await.
      const handoff = consumeConfigHandoff(window.location, window.history)
      await loadDefault()
      const result = await handoff
      if (result.kind === 'config') {
        try {
          importSource(
            result.source,
            '当前 KeySteer 配置',
            '已从 KeySteer 导入当前配置；数据仅在本机浏览器中处理',
          )
          if (result.presets !== undefined) {
            modeUsage.value = result.usage ?? {}
            replaceWindowPresets(simulator, result.presets); layoutStorageError.value = ''; persistLayoutLibrary()
            setPreviewMode('window_restore')
            message.value = `已从 KeySteer 同时导入按键配置和 ${result.presets.length} 个预设`
          }
          if (result.presetError) message.value += `；预设未导入：${result.presetError}`
        } catch (error) {
          message.value = `TOML 解析失败：${formatError(error)}`
        }
      } else if (result.kind === 'error') {
        message.value = result.message
      }
    }

    function selectKey(spec: KeySpec): void {
      if (!spec.key) return
      const parts: string[] = spec.literal ? [] : (Object.keys(modifiers) as Modifier[]).filter((modifier) => modifiers[modifier])
      if (!parts.includes(spec.key)) parts.push(spec.key)
      selectedChord.value = parts.join('+')
      if (pendingAction.value) {
        setAction(pendingAction.value)
        pendingAction.value = ''
      }
      customAction.value = selectedAction.value
      const target = /^move_window\s+(\d+)$/.exec(selectedAction.value)
      if (target) windowScreen.value = target[1]
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
      pendingAction.value = ''
      customAction.value = selectedAction.value
      message.value = selectedAction.value
        ? `已移除当前模式绑定，${selectedChord.value} 恢复继承：${selectedAction.value}`
        : `已移除 ${selectedChord.value}，此键未绑定`
    }

    function keyBindingInfo(spec: KeySpec): KeyBindingInfo | undefined {
      if (!effectiveDocument.value || !spec.key) return undefined
      const entries = [...effectiveBindings(effectiveDocument.value, activeMode.value)]
        .filter(([chord]) => chord === spec.key || (!spec.literal && chord.endsWith(`+${spec.key}`)))
      if (entries.length === 0) return undefined
      const [chord, binding] = entries[0]
      const action = Array.isArray(binding.value) ? String(binding.value[0] ?? '') : String(binding.value)
      const sourcePrefix = binding.source === activeMode.value ? '' : `${shortMode(binding.source)} `
      const prefix = chord === spec.key ? sourcePrefix : `${sourcePrefix}${shortChord(chord)} `
      return { text: `${prefix}${t(shortAction(action))}`, tone: actionTone(action) }
    }

    function onImport(event: Event): void {
      const input = event.target as HTMLInputElement
      const file = input.files?.[0]
      if (!file) return
      file.text().then((source) => {
        try {
          importSource(source, file.name, `已导入 ${file.name}；配置值已解析，原注释不会写入生成文件`)
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
      const lookupMode = simulator.mode === 'idle' ? 'hotkeys' : isWindowMode(simulator.mode) && simulator.window.temporary ? temporaryMode.value : simulator.mode
      const binding = resolveBinding(effectiveDocument.value, lookupMode, chord)?.value
      if (Array.isArray(binding)) return binding.map(String)
      return typeof binding === 'string' ? [binding] : []
    }

    function executeAction(action: string, continuous = false): void {
      if (action === 'window_restore' || action === 'window_delete' || action === 'window_confirm' && simulator.window.deletingPresets && simulator.window.deleteSelection) {
        try { simulator.window.presets = readSavedPresets(localStorage.getItem(WORKSPACE_STORAGE_KEY)) }
        catch (error) { layoutStorageError.value = formatError(error); simulator.lastEvent = layoutStorageError.value; return }
      }
      if (applyWindowAction(simulator, action, effectiveDocument.value?.[isWindowMode(action) ? action : simulator.mode] ?? {}, Date.now(), continuous ? .016 : undefined)) { persistLayoutLibrary(); return }
      if (applyKeyHelpAction(simulator, action, effectiveDocument.value?.key_help?.mouse_key_help !== false, effectiveDocument.value?.key_help?.window_key_help !== false)) return
      if (MOVEMENT_ACTIONS.has(action)) {
        movePointer(simulator, action, continuous ? 0.45 : 2.5)
        return
      }
      if (applyModeAction(simulator, action, effectiveDocument.value?.key_help?.mouse_key_help === true)) return
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
      if (['mouse_x1', 'xbutton1', 'mouse4', 'x1_click', 'mouse_x2', 'xbutton2', 'mouse5', 'x2_click', 'x1_double_click', 'x2_double_click'].includes(action)) {
        clickPulse.value += 1
        simulator.lastEvent = action
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
      if (simulator.window.noteOpen) return
      if (!simulatorArmed.value) return
      const quickKey = String(effectiveDocument.value?.quick_switch?.key ?? 'q')
      if (quickStarted && !event.repeat && /^[1-9]$/.test(event.key)) {
        event.preventDefault()
        quickUsed = true
        const mode = quickRows.value[Number(event.key) - 1]
        if (mode) chooseQuickMode(mode)
        return
      }
      if (simulator.mode !== 'idle' && effectiveDocument.value?.quick_switch?.enabled !== false && event.key.toLowerCase() === quickKey && !event.shiftKey && !event.altKey && !event.ctrlKey && !event.metaKey) {
        event.preventDefault()
        if (!event.repeat) { quickStarted = performance.now(); quickUsed = false; quickRows.value = [...quickCandidates.value] }
        return
      }
      if (quickVisible.value) {
        event.preventDefault()
        if (event.key === 'Escape') { quickVisible.value = false; quickUsed = true }
        else if (/^[1-9]$/.test(event.key)) { const mode = quickRows.value[Number(event.key) - 1]; if (mode) chooseQuickMode(mode) }
        return
      }
      if (event.repeat) {
        const document = effectiveDocument.value
        if (document && isWindowMode(simulator.mode)) {
          const resolved = resolvePhysicalBinding(document, simulator.mode, currentPhysicalKeys(event), physicalKey(event), isMac.value)
          const action = String(resolved?.value)
          if (['window_volume_down', 'window_volume_up', 'window_system_volume_down', 'window_system_volume_up'].includes(action) && windowActionAvailable(simulator.window, action)) {
            event.preventDefault(); executeAction(action)
          }
        }
        return
      }
      const physical = physicalKey(event)
      physicalKeys.add(physical)
      updateTemporaryWindow(event)
      if (event.key === 'Escape' && !isWindowMode(simulator.mode)) {
        simulatorArmed.value = false
        heldActions.clear(); heldCharacterActions.clear()
        return
      }
      const document = effectiveDocument.value
      if (document) {
        const pressed = currentPhysicalKeys(event)
        if (isWindowMode(simulator.mode)) {
          const temporary = simulator.window.temporary ? temporaryPhysicalKeys(document, simulator.mode, pressed, isMac.value, temporaryEntryKeys) : []
          let resolved = resolveLayeredPhysicalBinding(document, simulator.mode, pressed, physical, isMac.value, temporaryEntryKeys)
          // Shift-generated literals (notably ~) still reach character bindings
          // after physical chords, just as the native input router does.
          if (!resolved && !temporary.length && !event.ctrlKey && !event.altKey && !event.metaKey && [...event.key].length === 1) {
            resolved = resolveBinding(document, simulator.mode, event.key.toLowerCase())
          }
          if (!temporary.length && resolved && !windowActionAvailable(simulator.window, String(resolved.value))) resolved = undefined
          if (!resolved && !temporary.length && pressed.length === 1 && windowSelectionKey(simulator, physical, document[simulator.mode] ?? {})) { event.preventDefault(); persistLayoutLibrary(); return }
          if (resolved) {
            event.preventDefault()
            const actions = Array.isArray(resolved.value) ? resolved.value.map(String) : [String(resolved.value)]
            heldCharacterActions.set(event.code, actions)
            actions.forEach(action => {
              if (WINDOW_MOTION.has(action) || MOVEMENT_ACTIONS.has(action) && (simulator.mode !== 'window_tab' || simulator.window.temporary)) heldActions.add(action)
              executeAction(action)
            })
            return
          }
          if (temporary.includes(physical)) { event.preventDefault(); return }
          event.preventDefault(); return
        } else {
          const resolved = resolvePhysicalBinding(document, simulator.mode === 'idle' ? 'hotkeys' : simulator.mode, pressed, physical, isMac.value)
          const actions = Array.isArray(resolved?.value) ? resolved.value.map(String) : [String(resolved?.value)]
          if (actions.some(action => ['idle', 'normal', 'grid', 'recursive_grid', 'ui_hint'].includes(action) || isWindowMode(action))) {
            event.preventDefault(); heldActions.clear(); actions.forEach(action => executeAction(action)); return
          }
        }
      }
      const character = event.key.toLowerCase()
      const characterActions = [...character].length === 1 && character !== browserKeyName(event.key, event.code) ? resolveAction(character) : []
      if (characterActions.length > 0) {
        event.preventDefault()
        characterActions.forEach(action => {
          if (MOVEMENT_ACTIONS.has(action)) heldActions.add(action)
          else executeAction(action)
        })
        heldCharacterActions.set(event.code, characterActions)
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

    const heldCharacterActions = new Map<string, string[]>()
    const physicalKeys = new Set<string>()
    const temporaryEntryKeys = new Set<string>()
    watch(() => simulator.mode, () => {
      temporaryEntryKeys.clear()
      physicalKeys.forEach(key => temporaryEntryKeys.add(key))
    }, { flush: 'sync' })
    function physicalKey(event: KeyboardEvent): string {
      const modifiers: Record<string, string> = { AltLeft: 'left_alt', AltRight: 'right_alt', ControlLeft: 'left_ctrl', ControlRight: 'right_ctrl', ShiftLeft: 'left_shift', ShiftRight: 'right_shift', MetaLeft: 'left_cmd', MetaRight: 'right_cmd' }
      return modifiers[event.code] ?? browserKeyName(event.key, event.code)
    }
    function currentPhysicalKeys(event: KeyboardEvent): string[] {
      const keys = [...physicalKeys]
      for (const [family, active] of [['alt', event.altKey], ['ctrl', event.ctrlKey], ['shift', event.shiftKey], ['cmd', event.metaKey]] as const) {
        if (active && !keys.some(k => k.endsWith(`_${family}`))) keys.push(`left_${family}`)
      }
      return keys
    }
    function updateTemporaryWindow(event: KeyboardEvent): void {
      if (!isWindowMode(simulator.mode) || !effectiveDocument.value) return
      const pressed = currentPhysicalKeys(event), physical = physicalKey(event)
      const resolved = resolveLayeredPhysicalBinding(effectiveDocument.value, simulator.mode, pressed, physical, isMac.value, temporaryEntryKeys)
      const active = resolved?.source !== simulator.mode && temporaryPhysicalKeys(effectiveDocument.value, simulator.mode, pressed, isMac.value, temporaryEntryKeys).length > 0
      if (active !== simulator.window.temporary) heldActions.clear()
      temporaryWindow(simulator.window, active, effectiveDocument.value[simulator.mode] ?? {})
    }
    function onSimulatorKeyUp(event: KeyboardEvent): void {
      if (quickStarted && event.key.toLowerCase() === String(effectiveDocument.value?.quick_switch?.key ?? 'q')) {
        event.preventDefault()
        const shortPress = !quickVisible.value && !quickUsed
        quickStarted = 0; quickVisible.value = false
        if (shortPress && !handleTargetingKey(event.key)) resolveAction(event.key).forEach(action => executeAction(action))
        return
      }
      physicalKeys.delete(physicalKey(event))
      temporaryEntryKeys.delete(physicalKey(event))
      updateTemporaryWindow(event)
      heldCharacterActions.get(event.code)?.forEach(action => heldActions.delete(action))
      heldCharacterActions.delete(event.code)
      if (![...heldActions].some(action => WINDOW_MOTION.has(action))) simulator.window.gesture = false
      const chord = chordFromEvent(event, isMac.value)
      if (!chord) {
        heldActions.clear(); heldCharacterActions.clear()
        return
      }
      resolveAction(chord).forEach((action) => heldActions.delete(action))
    }

    function animate(timestamp: number): void {
      const delta = previousFrame ? Math.min(32, timestamp - previousFrame) : 16
      previousFrame = timestamp
      if (quickStarted && !quickUsed && timestamp - quickStarted >= Number(effectiveDocument.value?.quick_switch?.hold_ms ?? 350)) quickVisible.value = true
      heldActions.forEach((action) => {
        if (WINDOW_MOTION.has(action)) applyWindowAction(simulator, action, effectiveDocument.value?.[simulator.mode] ?? {}, Date.now(), delta / 1000)
        else movePointer(simulator, action, delta * 0.028)
      })
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
        updateGridPointer(settings, path)
        simulator.lastEvent = '返回上一层网格'
        return true
      }
      const keys = String(settings.keys ?? '')
      const index = Array.from(keys).indexOf(keyName)
      if (index < 0 || index >= numberSetting(settings.grid_cols, simulator.mode === 'grid' ? 5 : 3) * numberSetting(settings.grid_rows, simulator.mode === 'grid' ? 4 : 3)) return false
      const maxDepth = numberSetting(settings.max_depth, simulator.mode === 'grid' ? 3 : 10)
      if (path.length < maxDepth) path.push(keyName)
      updateGridPointer(settings, path)
      simulator.lastEvent = `${simulator.mode}：${path.join(' → ')}`
      return true
    }

    function updateGridPointer(settings: ConfigDocument, path: string[]): void {
      const region = gridRegion(numberSetting(settings.grid_cols, simulator.mode === 'grid' ? 5 : 3), numberSetting(settings.grid_rows, simulator.mode === 'grid' ? 4 : 3), String(settings.keys ?? ''), path)
      simulator.pointer.x = region.x + region.width / 2
      simulator.pointer.y = region.y + region.height / 2
    }

    function setPreviewMode(mode: 'normal' | 'grid' | 'recursive_grid' | 'ui_hint' | 'window' | 'window_quick' | 'window_editor' | 'window_restore' | 'window_tab'): void {
      heldActions.clear(); physicalKeys.clear()
      if (isWindowMode(mode)) switchWindowMode(simulator, mode, effectiveDocument.value?.[mode] ?? {})
      else applyModeAction(simulator, mode, effectiveDocument.value?.key_help?.mouse_key_help === true)
      simulator.lastEvent = `预览 ${mode}`
    }

    onMounted(() => {
      isMac.value = /Mac|iPhone|iPad/.test(navigator.platform)
      try { simulator.window.presets = readSavedPresets(localStorage.getItem(WORKSPACE_STORAGE_KEY)) }
      catch (error) { layoutStorageError.value = formatError(error) }
      try {
        const saved = localStorage.getItem(`${WORKSPACE_STORAGE_KEY}.usage`)
        if (saved && saved.length <= 65536) {
          const usage = JSON.parse(saved)
          modeUsage.value = decodeWorkspaceDocument(encodeWorkspaceFile([], usage)).usage
        }
      } catch (error) { layoutStorageError.value = formatError(error) }
      void initialize()
      animationFrame = requestAnimationFrame(animate)
      previewObserver = new ResizeObserver(entries => {
        const rect = entries[0]?.contentRect
        if (rect && rect.width && rect.height) canvasScale.value = Math.min(rect.width / 960, rect.height / 600)
      })
      if (screen.value) previewObserver.observe(screen.value)
    })
    onBeforeUnmount(() => {
      previewObserver?.disconnect()
      cancelAnimationFrame(animationFrame)
    })

    const keyboard = () => (
      <div class={{ 'ks-keyboard-scroll': true, 'ks-main-keyboard': !fullKeyboard.value }} aria-label={fullKeyboard.value ? t("ANSI 104 键盘") : t("键盘主键区")}>
        <FittedKeyboard>
        <div class="ks-keyboard">
          {fullKeyboard.value && <KeyboardRows rows={[functionRow.map(platformKey)]} selected={selectedChord.value} onKey={selectKey} bindingInfo={keyBindingInfo} />}
          <div class="ks-keyboard-body">
            <KeyboardRows rows={mainRows.map(row => row.map(platformKey))} selected={selectedChord.value} onKey={selectKey} bindingInfo={keyBindingInfo} />
            {fullKeyboard.value && <KeyboardRows rows={navigationRows} selected={selectedChord.value} onKey={selectKey} bindingInfo={keyBindingInfo} compact />}
            {fullKeyboard.value && <Numpad keys={numpadKeys} selected={selectedChord.value} onKey={selectKey} bindingInfo={keyBindingInfo} />}
          </div>
        </div>
        </FittedKeyboard>
      </div>
    )

    const renderKeyboard = () => (
        <section class="ks-card ks-keyboard-card" style={editorVisual.value as any}>
          <div class="ks-toolbar ks-compact-toolbar">
            <div>
              <h2>{t("按键映射")}</h2>
              <p>{t("先选按键再选动作，也可以把动作拖到键帽上。")}</p>
            </div>
            <button class="ks-button" aria-pressed={fullKeyboard.value} onClick={() => fullKeyboard.value = !fullKeyboard.value}>{fullKeyboard.value ? t("切换主键区") : t("显示完整键盘")}</button>
          </div>
          <div class="ks-keyboard-tools">
            <div class="ks-platform-picker" role="group" aria-label={t("预览平台")}>
              <button aria-pressed={!isMac.value} onClick={() => selectPlatform(false)}><span aria-hidden="true">⊞</span> Windows</button>
              <button aria-pressed={isMac.value} onClick={() => selectPlatform(true)}><span aria-hidden="true">⌘</span> macOS</button>
              <small>Primary → {primaryCaption.value}</small>
            </div>
            <div class="ks-modifiers">
              <span>{t("组合键")}</span>
              {(Object.keys(modifiers) as Modifier[]).map((modifier) => (
                <button class={{ active: modifiers[modifier] }} aria-pressed={modifiers[modifier]} title={modifier === 'primary' ? t("跨平台别名，当前解析为 {0}；绑定仍保存为 primary", [primaryCaption.value]) : undefined} onClick={() => { modifiers[modifier] = !modifiers[modifier] }}>
                  {modifier === 'primary' ? 'Primary' : modifier === 'shift' ? '⇧ Shift' : isMac.value ? '⌥ Option' : 'Alt'}
                </button>
              ))}
              <input aria-label={t("绑定按键或符号")} placeholder={t("点击键盘或输入符号，如 ?")} value={selectedChord.value} onInput={(event) => { selectedChord.value = (event.target as HTMLInputElement).value }} />
            </div>
          </div>
          {keyboard()}
          {draggedAction.value && <p class="ks-pending-action">{t("拖动 ")}{draggedAction.value}{dragTarget.value ? t(" → {0}，松开替换绑定", [dragTarget.value]) : t(" 到目标键帽")}</p>}
          <details class="ks-keyboard-guide"><summary>{t("符号绑定与颜色说明")}</summary><p class="ks-keyboard-hint">{t("点击键帽内的上层符号可按字符绑定（如 ?）；下层按键可搭配上方组合键。彩色圆点表示已有绑定，悬停可查看动作。")}</p>
          <div class="ks-key-legend" aria-label={t("按键颜色分类")}>
            <span class="tone-move"><i />{t("方向移动")}</span>
            <span class="tone-click"><i />{t("鼠标点击")}</span>
            <span class="tone-speed"><i />{t("速度控制")}</span>
            <span class="tone-state"><i />{t("按键状态")}</span>
            <span class="tone-scroll"><i />{t("滚动")}</span>
            <span class="tone-mode"><i />{t("模式切换")}</span>
            <span class="tone-utility"><i />{t("其他")}</span>
          </div>
          </details>
          <section class="ks-binding-details ks-mapping-editor">
            <div class="ks-mapping-selection" aria-live="polite">
              <div><small>{t("当前按键 · ")}{activeMode.value}</small><strong>{selectedChord.value || t("请选择键帽")}</strong></div>
              <div><small>{t("当前动作")}</small><code>{selectedAction.value === 'none' ? t("已取消绑定（none）") : selectedAction.value || t("未绑定")}</code>
                {selectedAction.value && <small>{resolveBinding(effectiveDocument.value!, activeMode.value, selectedChord.value)?.source === activeMode.value ? t("当前模式绑定") : t("继承自 {0}", [resolveBinding(effectiveDocument.value!, activeMode.value, selectedChord.value)?.source])}{t(" · 选择新动作将覆盖此键")}</small>}
              </div>
              {selectedChord.value && <div class="ks-binding-actions">
                <div>
                  <button disabled={!selectedAction.value || selectedAction.value === 'none'} onClick={() => { pendingAction.value = ''; setAction('none'); message.value = `已取消 ${selectedChord.value} 的绑定，不再执行本模式或继承的绑定动作` }}>{t("取消绑定")}</button>
                  <button disabled={resolveBinding(effectiveDocument.value!, activeMode.value, selectedChord.value)?.source !== activeMode.value} onClick={removeBinding}>{t("移除当前模式绑定")}</button>
                </div>
                <small>{t("取消绑定会禁用此键的绑定动作；移除当前模式绑定后，若有继承动作则恢复继承。")}</small>
              </div>}
            </div>
            {pendingAction.value && <p class="ks-pending-action">{t("已选择 ")}{pendingAction.value}{t("，请点击目标键帽。")}<button onClick={() => pendingAction.value = ''}>{t("取消")}</button></p>}
            <div class="ks-binding-panel">
              <div class="ks-action-search"><input aria-label={t("搜索映射动作")} placeholder={t("搜索动作，例如 点击、屏幕、move_left")} value={actionQuery.value} onInput={event => actionQuery.value = (event.target as HTMLInputElement).value} />
                <select aria-label={t("动作分类")} value={actionCategory.value} onChange={event => actionCategory.value = (event.target as HTMLSelectElement).value}><option value="">{t("全部动作")}</option>{actionGroups.map(group => <option value={group.name}>{t(group.name)}</option>)}</select>
              </div>
              <div class="ks-action-groups">
                {filteredActionGroups.value.map((group) => (
                  <div class="ks-action-group">
                    <strong>{t(group.name)}</strong>
                    <div>{group.actions.map((action) => (
                      <button class={{ active: selectedAction.value === action.value || pendingAction.value === action.value }}
                        title={t("{0} · 点击设置或拖到键帽", [action.value])}
                        onPointerdown={event => actionPointerDown(event, action.value)} onPointermove={actionPointerMove} onPointerup={actionPointerUp}
                        onPointercancel={() => { actionDragStart = undefined; draggedAction.value = ''; dragTarget.value = '' }}
                        onLostpointercapture={() => { actionDragStart = undefined; draggedAction.value = ''; dragTarget.value = '' }}
                        onClick={event => { if (suppressActionClick && event.detail !== 0) { suppressActionClick = false; return }; if (selectedChord.value) setAction(action.value); else pendingAction.value = action.value }}><span>{t(action.label)}</span><code>{action.value}</code></button>
                    ))}</div>
                  </div>
                ))}
                {!filteredActionGroups.value.length && <p>{t("没有匹配的动作，可在下方高级映射中输入自定义命令。")}</p>}
              </div>
              <details class="ks-mapping-advanced"><summary>{t("高级映射 · 自定义命令与指定屏幕")}</summary>
              <div class="ks-custom-action">
                <label for="ks-window-screen">{t("窗口目标屏幕（从 1 开始）")}</label>
                <input id="ks-window-screen" type="number" min="1" step="1" value={windowScreen.value} onInput={(event) => { windowScreen.value = (event.target as HTMLInputElement).value }} />
                <button disabled={!selectedChord.value || !validWindowScreen.value} onClick={() => setAction(`move_window ${Number(windowScreen.value)}`)}>{t("窗口移到指定屏幕")}</button>
              </div>
              <p>{t("速度切换动作按一次启用，再按一次取消。窗口移动作用于鼠标所在的窗口；网页预览暂不模拟这两类动作。")}</p>
              <div class="ks-custom-action">
                <input value={customAction.value} placeholder={t("自定义动作，例如 press shift")} onInput={(event) => { customAction.value = (event.target as HTMLInputElement).value }} onKeydown={(event) => { if (event.key === 'Enter') setAction(customAction.value.trim()) }} />
                <button disabled={!selectedChord.value || !customAction.value.trim()} onClick={() => setAction(customAction.value.trim())}>{t("应用")}</button>
              </div>
              </details>
            </div>
          </section>
        </section>
    )
    const renderPreview = () => (
            <section class="ks-card ks-simulator-card">
              <div class="ks-toolbar ks-compact-toolbar">
                <div><h2>{t("实时预览")}</h2><p>{t("运行模式：")}{simulator.mode}</p></div>
                <button class="ks-button" onClick={toggleExpanded}>{expanded.value ? t("关闭放大") : t("放大预览")}</button>
                <span class={{ 'ks-status': true, armed: simulatorArmed.value }}>{simulatorArmed.value ? t("键盘已捕获") : t("点击预览可试按键")}</span>
              </div>
              {isWindowMode(simulator.mode) && <div class="ks-layout-file-tools"><input ref={layoutFileInput} type="file" accept=".ksw,application/octet-stream" aria-label={t("导入工作区文件")} hidden onChange={importLayoutFile} />
                    <button onClick={() => layoutFileInput.value?.click()}>{t("导入工作区文件")}</button><button onClick={downloadLayouts}>{hasWindowPresetChanges(simulator.window) ? t("保存并下载工作区") : t("下载工作区文件")}</button>
                  </div>}
              <div
                ref={screen}
                class={{ 'ks-screen': true, armed: simulatorArmed.value, [`mode-${simulator.mode}`]: true }}
                style={targetingVisual.value as any}
                tabindex="0"
                onFocus={() => { simulatorArmed.value = true }}
                onBlur={releasePreview}
                onKeydown={event => { if (event.target === screen.value) onSimulatorKeyDown(event) }}
                onKeyup={event => { if (event.target === screen.value) onSimulatorKeyUp(event) }}
              >
                <div class="ks-preview-canvas" style={{ transform: `translate(-50%, -50%) scale(${canvasScale.value})` }}>
                <div class="ks-screen-grid" />
                {!isWindowMode(simulator.mode) && <DesktopBackdrop />}
                {(!isWindowMode(simulator.mode) || simulator.window.temporary) && <div class="ks-mode-badge">{isWindowMode(simulator.mode) ? temporaryMode.value : simulator.mode}</div>}
                {(isWindowMode(simulator.mode) || simulator.window.tabs.groups.length > 0) && <div class="ks-window-demo">
                  <div class="ks-window-screen-label">{t("示例屏幕 ")}{simulator.window.screen + 1} / 2</div>

                  {isWindowMode(simulator.mode) && <label class="ks-window-count">{t("示例窗口数 ")}<select aria-label={t("示例窗口数量")} value={simulator.window.windows.filter(w => w.screen === simulator.window.screen).length}
                    onChange={e => setDemoWindowCount(simulator, Number((e.target as HTMLSelectElement).value))}>
                    {[1, 3, 9, 20, 23, 30].map(count => <option value={count}>{count}</option>)}
                  </select></label>}
                  {simulator.window.windows.filter(w => !w.minimized && w.screen === simulator.window.screen && !containingTab(simulator.window, w.id)).map(w => <div
                    class={{ 'ks-demo-window': true, target: w.id === simulator.window.target && !simulator.window.temporary }}
                    style={{ left: `${w.x / WINDOW_AREA.width * 100}%`, top: `${w.y / WINDOW_AREA.height * 100}%`, width: `${w.width / WINDOW_AREA.width * 100}%`, height: `${w.height / WINDOW_AREA.height * 100}%`, outlineWidth: `${numberSetting(targetingSettings.value.border_width, 3)}px`, zIndex: w.id === simulator.window.target ? 3 : containingTab(simulator.window, w.id)?.active === w.id ? 2 : 1 }}>
                    <div class="ks-demo-window-title">{w.app} · {t(w.title)}</div>
                    <div class="ks-demo-window-lines"><i /><i /><i /></div>
                  </div>)}
                  {simulator.window.tabs.groups.map(group => {
                    const active = simulator.window.windows.find(w => w.id === group.active)
                    if (!active || active.minimized || active.screen !== simulator.window.screen) return null
                    return <div class={{ 'ks-demo-window': true, 'ks-demo-tabbed-window': true, target: group.members.includes(simulator.window.target ?? -1) && !simulator.window.temporary }} style={{ left: `${active.x / 10}%`, top: `${active.y / 6.5}%`, width: `${active.width / 10}%`, height: `${active.height / 6.5}%`, zIndex: group.members.includes(simulator.window.target ?? -1) ? 3 : 2 }}>
                      <div class="ks-demo-tab-bar" onWheel={event => {
                        event.preventDefault()
                        const strip = event.currentTarget as HTMLElement
                        strip.scrollLeft += (event.deltaX || event.deltaY) * (event.deltaMode === 1 ? 16 : event.deltaMode === 2 ? strip.clientWidth : 1)
                      }}>
                      <button class={{ selected: simulator.mode === 'window_tab' && simulator.window.numberSlot }} onMousedown={e => e.preventDefault()} onClick={() => { if (simulator.mode === 'window_tab') chooseTabTarget(simulator, { kind: 'group', id: group.id }) }}>~{group.id}</button>
                      {group.members.map(id => <button class={{ selected: id === group.active }} onMousedown={e => e.preventDefault()} onClick={() => activateTab(simulator, id)}>{simulator.window.numbers[id]} · {simulator.window.windows.find(w => w.id === id)?.title}</button>)}
                      </div>
                      <div class="ks-demo-window-title">{active.app} · {t(active.title)}</div><div class="ks-demo-window-lines"><i /><i /><i /></div>
                    </div>
                  })}
                  {isWindowMode(simulator.mode) && !simulator.window.temporary && !simulator.window.library && windowNumberLabels(simulator.window, simulator.mode === 'window' ? effectiveDocument.value?.window?.card ?? {} : simulator.mode === 'window_editor' ? effectiveDocument.value?.window_editor?.card ?? {} : {}).map(label => <button class="ks-window-number"
                    aria-label={t("选择窗口 {0}", [label.number])} style={{ left: `${label.x}%`, top: `${label.y}%`, fontSize: `${numberSetting(targetingSettings.value.ui?.font_size, 28)}px` }}
                    onMousedown={e => e.preventDefault()} onClick={() => chooseWindowNumber(simulator, label.number, targetingSettings.value)}><b>{label.number}</b><span><strong>{t(label.app)}</strong>{(label.members ?? [label.title]).map(title => <small title={t(title)}>{t(title)}</small>)}</span></button>)}
                  {isWindowMode(simulator.mode) && !simulator.window.temporary && !simulator.window.library && simulator.window.tree && treeSlots(simulator.window.tree).map(slot => {
                    const rect = layoutRect(slot.rect, WINDOW_AREA, numberSetting(targetingSettings.value.gap, 0))
                    return <div class={{ 'ks-window-slot': true, selected: simulator.window.tree?.selected === slot.id }}
                      style={{ left: `${rect.x / WINDOW_AREA.width * 100}%`, top: `${rect.y / WINDOW_AREA.height * 100}%`, width: `${rect.width / WINDOW_AREA.width * 100}%`, height: `${rect.height / WINDOW_AREA.height * 100}%` }}>
                      <button aria-label={t("选择区域 {0}", [slot.id])} style={{ fontSize: `${numberSetting(targetingSettings.value.ui?.font_size, 28)}px` }} onMousedown={e => e.preventDefault()} onClick={() => chooseWindowNumber(simulator, slot.id, targetingSettings.value, true)}>{'`'}{slot.id}</button>
                    </div>
                  })}
                </div>}
                {isWindowMode(simulator.mode) && simulator.window.noteOpen && <div class="ks-layout-note-backdrop"><form class="ks-layout-note" aria-label="Save preset"
                  onSubmit={e => { e.preventDefault(); finishLayoutNote(true) }}
                  onKeydown={e => { e.stopPropagation(); if (e.key === 'Escape' && !e.isComposing) { e.preventDefault(); finishLayoutNote(false) } }} onKeyup={e => e.stopPropagation()}>
                  <div class="ks-layout-note-heading"><strong>{simulator.window.editingPresetId === null ? 'Save preset' : `Update preset ${simulator.window.editingPresetId}`}</strong><span>{t("备注可留空，将自动命名")}</span></div>
                  <div class="ks-layout-note-row"><input aria-label={t("备注（可留空）")} ref={layoutNoteInput} value={layoutNote.value} onInput={e => layoutNote.value = (e.target as HTMLInputElement).value} placeholder={t("例如：写代码 / 阅读")} /><button type="submit">Save</button><button type="button" onClick={() => finishLayoutNote(false)}>Cancel</button></div>
                </form></div>}
                {quickVisible.value && <QuickSwitchPreview rows={quickRows.value} settings={effectiveDocument.value?.quick_switch ?? {}} appearance={appearance.value} pointer={simulator.pointer} windowCenter={(() => { const w = simulator.window.windows.find(w => w.id === simulator.window.target); return w ? { x: (w.x + w.width / 2) / WINDOW_AREA.width * 100, y: (w.y + w.height / 2) / WINDOW_AREA.height * 100 } : { x: 50, y: 50 } })()} onSelect={chooseQuickMode} />}
                {(simulator.mode === 'grid' || simulator.mode === 'recursive_grid') && <div class="ks-target-backdrop" />}
                {simulator.mode === 'grid' && <TargetGrid mode="grid" settings={targetingSettings.value} path={simulator.targeting.grid.path} />}
                {simulator.mode === 'recursive_grid' && <TargetGrid mode="recursive_grid" settings={targetingSettings.value} path={simulator.targeting.recursiveGrid.path} />}
                {simulator.mode === 'ui_hint' && <HintOverlay settings={targetingSettings.value} />}
                <div class={{ 'ks-pointer': true, pressed: simulator.pressedButtons.has('left') }} style={{ left: `${simulator.pointer.x}%`, top: `${simulator.pointer.y}%` }}>
                  <span key={clickPulse.value} class={clickPulse.value ? 'pulse' : ''} />
                </div>
                {scrollPulse.value && <div class="ks-scroll-pulse">{scrollPulse.value}</div>}
                {effectiveDocument.value && isWindowMode(simulator.mode) && (simulator.windowHelpOverride ?? effectiveDocument.value.key_help?.window_key_help !== false) && !simulator.window.temporary && !simulator.window.noteOpen && (
                  <KeyHelpPreview isMac={isMac.value} document={effectiveDocument.value} mode={simulator.mode} appearance={appearance.value}
                    anchor={simulator.window.library ? undefined : windowTarget(simulator.window)} detail={windowDetail(simulator.window)}
                    onRestore={(id: number) => { restoreWindowPreset(simulator, id, targetingSettings.value); persistLayoutLibrary() }}
                    status={windowInputStatus(simulator.window) || ((simulator.lastEvent.startsWith('window_') || simulator.lastEvent === 'size_cycle') ? '' : simulator.lastEvent)}
                    windowState={simulator.window} />
                )}
                {effectiveDocument.value && simulator.keyHelpVisible && simulator.mode !== 'idle' && (!isWindowMode(simulator.mode) || simulator.window.temporary) && (
                  <KeyHelpPreview isMac={isMac.value} document={effectiveDocument.value} mode={isWindowMode(simulator.mode) ? temporaryMode.value : simulator.mode} appearance={appearance.value} />
                )}
                {!isWindowMode(simulator.mode) && <div class="ks-event-log">{t(simulator.lastEvent)}</div>}
              </div>
              </div>
              <details class="ks-preview-help"><summary>{t("试用说明")}</summary><p>{t("点击画面后试用快捷键；离开画面即释放按键。模拟操作只影响示例窗口。编辑设置不会控制桌面上的实际窗口。")}</p><button onClick={() => executeAction('key_help')}>{t("切换按键提示")}</button></details>
            </section>
    )
    const renderQuickSwitch = () => (
            <details class="ks-toml-details ks-quick-settings" open>
              <summary>{t("快速模式切换")}</summary>
              <div class="ks-settings-body">
              <p>{t("操作模式中长按 ")}{String(effectiveDocument.value?.quick_switch?.key ?? 'q').toUpperCase()}{t(" 展开，配合 1…9 选择；松开收起。Idle 不接管，黑名单仅限制快速切换。")}</p>
              <button class="ks-button" disabled={!document.value} onClick={previewQuickSwitch}>{t("在模拟器中预览")}</button>
              <div class="ks-settings-grid">
              <label hidden={selectedTab.value !== 'behavior'} data-config-path="quick_switch.key">{t("触发键")}<input value={effectiveDocument.value?.quick_switch?.key ?? 'q'} onChange={event => {
                const value = (event.target as HTMLInputElement).value.trim().toLowerCase()
                if (document.value && /^[a-z]$/.test(value)) { document.value.quick_switch ??= {}; document.value.quick_switch.key = value }
              }} /></label>
              <label hidden={selectedTab.value !== 'behavior'} data-config-path="quick_switch.enabled">{t("启用 ")}<input type="checkbox" checked={effectiveDocument.value?.quick_switch?.enabled !== false}
                onChange={event => { if (document.value) { document.value.quick_switch ??= {}; document.value.quick_switch.enabled = (event.target as HTMLInputElement).checked } }} /></label>
              <label hidden={selectedTab.value !== 'behavior'} data-config-path="quick_switch.hold_ms">{t("长按毫秒 ")}<input type="number" min="1" max="10000" value={effectiveDocument.value?.quick_switch?.hold_ms ?? 350}
                onChange={event => { const value = Number((event.target as HTMLInputElement).value); if (document.value && value >= 1 && value <= 10000 && Number.isInteger(value)) { document.value.quick_switch ??= {}; document.value.quick_switch.hold_ms = value } }} /></label>
              <label hidden={selectedTab.value !== 'behavior'} data-config-path="quick_switch.position">{t("位置 ")}<select value={effectiveDocument.value?.quick_switch?.position ?? 'mouse'} onChange={event => {
                if (document.value) { document.value.quick_switch ??= {}; document.value.quick_switch.position = (event.target as HTMLSelectElement).value }
              }}><option value="screen">{t("当前屏幕中心")}</option><option value="window">{t("当前窗口中心")}</option><option value="mouse">{t("鼠标模式提示下方")}</option></select></label>
              <label hidden={selectedTab.value !== 'behavior'} data-config-path="quick_switch.blacklist">{t("面板黑名单（逗号分隔）")}<input value={(effectiveDocument.value?.quick_switch?.blacklist ?? ['idle']).join(', ')}
                onChange={event => { if (document.value) { document.value.quick_switch ??= {}; document.value.quick_switch.blacklist = (event.target as HTMLInputElement).value.split(',').map(value => value.trim()).filter(Boolean) } }} /></label>
              {(['font_size', 'border_width', 'border_radius', 'padding_x', 'padding_y'] as const).map(field => <label hidden={selectedTab.value !== 'appearance'} data-config-path={`quick_switch.ui.${field}`} key={field}>{{ font_size: t("字号"), border_width: t("边框宽度"), border_radius: t("圆角（-1 自动）"), padding_x: t("左右内边距"), padding_y: t("上下内边距") }[field]}
                <input type="number" min={field === 'font_size' ? 1 : field === 'border_width' ? 0 : -1} max="200"
                  value={effectiveDocument.value?.quick_switch?.ui?.[field] ?? (field === 'font_size' ? 28 : field === 'border_width' ? 1 : field === 'padding_x' ? 10 : -1)}
                  onChange={event => { const value = Number((event.target as HTMLInputElement).value); if (document.value && Number.isInteger(value)) { document.value.quick_switch ??= {}; document.value.quick_switch.ui ??= {}; document.value.quick_switch.ui[field] = value } }} />
              </label>)}
              {(['background_color', 'text_color', 'border_color'] as const).map(field => <label hidden={selectedTab.value !== 'appearance'} data-config-path={`quick_switch.ui.${field}`} key={field}>{{ background_color: t("背景颜色"), text_color: t("文字颜色"), border_color: t("边框颜色") }[field]}
                <input placeholder={t("#RRGGBBAA · 留空继承主题")} value={typeof effectiveDocument.value?.quick_switch?.ui?.[field] === 'string' ? effectiveDocument.value.quick_switch.ui[field] : ''}
                  onChange={event => { const value = (event.target as HTMLInputElement).value.trim(); if (document.value && (!value || /^#[0-9a-f]{8}$/i.test(value))) { document.value.quick_switch ??= {}; document.value.quick_switch.ui ??= {}; if (value) document.value.quick_switch.ui[field] = value; else delete document.value.quick_switch.ui[field] } }} />
              </label>)}
              </div></div>
            </details>

    )
    const renderUsage = () => (
            <section class="ks-utility-card ks-usage-settings">
              <header class="ks-utility-heading"><h2>{t("模式使用统计")}</h2><span>{usageRanking.value.length}{t(" 个模式")}</span></header>
              <div class="ks-settings-body">
              <p>{t("来自导入的 workspace.ksw，按进入次数排序；模拟操作不会改变实际使用统计。")}</p>
              <label data-config-path="mode_usage.save_after_entries">{t("累计进入次数后保存 ")}<input type="number" min="1" max="4294967295"
                value={Number(effectiveDocument.value?.mode_usage?.save_after_entries ?? 100)}
                onChange={(event) => {
                  const count = Number((event.target as HTMLInputElement).value)
                  if (document.value && Number.isInteger(count) && count >= 1 && count <= 4294967295) {
                    document.value.mode_usage = { save_after_entries: count }
                  }
                }} /></label>
              {!usageRanking.value.length && <div class="ks-usage-empty"><strong>{t("暂无使用记录")}</strong><span>{t("导入 workspace.ksw 后，在这里查看常用模式排名。")}</span></div>}
              <table class="ks-usage-table" hidden={!usageRanking.value.length}><thead><tr><th>{t("常用模式")}</th><th>{t("进入次数")}</th></tr></thead><tbody>
                {usageRanking.value.map(([mode, count], index) => <tr key={mode}><td><span class="ks-usage-rank">{index + 1}</span>{mode}<progress max="100" value={Number(BigInt(count) * 100n / (BigInt(usageRanking.value[0]?.[1] ?? '1') || 1n))} /></td><td>{BigInt(count).toLocaleString()}</td></tr>)}
              </tbody></table>
              </div>
            </section>

    )
    const renderFiles = () => (<div class="ks-file-settings">
            <section class="ks-utility-card">
            <div class="ks-toolbar ks-compact-toolbar ks-preview-toolbar">
              <div class="ks-toml-source">
                <span class="ks-source-kicker">{t("配置源")}</span>
                <h2>{sourceName.value}</h2>
                <p>{t(message.value)}</p>
                <div class="ks-source-metrics">
                  <span>{sourceStats.value.sections}{t(" 个顶层配置段")}</span>
                  <span>{sourceStats.value.values}{t(" 个值")}</span>
                  <span>{formatBytes(sourceStats.value.bytes)}</span>
                </div>
              </div>
              <div class="ks-source-actions">
                <button class="ks-button" onClick={() => importInput.value?.click()}>{t("导入 TOML")}</button>
                <button class="ks-button" onClick={downloadDefault}>{t("默认配置")}</button>
                <button class="ks-button ks-button-primary" onClick={downloadConfig}>{t("下载用户配置")}</button>
                <button class="ks-button" onClick={copyToml}>{t("复制")}</button>

              </div>
            </div>
            <div class="ks-toml-sync-note">
              <strong>{t("与程序默认配置同步")}</strong>
              <p>{t("页面构建前会从仓库根目录复制 ")}<code>keysteer.default.toml</code>{t("。导入局部配置时，预览按 Rust 缺省规则补全，下载仍保持局部文件。")}</p>
              <small>{t("浏览器会验证 TOML 结构，但不会替代 ")}<code>keysteer --check</code>{t("；解析后注释不会保留。")}</small>
            </div>
            </section>
            <section class="ks-utility-card ks-workspace-files"><header class="ks-utility-heading"><h2>{t("工作区文件")}</h2><code>workspace.ksw</code></header><div class="ks-settings-body"><p>{t("导入或导出布局、标签分组与使用统计。")}</p><div class="ks-file-actions"><input aria-label={t("导入工作区文件")} type="file" accept=".ksw,application/octet-stream" onChange={importLayoutFile} /><button class="ks-button" onClick={downloadLayouts}>{t("下载工作区文件")}</button></div></div></section></div>)
    return () => (
      <div class="ks-studio ks-classified" style={{ '--ks-navigation-width': `${navigationWidth.value}px`, '--ks-settings-share': `${settingsShare.value}fr`, '--ks-preview-share': `${100 - settingsShare.value}fr` }}>
        <header class="ks-studio-topbar" inert={expanded.value}>
          <div><strong>{t("配置工作台")}</strong><small role="status" aria-live="polite" title={t(message.value)}>{t(message.value || sourceName.value)}</small></div>
          <div class="ks-settings-search"><input aria-label={t("搜索设置")} placeholder={t("搜索设置、模式或 TOML 路径…")} value={searchQuery.value} onInput={e => searchQuery.value = (e.target as HTMLInputElement).value} />
            {searchQuery.value.trim() && <div class="ks-search-results">{searchResults.value.map(entry => <button onClick={() => locateField(entry)}><strong>{t(entry.label)}</strong><small>{entry.path}</small></button>)}{!searchResults.value.length && <p>{t("没有匹配设置")}</p>}</div>}
          </div>
          <button onClick={() => importInput.value?.click()}>{t("导入 TOML")}</button><button class="ks-button-primary" onClick={downloadConfig}>{t("下载用户配置")}</button>
          <input ref={importInput} type="file" hidden accept=".toml,text/plain" onChange={onImport} />
        </header>
        <SettingsNavigation mobile page={pageId.value} disabled={expanded.value} onSelect={selectPage} />
        <div inert={expanded.value} class="ks-mobile-view">{['settings', 'preview'].map(view => <button aria-pressed={mobileView.value === view} onClick={() => { releasePreview(); mobileView.value = view }}>{view === 'settings' ? t("设置") : t("预览")}</button>)}</div>
        <div class="ks-workspace" data-mobile-view={mobileView.value}>
          <SettingsNavigation page={pageId.value} disabled={expanded.value} onSelect={selectPage} />
          <PaneDivider navigation value={navigationWidth.value} disabled={expanded.value} onStart={releasePreview} onChange={value => navigationWidth.value = value} />
          <section class="ks-settings-pane" ref={settingsPane} tabindex="-1" aria-label={t("设置编辑区")} inert={expanded.value}>
            <div class="ks-settings-heading"><h1>{t(currentPage.value.label)}</h1>{currentPage.value.mode && <code>{currentPage.value.mode}</code>}
              {currentPage.value.tabs.length > 0 && <div class="ks-settings-tabs" aria-label={t("设置类型")}>{currentPage.value.tabs.map(tab => <button aria-pressed={selectedTab.value === tab} onClick={() => { releasePreview(); selectedTab.value = tab }}>{t(tabLabels[tab])}</button>)}</div>}
            </div>
            {selectedTab.value === 'keys' && currentPage.value.mode && renderKeyboard()}
            {document.value && effectiveDocument.value && selectedTab.value !== 'keys' && <>
              {['normal', 'grid', 'recursive_grid', 'ui_hint'].includes(pageId.value) && <CommonConfigControls page={pageId.value} tab={selectedTab.value} document={document.value} effectiveDocument={effectiveDocument.value} onChange={next => document.value = next} />}
              {(pageId.value.startsWith('window') || ['grid', 'recursive_grid', 'ui_hint', 'key_help'].includes(pageId.value)) && <ModeStyleControls page={pageId.value} tab={selectedTab.value} mode={(pageId.value === 'window_card' ? 'window' : pageId.value) as any} appearance={appearance.value} document={document.value} effectiveDocument={effectiveDocument.value} onChange={next => document.value = next} onAppearanceChange={next => appearance.value = next} />}
            </>}
            {pageId.value === 'quick_switch' && renderQuickSwitch()}
            {pageId.value === 'mode_usage' && renderUsage()}
            {pageId.value === 'files' && renderFiles()}
            {pageId.value === 'toml' && <div><button onClick={copyToml}>{t("复制 TOML")}</button><p>{t("下载保持局部配置；解析后不保留注释。")}</p><pre class="ks-toml"><code innerHTML={highlightToml(tomlPreview.value)} /></pre></div>}
          </section>
          <PaneDivider value={settingsShare.value} disabled={expanded.value} onStart={releasePreview} onChange={value => settingsShare.value = value} />
          <aside ref={previewPane} class={{ 'ks-preview-pane': true, 'is-expanded': expanded.value }} role={expanded.value ? 'dialog' : 'complementary'} aria-modal={expanded.value ? true : undefined} aria-label={t("实时预览")} {...{ onKeydownCapture: previewDialogKey }}>
            <div class="ks-preview-context"><span>{t("编辑：")}{t(currentPage.value.label)}</span><button onClick={() => { releasePreview(); setPreviewMode((pageId.value === 'window_card' ? 'window' : currentPage.value.mode && currentPage.value.mode !== 'hotkeys' ? currentPage.value.mode : 'normal') as any) }}>{t("返回当前编辑模式")}</button>
              <select aria-label={t("预览主题")} value={appearance.value} onChange={e => appearance.value = (e.target as HTMLSelectElement).value as Appearance}><option value="light">{t("浅色")}</option><option value="dark">{t("深色")}</option></select>
            </div>
            {renderPreview()}
          </aside>
        </div>
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
    const { t } = useStudioI18n()
    return () => {
      const cols = numberSetting(props.settings.grid_cols, props.mode === 'grid' ? 5 : 3)
      const rows = numberSetting(props.settings.grid_rows, props.mode === 'grid' ? 4 : 3)
      const keys = String(props.settings.keys ?? '')
      const cells = Array.from({ length: cols * rows }, (_, index) => keys[index] ?? '·')
      const region = gridRegion(cols, rows, keys, props.path)
      const labelChar = String(props.settings.ui?.label_char ?? '')
      const previewSecondLayer = props.mode === 'grid'
        && props.path.length === 0
        && numberSetting(props.settings.max_depth, 3) > 1
      return (
        <div
          class={{
            'ks-target-grid': true,
            recursive: props.mode === 'recursive_grid',
            'auto-font': props.mode === 'recursive_grid' && Number(props.settings.ui?.font_size ?? 0) === 0,
            'with-second-layer-preview': previewSecondLayer,
          }}
          style={{
            inset: 'auto',
            left: `${region.x}%`, top: `${region.y}%`, width: `${region.width}%`, height: `${region.height}%`,
            overflow: 'hidden',
            gridTemplateColumns: `repeat(${cols}, minmax(0, 1fr))`,
            gridTemplateRows: `repeat(${rows}, minmax(0, 1fr))`,
          }}
        >
          {cells.map((label, index) => {
            const cellClass = {
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
    const { t } = useStudioI18n()
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
    const { t } = useStudioI18n()
    return () => (
      <div class="ks-desktop-backdrop" aria-hidden="true">
        <div class="ks-desktop-window">
          <div class="ks-desktop-titlebar">
            <div class="ks-window-controls"><i /><i /><i /></div>
            <div class="ks-address-bar">{t("工作台 / 今日概览")}</div>
            <div class="ks-avatar">KS</div>
          </div>
          <div class="ks-desktop-body">
            <aside>
              <strong>KeySteer</strong>
              <span class="active">{t("概览")}</span><span>{t("项目")}</span><span>{t("日历")}</span><span>{t("收件箱")}</span><span>{t("设置")}</span>
            </aside>
            <main>
              <header><div><strong>{t("上午好")}</strong><small>{t("这里是今天需要处理的内容")}</small></div><button>{t("新建项目")}</button></header>
              <div class="ks-desktop-search">{t("搜索项目、任务或联系人…")}</div>
              <div class="ks-desktop-stats"><div><small>{t("进行中")}</small><b>12</b></div><div><small>{t("本周完成")}</small><b>28</b></div><div><small>{t("待处理")}</small><b>7</b></div></div>
              <section class="ks-desktop-content">
                <div class="ks-desktop-list"><strong>{t("最近任务")}</strong><p><i />{t("完成发布说明 ")}<button>{t("打开")}</button></p><p><i />{t("检查界面标注 ")}<button>{t("查看")}</button></p><p><i />{t("整理下周计划 ")}<button>{t("编辑")}</button></p></div>
                <div class="ks-desktop-panel"><strong>{t("进度")}</strong><div class="ks-progress-ring">72%</div><small>{t("本周目标")}</small></div>
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
    const { t } = useStudioI18n()
    return () => (
      <div class={{ 'ks-key-section': true, compact: props.compact }}>
        {props.rows.map((row) => (
          <div class="ks-key-row">
            {row.map((spec) => spec.key ? (() => {
              const renderKey = (layer: KeySpec) => {
                const binding = props.bindingInfo(layer)
                return <button
                  style={{ '--key-width': String(layer.width ?? 1) }}
                  class={{
                    selected: props.selected === layer.key || (!layer.literal && props.selected.endsWith(`+${layer.key}`)),
                    bound: Boolean(binding),
                    [`tone-${binding?.tone ?? 'none'}`]: Boolean(binding),
                  }}
                  aria-label={layer.literal ? t("符号 {0}", [layer.label]) : t("按键 {0}", [layer.label])}
                  data-map-key={layer.key} data-literal={Boolean(layer.literal)}
                  title={binding ? `${layer.key}: ${binding.text}` : layer.key}
                  onClick={() => props.onKey(layer)}
                >
                  <span>{layer.label}</span>
                  {binding && <small>{binding.text}</small>}
                </button>
              }
              return spec.shifted ? <div class="ks-dual-key" style={{ '--key-width': String(spec.width ?? 1) }}>
                {renderKey({ key: spec.shifted, label: spec.shifted, literal: true })}
                {renderKey(spec)}
              </div> : renderKey(spec)
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
    const { t } = useStudioI18n()
    return () => (
      <div class="ks-numpad" aria-label={t("数字小键盘")}>
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
              data-map-key={spec.key} aria-label={t("按键 {0}", [spec.label])}
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
  // US keycap legends only; binding parsing and input use literal characters.
  const base = [...'`1234567890-=[]\\;\u0027,./']
  const shifted = [...'~!@#$%^&*()_+{}|:\u0022<>?']
  return { key: keyName, label, width, shifted: shifted[base.indexOf(keyName)] }
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
  // Physical key identity is separate from KeyboardEvent.key's actual text.
  const punctuation: Record<string, string> = {
    Slash: '/', Semicolon: ';', Quote: "'", Backslash: '\\', BracketLeft: '[', BracketRight: ']',
    Backquote: '`', Equal: '=', Minus: '-', Comma: ',', Period: '.',
  }
  if (punctuation[code]) return punctuation[code]
  if (/^Key[A-Z]$/.test(code)) return code.slice(3).toLowerCase()
  if (/^Digit[0-9]$/.test(code)) return code.slice(5)

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
  const card = document?.window?.card ?? {}
  const numberSize = numberSetting(ui.font_size, 28)
  const appSize = numberSetting(card.app_font_size, 0) || Math.max(numberSize * .6, 14)
  const titleSize = numberSetting(card.title_font_size, 0) || Math.max(numberSize * .45, 12)
  return {
    '--ks-card-app-size': `${appSize}px`,
    '--ks-card-background': themedColor(card.background_color, labelBackground),
    '--ks-card-border': themedColor(card.border_color, configuredBorder),
    '--ks-card-number-color': themedColor(card.number_color, themedColor(ui.text_color, text)),
    '--ks-card-title-size': `${titleSize}px`,
    '--ks-card-app-font': String(card.app_font_family || ui.font_family || 'var(--vp-font-family-base)'),
    '--ks-card-title-font': String(card.title_font_family || ui.font_family || 'var(--vp-font-family-base)'),
    '--ks-card-app-weight': card.app_bold === false ? '400' : '700',
    '--ks-card-title-weight': card.title_bold ? '700' : '400',
    '--ks-card-app-color': themedColor(card.app_color, themedColor(ui.text_color, text)),
    '--ks-card-title-color': themedColor(card.title_color, themedColor(ui.text_color, text)),
    '--ks-card-width': `${numberSetting(card.text_width, 260)}px`,
    '--ks-card-padding-x': `${finiteSetting(card.padding_x, 9)}px`,
    '--ks-card-padding-y': `${finiteSetting(card.padding_y, 4)}px`,
    '--ks-card-row-height': `${Math.max(appSize, titleSize) * numberSetting(card.line_height, 1.4)}px`,
    '--ks-card-min-height': `${finiteSetting(card.min_height, 44)}px`,
    '--ks-card-number-width': `${Math.max(finiteSetting(card.number_min_width, 38), numberSize * .75 + autoSetting(ui.padding_x, Math.round(numberSize * .4)) * 2)}px`,
    '--ks-card-radius': `${autoSetting(ui.border_radius, Math.round(numberSize * .35))}px`,
    '--ks-card-number-padding-y': `${autoSetting(ui.padding_y, Math.round(numberSize * .2))}px`,
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
  if (action.startsWith('move_window ')) return 'utility'
  if (action.startsWith('move_')) return 'move'
  if (['precision', 'slow', 'fast', 'precision_toggle', 'slow_toggle', 'fast_toggle'].includes(action)) return 'speed'
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
    mouse_x1: '侧键 1', mouse_x2: '侧键 2',
    grid: 'Grid', recursive_grid: '递归 Grid', ui_hint: 'UI Hint', normal: 'Normal', idle: 'Idle',
    precision: '精确', slow: '慢速', fast: '快速', finish: '完成', restart_mode: '重启', escape: '返回',
    precision_toggle: '切换精确', slow_toggle: '切换慢速', fast_toggle: '切换快速',
    'move_window previous': '窗口上一屏', 'move_window prev': '窗口上一屏', 'move_window next': '窗口下一屏',
  }
  const windowTarget = /^move_window\s+(\d+)$/.exec(action)
  if (windowTarget) return `窗口→屏幕 ${windowTarget[1]}`
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

function downloadLayoutBinary(bytes: Uint8Array): void {
  const buffer = new ArrayBuffer(bytes.byteLength); new Uint8Array(buffer).set(bytes)
  const url = URL.createObjectURL(new Blob([buffer], { type: 'application/octet-stream' }))
  const anchor = document.createElement('a'); anchor.href = url; anchor.download = WORKSPACE_FILE_NAME; anchor.click(); URL.revokeObjectURL(url)
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

const KeyHelpPreview = defineComponent({
  emits: ['restore'],
  props: {
    isMac: { type: Boolean, default: false },
    document: { type: Object as () => ConfigDocument, required: true },
    mode: { type: String, required: true },
    appearance: { type: String as () => 'light' | 'dark', required: true },
    anchor: { type: Object as () => { x: number; y: number; width: number; height: number } },
    detail: { type: String, default: '' },
    status: { type: String, default: '' },
    windowState: { type: Object as () => WindowState },
  },
  setup(props, { emit }) {
    const { t } = useStudioI18n()
    const host = ref<HTMLElement>()
    const size = ref({ width: 800, height: 450 })
    let observer: ResizeObserver | undefined
    onMounted(() => {
      observer = new ResizeObserver(([entry]) => {
        size.value = { width: entry.contentRect.width, height: entry.contentRect.height }
      })
      if (host.value) observer.observe(host.value)
    })
    onBeforeUnmount(() => observer?.disconnect())
    const entries = computed(() => keyHelpEntries(props.document, props.mode, props.isMac))
    return () => {
      if (!props.document.key_help) return null
      const ui = keyHelpStyle(props.document.key_help)
      const isWindow = isWindowMode(props.mode)
      if (isWindow) ui.title_font_size = ui.font_size * 1.75
      const preview = props.mode === 'window_quick' && props.windowState ? quickRect(props.windowState.quick, props.windowState.ratios) : null
      const previewHeight = 0
      const target = props.windowState ? windowTarget(props.windowState) : null
      const library = props.windowState?.library ? props.windowState : undefined
      const presets = library ? availableWindowPresets(library).slice(library.libraryPage * 6, library.libraryPage * 6 + 6) : []
      const tabs = props.mode === 'window_tab' ? props.windowState : undefined
      const tabTarget = tabs?.tabs.target
      const tabGroup = tabTarget?.kind === 'group' ? tabs?.tabs.groups.find(g => g.id === tabTarget.id) : undefined
      const info = library ? [`${library.deletingPresets ? '输入编号选择要删除的预设' : '输入预设编号恢复'} · 第 ${library.libraryPage + 1} / ${Math.max(1, Math.ceil(library.presets.length / 6))} 页`,
        ...(presets.length ? presets.map(preset => `${preset.id}   ${presetName(preset)}`) : ['尚未保存预设，请先保存布局或标签组。'])]
        : tabs ? tabs.tabs.restore ? ['按标签顺序选择窗口，选满后自动套用'] : [tabGroup ? `成员：${tabGroup.members.map(id => tabs.numbers[id]).join('、')}` : tabTarget ? `起点：窗口 ${tabs.numbers[tabTarget.id]}` : '输入窗口编号开始组合', '输入编号加入 · 结束本轮后开始下一组']
        : isWindow && target ? [`${target.app}  \u00b7  ${t(target.title)}`, ...(target.minimized ? ['已最小化 · 再次循环可还原'] : [])] : []
      const detailParts = props.detail.split(' · ')
      const detailBadge = detailParts[0]
      if (isWindow && detailParts.length > 1) info.unshift(detailParts.slice(1).map(text => t(text)).join(' · '))
      const inputValue = isWindow ? props.status.match(/^(?:Input:|输入：)\s*(.*)$/)?.[1] : undefined
      const prompt = inputValue === undefined ? '>' : `> ${inputValue}`
      const promptHeight = isWindow && inputValue !== undefined ? ui.font_size * 2.3 : 0
      const headerGap = isWindow ? 12 : 0
      const statusLines = [...info, ...(props.status && inputValue === undefined ? [props.status] : [])].map(text => t(text))
      const statusHeight = statusLines.length * ui.font_size * 1.8 + promptHeight + headerGap
      const anchorWidth = props.anchor ? props.anchor.width / WINDOW_AREA.width * size.value.width : size.value.width
      const anchorHeight = props.anchor ? props.anchor.height / WINDOW_AREA.height * size.value.height : size.value.height
      const inside = false // Anchors affect placement, never font sizing.
      const helpWidth = isWindow ? Math.min(size.value.width, 944) : size.value.width
      const helpEntries = [...entries.value]
      if (library) {
        if (library.libraryPage > 0) helpEntries.push({ id: 'Previous page', keys: 'PAGEUP', action: 'Previous page' })
        if ((library.libraryPage + 1) * 6 < availableWindowPresets(library).length) helpEntries.push({ id: 'Next page', keys: 'PAGEDOWN', action: 'Next page' })
      }
      const sections = isWindow ? windowHelpSections(helpEntries, props.mode, props.detail.startsWith('Resize')) : null
      const grid = sections ? windowHelpGrid(sections, helpWidth - ui.screen_margin * 2) : null
      const bodyEntries = grid?.entries ?? entries.value
      const textUnits = (text: string) => [...text].reduce((sum, ch) => sum + (ch.codePointAt(0)! < 128 ? .75 : 1), 0)
      const minimumContentWidth = isWindow ? Math.max(textUnits(prompt) * ui.font_size * 1.65,
        ...statusLines.map(text => textUnits(text ?? '') * ui.font_size),
        ui.font_size * 1.75 * 5 + [...detailBadge].length * ui.font_size * .75 + 16
          + [...(sections?.exit ?? '')].length * ui.font_size * .75 + 4
          + textUnits(sections?.exitLabel ?? '') * ui.font_size + 2 + ui.key_gap * 3,
      ) : 0
      let layout = keyHelpLayout(ui, bodyEntries, helpWidth, inside ? anchorHeight : size.value.height, previewHeight + statusHeight, isWindow, grid?.columns, sections?.exit ?? '', sections?.modes ?? [], minimumContentWidth)
      const rulerY = layout.rowHeights.slice(0, layout.rows).reduce((a, b) => a + b, 0) + 8
      const ruler = preview && props.windowState ? quickRulerPlan(props.windowState.ratios, props.windowState.ratioLabels, preview,
        layout.widths[0].key + ui.key_gap + layout.widths[0].action, ui.font_size,
        Math.max(88, Math.min(150, layout.bodyHeight - rulerY)), WINDOW_AREA.width / WINDOW_AREA.height) : null
      if (ruler) {
        const extra = Math.max(0, rulerY + ruler.height - layout.bodyHeight)
        layout.bodyHeight += extra; layout.height += extra
        layout.displayScale = Math.min(1, (helpWidth - ui.screen_margin * 2) / layout.width, (size.value.height - ui.bottom_margin) / layout.height)
      }
      if (isWindow) {
        const bottomPadding = Math.max(0, headerGap / 2 - 1 - ui.font_size * .1)
        layout.bodyHeight += bottomPadding; layout.height += bottomPadding
        layout.displayScale = Math.min(1, (helpWidth - ui.screen_margin * 2) / layout.width, (size.value.height - ui.bottom_margin) / layout.height)
      }
      const theme = props.document.theme?.[props.appearance] ?? {}
      const indicator = { ...props.document.mode_indicator?.ui, ...props.document.mode_indicator?.modes?.[props.mode]?.ui }
      const color = (value: unknown, fallback: string): string => {
        const raw = typeof value === 'object' && value ? (value as ConfigDocument)[props.appearance] : value
        return typeof raw === 'string' && /^#[\da-f]{8}$/i.test(raw) ? raw : fallback
      }
      const derivedBackground = color(indicator.background_color, theme.accent ?? '#465FBCFF')
      const background = color(ui.background_color, derivedBackground)
      const foreground = color(ui.text_color, color(indicator.text_color, readable(derivedBackground, theme.text, theme.on_accent_alt)))
      const close = sections?.exit ?? entries.value.filter(entry => entry.action === 'key_help').map(entry => entry.keys).join(' / ')
      const closeText = isWindow ? close : close ? `${close}  Close` : ''
      const closeWidth = isWindow ? layout.exitWidth + (close ? [...(sections?.exitLabel ?? '')].reduce((sum, ch) => sum + (ch.codePointAt(0)! < 128 ? .75 : 1), 0) * ui.font_size + 2 + ui.key_gap : 0) : Math.min(closeText.length * ui.title_font_size * 0.75, layout.width * 0.45)
      const titleStyle = { fontSize: `${ui.title_font_size}px`, fontWeight: ui.title_bold ? '700' : '400', color: color(ui.title_color, foreground),
        height: `${layout.headerHeight}px`, display: 'flex', alignItems: 'center', overflow: 'hidden', whiteSpace: 'nowrap' as const }
      const scale = layout.bodyScale
      const fitsAnchor = props.anchor && anchorWidth >= layout.width + 16 && anchorHeight >= layout.height + 16
      const left = fitsAnchor ? Math.max(0, Math.min(size.value.width - layout.width,
        (props.anchor.x + props.anchor.width / 2) / WINDOW_AREA.width * size.value.width - layout.width / 2)) : (size.value.width - layout.width) / 2
      const top = fitsAnchor ? Math.max(0, Math.min(size.value.height - layout.height - 8,
        (props.anchor.y + props.anchor.height) / WINDOW_AREA.height * size.value.height - layout.height - 8)) : size.value.height - layout.height - ui.bottom_margin
      return (
        <div ref={host} style={{ position: 'absolute', inset: 0, pointerEvents: 'none', zIndex: 20, overflow: 'hidden' }}>
          <div aria-label={isWindow ? t("Window 操作提示") : t("按键提示预览")} style={{ position: 'absolute', left: `${left}px`, top: `${top}px`,
            width: `${layout.width}px`, height: `${layout.height}px`, transform: `scale(${layout.displayScale})`, transformOrigin: 'center bottom', background, borderRadius: `${ui.border_radius}px`,
            boxShadow: `inset 0 0 0 ${ui.border_width}px ${color(ui.border_color, '#00000000')}`,
            fontFamily: ui.font_family || indicator.font_family || 'system-ui, sans-serif', lineHeight: 1.4, color: foreground }}>
            {isWindow && <>

              <div style={{ position: 'absolute', left: `${layout.padding}px`, top: `${ui.padding_y + layout.headerHeight + statusHeight - headerGap / 2}px`, width: `${layout.width - layout.padding * 2}px`, height: '1px', background: foreground, opacity: .18 }} />
            </>}
            <div style={{ ...titleStyle, position: 'absolute', gap: '12px', left: `${layout.padding}px`, top: `${ui.padding_y}px`, width: `${Math.max(1, layout.width - layout.padding * 2 - closeWidth - ui.key_gap)}px` }}>{isWindow ? <><span>Window</span><span style={{ fontSize: `${ui.font_size}px`, padding: '2px 6px', background: '#ebedf2', color: '#1e222b', border: '1px solid #c4c9d3', borderRadius: '3px' }}>{detailBadge}</span></> : `${props.mode} · Available keys`}</div>
            <div style={{ ...titleStyle, position: 'absolute', right: `${layout.padding}px`, top: `${ui.padding_y}px`, width: `${closeWidth}px`, justifyContent: 'flex-end', fontSize: `${isWindow ? ui.font_size : ui.title_font_size}px` }}>{isWindow ? <><span aria-label={sections?.exitLabel} style={{ display: 'flex', flexDirection: 'column', alignItems: 'flex-end' }}>{layout.exitLines.map(line => <span style={{ padding: '1px 2px', background: '#ebedf2', color: '#1e222b', border: '1px solid #c4c9d3', borderRadius: '3px', margin: '1px 0' }}>{line}</span>)}</span><span style={{ marginLeft: `${ui.key_gap}px` }}>{sections?.exitLabel}</span></> : closeText}</div>
            {promptHeight > 0 && <div style={{ position: 'absolute', left: `${layout.padding}px`, top: `${ui.padding_y + layout.headerHeight}px`, width: `${layout.width - layout.padding * 2}px`, height: `${promptHeight}px`, display: 'flex', alignItems: 'center', fontSize: `${ui.font_size * 1.65}px`, fontWeight: 700, whiteSpace: 'nowrap' }}>{prompt}</div>}
            {statusLines.map((text, index) => <div onMousedown={e => e.preventDefault()} onClick={() => { if (library && presets[index - 1]) emit('restore', presets[index - 1].id) }} style={{ position: 'absolute', left: `${layout.padding}px`, top: `${ui.padding_y + layout.headerHeight + promptHeight + index * ui.font_size * 1.8}px`, width: `${layout.width - layout.padding * 2}px`, height: `${ui.font_size * 1.8}px`, fontSize: `${ui.font_size}px`, fontWeight: index === 0 && info.length ? '700' : '400', overflow: 'hidden', textOverflow: isWindow ? undefined : 'ellipsis', whiteSpace: 'nowrap', opacity: index === 0 ? .9 : .72, pointerEvents: library && presets[index - 1] ? 'auto' : undefined, cursor: library && presets[index - 1] ? 'pointer' : undefined }}>{text}</div>)}
            {ruler && <div aria-label={t("Quick 屏幕布局预览")} style={{ position: 'absolute', left: `${layout.bodyLeft}px`, top: `${ui.padding_y + layout.headerHeight + statusHeight + rulerY}px`, width: `${layout.widths[0].key + ui.key_gap + layout.widths[0].action}px`, height: `${ruler.height}px`, fontSize: `${ruler.font}px` }}>
              {[ruler.frame, ruler.selection].map((rect, index) => <span style={{ position: 'absolute', left: `${rect.x}px`, top: `${rect.y}px`, width: `${rect.width}px`, height: `${rect.height}px`, boxShadow: `inset 0 0 0 ${index ? 1.5 : 1}px ${foreground}`, opacity: index ? .9 : .4 }}><span style={{ position: 'absolute', inset: 0, background: foreground, opacity: index ? .2 : .075 }} /></span>)}
              {ruler.ticks.map(tick => <span style={{ position: 'absolute', left: `${tick.x}px`, top: `${tick.y}px`, width: `${tick.width}px`, height: `${tick.height}px`, background: foreground, opacity: tick.active ? .9 : .35 }} />)}
              {ruler.captions.map(caption => <span style={{ position: 'absolute', left: `${caption.x}px`, top: `${caption.y}px`, fontWeight: 700, whiteSpace: 'nowrap' }}>{caption.text}</span>)}
            </div>}
            {!!layout.footer.length && <div style={{ position: 'absolute', left: `${layout.padding}px`, top: `${ui.padding_y + layout.headerHeight + statusHeight + previewHeight + layout.bodyHeight}px`, width: `${layout.width - layout.padding * 2}px`, height: '1px', background: foreground, opacity: .18 }} />}
            {layout.footer.map(item => <div style={{ position: 'absolute', left: `${layout.padding + item.x}px`, top: `${ui.padding_y + layout.headerHeight + statusHeight + previewHeight + layout.bodyHeight + 4 + item.y}px`, height: `${layout.footerLineHeight}px`, display: 'flex', alignItems: 'center', gap: `${ui.key_gap}px`, fontSize: `${ui.font_size}px` }}>
              <span style={{ padding: '1px 2px', fontWeight: 700, background: '#ebedf2', color: '#1e222b', borderRadius: '3px', boxShadow: 'inset 0 0 0 1px #c4c9d3' }}>{item.keys}</span><span style={{ opacity: ui.text_opacity }}>{item.action}</span>
            </div>)}
            {bodyEntries.map((entry, index) => {
              const column = Math.floor(index / layout.rows)
              const widths = layout.entryWidths[index]
              const left = layout.bodyLeft + layout.widths.slice(0, column).reduce((sum, w) => sum + w.key + w.action + ui.key_gap + (isWindow ? 20 : ui.column_gap), 0)
              const top = ui.padding_y + layout.headerHeight + statusHeight + previewHeight + layout.rowHeights.slice(column * layout.rows, index).reduce((a, b) => a + b, 0)
              if (isWindow && !entry.keys) return entry.action ? <div key={`section-${index}`} style={{ position: 'absolute', left: `${left}px`, top: `${top}px`, height: `${layout.rowHeights[index]}px`, display: 'flex', alignItems: 'center', fontSize: `${ui.font_size}px`, fontWeight: 700, opacity: .65 }}>{entry.action}</div> : null
              return <div key={`${entry.keys}-${index}`} style={{ position: 'absolute', left: `${left}px`, top: `${top}px`, display: 'flex', alignItems: 'center',
                height: `${Math.max(1, layout.rowHeights[index] - ui.row_gap)}px`, gap: `${ui.key_gap}px`, fontSize: `${ui.font_size * scale}px`, whiteSpace: 'nowrap' }}>
                <span style={{ width: `${widths.key}px`, display: 'flex', flexDirection: 'column', alignItems: 'flex-end' }}>
                  {layout.keyLines[index].map(line => <span style={{ fontWeight: ui.key_bold ? '700' : '400', color: color(ui.key_text_color, '#1E222BFF'), background: color(ui.key_background_color, '#EBEDF2FF'),
                    padding: `${ui.key_padding_y * scale}px ${(isWindow ? 2 : ui.key_padding_x) * scale}px`, margin: isWindow ? '1px 0' : undefined, borderRadius: `${ui.key_border_radius * scale}px`,
                    boxShadow: `inset 0 0 0 ${ui.key_border_width * scale}px ${color(ui.key_border_color, '#C4C9D3FF')}` }}>{line}</span>)}
                </span>
                <span style={{ width: `${widths.action}px`, whiteSpace: 'nowrap', opacity: ui.text_opacity, fontWeight: ui.text_bold ? '700' : '400' }}>{layout.actionLines[index].map(line => <span style={{ display: 'block' }}>{line || '\u00a0'}</span>)}</span>
              </div>
            })}
          </div>
        </div>
      )
    }
  },
})

function readable(background: string, text = '#10172DFF', alternate = '#FFFFFFFF'): string {
  const luminance = (hex: string) => {
    const channels = [1, 3, 5].map(offset => parseInt(hex.slice(offset, offset + 2), 16) / 255)
      .map(value => value <= 0.04045 ? value / 12.92 : ((value + 0.055) / 1.055) ** 2.4)
    return channels[0] * 0.2126 + channels[1] * 0.7152 + channels[2] * 0.0722
  }
  const bg = luminance(background)
  const contrast = (color: string) => (Math.max(bg, luminance(color)) + 0.05) / (Math.min(bg, luminance(color)) + 0.05)
  return contrast(text) >= contrast(alternate) ? text : alternate
}


function windowNumberLabels(state: WindowState, card: Record<string, any>): Array<{ number: number; x: number; y: number; app: string; title: string; members?: string[] }> {
  const [top, right, bottom, left] = cardPositionRatios(card.position)
  const anchorX = (left + 1 - right) / 2
  const anchorY = (top + 1 - bottom) / 2
  const labels: Array<{ number: number; x: number; y: number; app: string; title: string; members?: string[] }> = []
  for (const window of state.windows.map(w => ({ ...w, ...tabFrame(activeTabWindow(state, w.id) ?? w) })).filter(w => (state.includeMinimized || !w.minimized) && w.screen === state.screen && state.numbers[w.id] !== undefined).sort((a, b) => state.numbers[a.id] - state.numbers[b.id])) {
    const group = containingTab(state, window.id)
    if (group && (state.tree ? group.members[0] : group.active) !== window.id) continue
    const slot = state.tree ? treeSlots(state.tree).find(s => s.window === window.id) : undefined
    let x = Math.max(17, Math.min(83, (slot ? slot.rect.x + slot.rect.width * anchorX : (window.x + window.width * anchorX) / WINDOW_AREA.width) * 100))
    let y = Math.max(6, Math.min(94, slot ? (card.position ? (slot.rect.y + slot.rect.height * anchorY) * 100 : slot.rect.y * 100 + 6) : (window.y + window.height * anchorY) / WINDOW_AREA.height * 100))
    if (labels.some(l => Math.abs(l.x - x) < 33 && Math.abs(l.y - y) < 10)) {
      let distance = Infinity
      for (let row = 0; row < 9; row++) for (let col = 0; col < 3; col++) {
        const cx = 17 + col * 33, cy = 6 + row * 10
        if (labels.some(l => Math.abs(l.x - cx) < 33 && Math.abs(l.y - cy) < 10)) continue
        const d = (cx - x) ** 2 + (cy - y) ** 2
        if (d < distance) { distance = d; x = cx; y = cy }
      }
    }
    labels.push({ number: state.numbers[window.id], x, y, app: group ? `~${group.id} · ${group.members.length} 个窗口` : window.app, title: window.title,
      members: group?.members.map(id => {
        const member = state.windows.find(window => window.id === id)
        return `${id === group.active ? '●' : '○'} ${state.numbers[id]} · ${member?.app ?? ''} — ${member?.title ?? ''}`
      }) })
  }
  if (card.position_mode === 'screen') {
    const centers = packedCardCenters(labels.length, 32, 10, [top, right, bottom, left])
    labels.forEach((label, i) => Object.assign(label, centers[i]))
  }
  return labels
}

function keyHelpEntries(document: ConfigDocument, mode: string, isMac = false): HelpEntry[] {
  const groups = new Map<string, string[]>()
  const labelsByAction = new Map<string, string>()
  const returns = new Set<string>()
  const bindings = effectiveBindings(document, mode)
  if (mode === 'window_editor') {
    bindings.set('`', { value: 'window_area_number', source: 'window' })
  }
  for (const [chord, binding] of bindings) {
    if (binding.value === '__disabled__') continue
    let action = Array.isArray(binding.value) ? binding.value.join(' → ') : String(binding.value)
    const id = action
    if (binding.source === mode && id !== mode && (id === 'idle' || modes.some(m => m.id === id)) && (!isWindowMode(id) || id === 'window')) returns.add(id)
    if (isWindowMode(mode)) {
      if (!['window_number', 'window_area_number'].includes(action) && !windowHelpActionSupported(mode, action)) continue
      const labels: Record<string, string> = {
        window_left: 'Left', window_right: 'Right', window_up: 'Up', window_down: 'Down',
        window_size: 'Move / resize', window_layout: 'Quick layout', window_edit: 'Edit layout tree', window_tile: 'Tile windows',
        window_number: '选择窗口编号', window_area_number: '区域编号前缀',
        window_tab_end: 'End group / next group', window_tab_group: 'Choose group number', window_number_end: 'Finish number',
        window_tab_remove: 'Remove active tab', window_tab_dissolve: 'Dissolve group', window_tab_next: 'Next tab', window_tab_previous: 'Previous tab',
        window_tab_move_left: 'Move tab left', window_tab_move_right: 'Move tab right',
        window_screen_next: 'Next screen', window_screen_previous: 'Previous screen', size_cycle: 'Maximize / minimize / restore', window_maximize: 'Maximize / restore', window_minimize: 'Minimize / restore',
        window_center: 'Center window', window_close: 'Close window', window_volume_down: 'App volume down', window_volume_up: 'App volume up', window_volume_mute: 'Mute / unmute app', window_audio_previous: 'Previous app output', window_audio_next: 'Next app output', window_system_volume_down: 'System volume down', window_system_volume_up: 'System volume up', window_system_volume_mute: 'Mute / unmute system', window_system_audio_previous: 'Previous system output', window_system_audio_next: 'Next system output', window_select: 'Next window', window_select_previous: 'Previous window', window_undo: 'Undo', window_redo: 'Redo', window_reset_initial: 'Restore initial state', window_remove_region: 'Delete region', window_saved_layouts: 'Restore layout', window_save_layout: 'Save layout',
        window_confirm: 'Confirm', window: 'Window', window_quick: 'Quick layout', window_editor: 'Editor', window_restore: 'Restore', window_delete: 'Restore / delete', window_tab: 'Tabs', idle: 'Exit', normal: 'Normal', grid: 'Grid', recursive_grid: 'Recursive Grid', ui_hint: 'UI Hint',
      }
      for (const direction of ['left', 'right', 'up', 'down']) {
        labels[`window_layout_${direction}`] = `Layout ${direction}`
        labels[`window_split_${direction}`] = `Split ${direction}`
        labels[`window_ratio_${direction}`] = ({ left: 'Shrink region width', right: 'Grow region width', up: 'Shrink region height', down: 'Grow region height' } as Record<string, string>)[direction]
      }
      action = labels[action] ?? action
    }
    labelsByAction.set(id, action)
    const keys = groups.get(id) ?? []
    keys.push(shortcutCaption(document, chord, isMac))
    groups.set(id, keys)
  }
  return [...groups].map(([id, keys]) => ({ id, returnCandidate: returns.has(id), action: labelsByAction.get(id) ?? id, keys: keys.sort().join(' / ') }))
    .sort((a, b) => a.keys.localeCompare(b.keys))
}

/** Same logical-pixel sizing as the native help decorator (browser font metrics differ). */
function keyHelpLayout(ui: ConfigDocument, entries: HelpEntry[], screenWidth: number, screenHeight: number, extraHeight = 0, windowHelp = false, fixedColumns?: number, exit = '', modes: HelpEntry[] = [], minimumContentWidth = 0) {
  ui = keyHelpStyle(ui)
  if (windowHelp) { ui.title_font_size = ui.font_size * 1.75; ui.column_gap = 20; ui.key_padding_x = 2 }
  const availableWidth = Math.max(1, screenWidth - ui.screen_margin * 2)
  const padding = Math.min(ui.padding_x, availableWidth / 4)
  const columns = fixedColumns ?? (availableWidth >= ui.column_threshold && entries.length > 8 ? Math.max(1, Math.min(ui.max_columns, entries.length)) : 1)
  const rows = Math.max(1, Math.ceil(entries.length / columns))
  const widths = Array.from({ length: columns }, (_, column) => {
    const chunk = entries.slice(column * rows, (column + 1) * rows)
    return {
      key: Math.max(1, ...chunk.map(entry => [...entry.keys].length)) * ui.font_size * 0.75 + ui.key_padding_x * 2,
      action: Math.max(1, ...chunk.map(entry => [...entry.action].length)) * ui.font_size * 0.75 + ui.font_size / 3,
    }
  })
  const textWidth = widths.reduce((sum, column) => sum + column.key + column.action, 0)
  const gaps = (columns - 1) * ui.column_gap + columns * ui.key_gap
  const naturalWidth = Math.max(windowHelp && columns === 2 ? 620 : ui.min_width, Math.max(textWidth + gaps, minimumContentWidth) + padding * 2)
  if (windowHelp && columns > 1 && naturalWidth > availableWidth) return keyHelpLayout(ui, entries, screenWidth, screenHeight, extraHeight, true, 1, exit, modes, minimumContentWidth)
  const width = windowHelp ? naturalWidth : Math.min(availableWidth, naturalWidth)
  const squeeze = Math.max(0.01, Math.min(1, (width - padding * 2 - gaps) / textWidth))
  widths.forEach(column => {
    if (windowHelp) {
      // Keep complete rows at their natural width.
    }
    else { column.key *= squeeze; column.action *= squeeze }
  })
  const entryWidths = entries.map((_, index) => widths[Math.floor(index / rows)])
  const exitWidth = exit ? Math.min([...exit].length * ui.font_size * .75 + ui.key_padding_x * 2, width * .42) : 0
  const exitLines = [exit]
  const headerHeight = Math.max(ui.header_height, ui.title_font_size * 1.4, windowHelp && exit ? exitLines.length * (ui.font_size * 1.4 + 4) : 0)
  const keyLines = entries.map(entry => [entry.keys])
  const actionLines = entries.map(entry => [entry.action])
  const rowHeight = windowHelp ? ui.font_size * 2 : Math.min(ui.row_height, Math.max(8, (screenHeight * ui.max_height_ratio - headerHeight - extraHeight - ui.padding_y * 2) / rows))
  const rowHeights = entries.map((entry, index) => {
    if (!windowHelp) return rowHeight
    if (!entry.keys && !entry.action) return 0
    const lines = Math.max(actionLines[index].length, keyLines[index].length)
    return ui.font_size * (1.4 * lines + .6) + 4 * (lines - 1)
  })
  const bodyHeight = Math.max(0, ...widths.map((_, col) => rowHeights.slice(col * rows, (col + 1) * rows).reduce((a, b) => a + b, 0)))
  const footerLineHeight = ui.font_size * 2.2
  let footerX = 0, footerY = 0
  const footer = modes.map(entry => {
    const keyWidth = [...entry.keys].length * ui.font_size * .75 + ui.key_padding_x * 2
    const actionWidth = [...entry.action].reduce((sum, ch) => sum + (ch.codePointAt(0)! < 128 ? .75 : 1), 0) * ui.font_size + 4
    const itemWidth = keyWidth + ui.key_gap + actionWidth
    if (footerX > 0 && footerX + itemWidth > width - padding * 2) { footerX = 0; footerY += footerLineHeight }
    const item = { ...entry, x: footerX, y: footerY, keyWidth, actionWidth }
    footerX += Math.max((width - padding * 2) / Math.max(1, Math.min(modes.length, 4)), itemWidth + 16)
    return item
  })
  const footerHeight = footer.length ? footerY + footerLineHeight + 4 : 0
  const height = headerHeight + extraHeight + bodyHeight + footerHeight + ui.padding_y * 2
  const bodyScale = windowHelp ? 1 : Math.max(0.01, Math.min(squeeze, (rowHeight - ui.row_gap) / (ui.font_size * 1.4 + ui.key_padding_y * 2)))
  const actualBodyWidth = widths.reduce((sum, column) => sum + column.key + column.action, 0) + gaps
  return { width, height, displayScale: windowHelp ? Math.min(1, availableWidth / width, (screenHeight - ui.bottom_margin) / height) : 1, padding, columns, rows, widths, entryWidths, footer, footerLineHeight, bodyHeight, rowHeight, rowHeights, headerHeight, bodyScale, keyLines, actionLines, exitWidth, exitLines,
    bodyLeft: (width - actualBodyWidth) / 2 }

}

/** Internal presentation values derived from the compact public style block. */
function keyHelpStyle(ui: ConfigDocument): ConfigDocument {
  return { ...ui, title_font_size: ui.font_size * 1.25, title_bold: true, key_bold: true, text_bold: false,
    text_opacity: 0.85, key_border_width: 1, key_border_radius: 3, key_padding_x: 5, key_padding_y: 1,
    column_gap: 6, key_gap: 8, row_gap: 2, row_height: ui.font_size * 2, header_height: ui.font_size * 7 / 3,
    screen_margin: 12, bottom_margin: 12, min_width: 340, max_height_ratio: 0.6, max_columns: 2, column_threshold: 560 }
}
