import { cardPositionRatios } from '../simulator/window-card-position.ts'
import { toRaw } from 'vue'
import { parse, stringify } from 'smol-toml'
import { parseSplitRatios } from '../simulator/window-ratios.ts'

export type ConfigDocument = Record<string, any>

const searchEditActions = new Set([
  'accept', 'cancel', 'paste', 'copy', 'cut', 'select_all',
  'left', 'right', 'home', 'end', 'select_left', 'select_right',
  'select_home', 'select_end', 'backspace', 'delete',
])

/** Import legacy action = keys tables, but keep the editable/exported model key = action. */
export function normalizeSearchEditBindings(value: unknown): Record<string, string> {
  if (!isRecord(value)) throw new Error('ui_hint.search_edit_keys must be a key = action table')
  const entries = Object.entries(value)
  if (entries.every(([, action]) => typeof action === 'string' && searchEditActions.has(action))) return { ...value }
  if (!entries.every(([action, keys]) => searchEditActions.has(action) && typeof keys === 'string')) throw new Error('ui_hint.search_edit_keys requires known editing actions')
  const result: Record<string, string> = {}
  for (const [action, alternatives] of entries) {
    let keys = alternatives as string
    while (keys in result) {
      if (keys.trim()) throw new Error('duplicate search editing key group')
      keys += ' '
    }
    result[keys] = action
  }
  return result
}

function mergeSearchEditBindings(defaults: unknown, configured: unknown): Record<string, string> {
  const base = normalizeSearchEditBindings(defaults ?? {})
  const overrides = normalizeSearchEditBindings(configured)
  const actions = new Set(Object.values(overrides))
  const result = Object.fromEntries(Object.entries(base).filter(([, action]) => !actions.has(action)))
  for (const [key, action] of Object.entries(overrides)) {
    if (key in result && result[key] !== action) throw new Error('search editing keys must be distinct')
    result[key] = action
  }
  return result
}

export interface ParsedConfigDocument {
  document: ConfigDocument
  bytes: number
  sections: number
  values: number
}

// Serde fills missing fields in struct sections, but a configured BTreeMap
// replaces that map as a whole. Keep the browser preview aligned with Rust for
// the maps that affect the studio rather than applying an indiscriminate deep
// merge to every TOML table.
const replacementTables = new Set([
  'hotkeys',
  'key_aliases.keys',
  'mode_indicator.modes',
  'normal.bindings',
  'text_input.bindings',
  'window.bindings',
  'window.multi_select.bindings',
  'window_quick.bindings', 'window_editor.bindings', 'window_restore.bindings', 'window_tab.bindings',
  'grid.bindings',
  'recursive_grid.bindings',
  'ui_hint.bindings',
  'ui_hint.search_bindings',
  'plugin_modes',
])

/** Clone Vue-backed configuration state into a plain writable document. */
export function cloneConfigDocument(document: ConfigDocument): ConfigDocument {
  return cloneConfigValue(document)
}

// A shallow copy of a reactive document may retain proxies at any depth.
// TOML values are arrays, records and primitives; unwrap each container while
// copying once, rather than passing nested proxies to structuredClone.
function cloneConfigValue<T>(value: T): T {
  const raw = toRaw(value)
  if (Array.isArray(raw)) return raw.map(item => cloneConfigValue(item)) as T
  if (isRecord(raw)) return Object.fromEntries(
    Object.entries(raw).map(([key, child]) => [key, cloneConfigValue(child)]),
  ) as T
  return raw
}

/** Parse an uploaded TOML file into the editable value model used by the UI. */
export function parseConfigDocument(source: string): ParsedConfigDocument {
  const parsed = parse(source)
  if (!isRecord(parsed)) throw new Error('TOML 顶层必须是配置表')
  const scrollSettings: unknown = parsed.scroll
  if (isRecord(scrollSettings) && 'steps_per_second' in scrollSettings) {
    throw new Error('scroll.steps_per_second was removed; use scroll.speed (pixels/second)')
  }
  const scrollSpeed = isRecord(scrollSettings) ? scrollSettings.speed : undefined
  if (scrollSpeed !== undefined && (typeof scrollSpeed !== 'number' || !Number.isFinite(scrollSpeed) || scrollSpeed < 0)) {
    throw new Error('scroll.speed must be finite and non-negative')
  }
  const oldWindow = parsed.window as Record<string, unknown> | undefined
  for (const key of ['exit_mode', 'gap', 'split_ratios', 'layout_keys', 'double_tap_ms']) {
    if (oldWindow && key in oldWindow) throw new Error(`window.${key} 已移除，请迁移至独立窗口模式配置；返回目标使用 bindings`)
  }
  function checkBindings(value: unknown): void {
    if (Array.isArray(value)) { value.forEach(checkBindings); return }
    if (!isRecord(value)) return
    for (const [key, child] of Object.entries(value)) {
      if ((key === 'bindings' || key === 'hotkeys') && isRecord(child)) for (const action of Object.values(child).flat()) {
        if (String(action) === 'window_cycle_state') throw new Error(`${action} 已移除，请使用 window_maximize / window_minimize`)
        if (['window_layout', 'window_edit', 'window_saved_layouts', 'window_cancel', 'window_exit'].includes(String(action))) throw new Error(`${action} 已移除，请使用 window_quick / window_editor / window_restore 或普通模式绑定`)
      }
      checkBindings(child)
    }
  }
  checkBindings(parsed)
  const indicator = parsed.mode_indicator as ConfigDocument | undefined
  for (const ui of [indicator?.ui, ...Object.values(indicator?.modes ?? {}).map(entry => (entry as ConfigDocument)?.ui)]) {
    if (ui?.indicator_offset !== undefined && (!Array.isArray(ui.indicator_offset) || ui.indicator_offset.length !== 2 || !ui.indicator_offset.every((v: unknown) => typeof v === 'number' && Number.isInteger(v) && v >= -32768 && v <= 32767))) throw new Error('indicator_offset requires [X, Y] integers')
    for (const field of ['position', 'indicator_x_offset', 'indicator_y_offset']) {
      if (ui?.[field] !== undefined) throw new Error(`mode_indicator.ui.${field} was removed; use indicator_offset = [X, Y]`)
    }
  }
  for (const mode of ['window', 'window_quick', 'window_editor', 'window_restore', 'window_tab']) {
    const settings: unknown = parsed[mode]
    const card = isRecord(settings) ? settings.card : undefined
    if (card === undefined) continue
    if (mode !== 'window' && mode !== 'window_editor') throw new Error('卡片样式统一使用 window.card')
    if (!isRecord(card)) throw new Error(`${mode}.card 必须是配置表`)
    cardPositionRatios(card.position)
    const ranges: Record<string, [number, number]> = {
      app_font_size: [0, 256], title_font_size: [0, 256], text_width: [1, 4096],
      padding_x: [0, 256], padding_y: [0, 256], line_height: [1, 4],
      min_height: [0, 4096], number_min_width: [0, 4096],
      selected_border_width: [0, 20],
    }
    for (const [key, value] of Object.entries(card)) {
      const range = ranges[key]
      if (key === 'position') continue
      if (key === 'position_mode') {
        if (value !== 'window' && value !== 'screen') throw new Error('window.card.position_mode must be window or screen')
        continue
      }
      if (mode === 'window_editor') throw new Error('window_editor.card 只覆盖位置；其他样式使用 window.card')
      const valid = range ? typeof value === 'number' && Number.isFinite(value) && value >= range[0] && value <= range[1]
        : key === 'guide_line_width' ? typeof value === 'number' && Number.isFinite(value) && value >= 0 && value <= 32
        : ['app_bold', 'title_bold', 'guide_line_enabled'].includes(key) ? typeof value === 'boolean'
          : ['app_font_family', 'title_font_family'].includes(key) ? typeof value === 'string'
            : ['app_color', 'title_color', 'background_color', 'border_color', 'number_color', 'guide_line_color', 'selected_background_color', 'selected_border_color'].includes(key) ? (typeof value === 'string' ? /^#[\da-f]{8}$/i.test(value)
              : isRecord(value) && Object.entries(value).every(([appearance, color]) => ['light', 'dark'].includes(appearance) && typeof color === 'string' && /^#[\da-f]{8}$/i.test(color))) : false
      if (!valid) throw new Error(`${mode}.card.${key} 配置无效`)
    }
  }
  const point = (parsed.ui_hint as ConfigDocument | undefined)?.search_point
  if (point !== undefined) {
    if (!isRecord(point)) throw new Error('ui_hint.search_point must be a table')
    for (const [key, value] of Object.entries(point)) {
      if (key === 'field_modes') {
        if (!Array.isArray(value) || value.length !== 4 || value.some(v => !['concat', 'switch'].includes(v))) throw new Error('ui_hint.search_point.field_modes requires four concat/switch values')
        continue
      }
      if (key === 'color_preview') {
        const ranges: Record<string, [number, number]> = { width: [1, 64], height: [1, 64], x_offset: [-200, 200], y_offset: [-200, 200], border_width: [0, 10] }
        if (!isRecord(value) || Object.entries(value).some(([field, v]) => field === 'enabled' ? typeof v !== 'boolean' : !ranges[field] || !Number.isInteger(v) || Number(v) < ranges[field][0] || Number(v) > ranges[field][1])) throw new Error('ui_hint.search_point.color_preview is invalid')
        continue
      }
      const valid = key === 'color_formats' ? Array.isArray(value) && value.length > 0 && value.length <= 3 && new Set(value).size === value.length && value.every(v => ['hex', 'rgb', 'hsl'].includes(v))
        : key === 'marker_radius' ? Number.isInteger(value) && Number(value) >= 2 && Number(value) <= 40
          : key === 'marker_width' ? Number.isInteger(value) && Number(value) >= 0 && Number(value) <= 10
            : ['marker_color', 'input_background_color', 'input_border_color'].includes(key) ? (typeof value === 'string' ? /^#[\da-f]{8}$/i.test(value) : isRecord(value) && Object.keys(value).length === 2 && ['light', 'dark'].every(k => typeof value[k] === 'string' && /^#[\da-f]{8}$/i.test(value[k]))) : false
      if (!valid) throw new Error(`ui_hint.search_point.${key} is invalid`)
    }
  }
  const priority = (parsed.ui_hint as ConfigDocument | undefined)?.search_match_priority
  if (priority !== undefined && (!Array.isArray(priority) || priority.length !== 3 || new Set(priority).size !== 3 || priority.some(value => !['label', 'text', 'pinyin'].includes(value)))) throw new Error('ui_hint.search_match_priority requires label, text and pinyin exactly once')
  const uiHint = parsed.ui_hint as ConfigDocument | undefined
  const searchEditKeys = uiHint?.search_edit_keys
  if (uiHint && searchEditKeys !== undefined) uiHint.search_edit_keys = normalizeSearchEditBindings(searchEditKeys)
  const searchBindings = (parsed.ui_hint as ConfigDocument | undefined)?.search_bindings
  if (searchBindings !== undefined && (!isRecord(searchBindings) || Object.values(searchBindings).some(value => !['point_toggle', 'point_next', 'color_next'].includes(String(value))))) throw new Error('ui_hint.search_bindings requires point_toggle, point_next or color_next')
  parseSplitRatios((parsed.window_quick as Record<string, unknown> | undefined)?.split_ratios)

  // Stringifying once catches values that the editor would be unable to save.
  stringify(parsed)
  return {
    document: parsed as ConfigDocument,
    bytes: new TextEncoder().encode(source).byteLength,
    sections: Object.keys(parsed).length,
    values: countValues(parsed),
  }
}

/**
 * Resolve a sparse user document against the generated product defaults.
 * The returned value is preview-only: downloads still contain the user's
 * document, so importing a small override never expands it into a huge file.
 */
export function resolveConfigDocument(
  defaults: ConfigDocument,
  document: ConfigDocument,
): ConfigDocument {
  return mergeValue(defaults, document, '') as ConfigDocument
}

export function getConfigPath(document: ConfigDocument, path: string): unknown {
  return path.split('.').reduce<unknown>((value, part) => (
    value && typeof value === 'object' ? (value as ConfigDocument)[part] : undefined
  ), document)
}

export function setConfigPath(document: ConfigDocument, path: string, value: unknown): void {
  const parts = path.split('.')
  let target = document
  for (const part of parts.slice(0, -1)) target = target[part] ??= {}
  target[parts.at(-1)!] = value
}

export function deleteConfigPath(document: ConfigDocument, path: string): void {
  const parts = path.split('.')
  let target: ConfigDocument | undefined = document
  for (const part of parts.slice(0, -1)) {
    const next: unknown = target?.[part]
    if (!isRecord(next)) return
    target = next
  }
  if (target) delete target[parts.at(-1)!]
}

function mergeValue(defaultValue: unknown, configuredValue: unknown, path: string): unknown {
  if (configuredValue === undefined) return cloneConfigValue(defaultValue)
  if (path === 'ui_hint.search_edit_keys') return mergeSearchEditBindings(defaultValue, configuredValue)
  if (replacementTables.has(path) || Array.isArray(configuredValue) || !isRecord(configuredValue)) {
    return cloneConfigValue(configuredValue)
  }
  if (!isRecord(defaultValue)) return cloneConfigValue(configuredValue)

  const result: ConfigDocument = {}
  const keys = new Set([...Object.keys(defaultValue), ...Object.keys(configuredValue)])
  for (const key of keys) {
    const childPath = path ? `${path}.${key}` : key
    result[key] = mergeValue(defaultValue[key], configuredValue[key], childPath)
  }
  return result
}

function countValues(value: unknown): number {
  if (Array.isArray(value)) return value.length === 0 ? 1 : value.reduce((sum, item) => sum + countValues(item), 0)
  if (!isRecord(value)) return 1
  return Object.values(value).reduce((sum, item) => sum + countValues(item), 0)
}

function isRecord(value: unknown): value is ConfigDocument {
  return Boolean(value) && typeof value === 'object' && !Array.isArray(value)
}
