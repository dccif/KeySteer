import { cardPositionRatios } from '../simulator/window-card-position.ts'
import { panelPositionPercentages } from '../simulator/search-panel-position.ts'
import { toRaw } from 'vue'
import { parse, stringify } from 'smol-toml'
import { parseSplitRatios } from '../simulator/window-ratios.ts'
import { normalizeConfigCompatibility } from './compatibility.ts'
import { readSearchEditBindings } from './search-edit-keys.ts'
import { resolvedChordKeys } from '../simulator/bindings.ts'
import { validQuickSwitchCharacters } from '../simulator/hint-labels.ts'

export type ConfigDocument = Record<string, any>

const routingModes = ['normal', 'quick_switch', 'text_input', 'text_select', 'grid', 'recursive_grid', 'bisect', 'ui_hint', 'window', 'window_quick', 'window_editor', 'window_restore', 'window_tab']

function mergeSearchEditBindings(defaults: unknown, configured: unknown): Record<string, string> {
  const base = readSearchEditBindings(defaults ?? {})
  const overrides = readSearchEditBindings(configured)
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
  warnings: string[]
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
  'quick_switch.bindings',
  'text_select.bindings',
  'window.bindings',
  'window.multi_select.bindings',
  'window_quick.bindings', 'window_editor.bindings', 'window_restore.bindings', 'window_tab.bindings',
  'grid.bindings',
  'recursive_grid.bindings',
  'bisect.bindings',
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
  const warnings = normalizeConfigCompatibility(parsed)
  for (const [mode, settings] of [
    ...routingModes.map(mode => [mode, parsed[mode]] as const),
    ...Object.entries(parsed.plugin_modes ?? {}).map(([mode, settings]) => [`plugin_modes.${mode}`, settings] as const),
  ]) {
    if (!isRecord(settings)) continue
    for (const key of ['inherits', 'temporary_mode_keys', 'temporary_mode_passthrough_keys']) {
      const value = settings[key]
      if (value !== undefined && (!Array.isArray(value) || value.some(entry => typeof entry !== 'string'))) throw new Error(`${mode}.${key} must be a string array`)
    }
    if (settings.temporary_mode !== undefined && typeof settings.temporary_mode !== 'string') throw new Error(`${mode}.temporary_mode must be a string`)
  }
  if (parsed.bisect !== undefined) {
    if (!isRecord(parsed.bisect)) throw new Error('bisect must be a table')
    const settings = parsed.bisect as ConfigDocument
    const allowed = new Set(['enabled', 'min_size_width', 'min_size_height', 'max_depth', 'cursor_follow_selection', 'inherits', 'temporary_mode', 'temporary_mode_keys', 'temporary_mode_passthrough_keys', 'lifecycle', 'ui', 'bindings', 'app_configs'])
    for (const key of Object.keys(settings)) if (!allowed.has(key)) throw new Error(`bisect.${key} is not supported`)
    for (const key of ['max_depth', 'min_size_width', 'min_size_height']) {
      const value = settings[key]
      if (value !== undefined && (!Number.isInteger(value) || value < 1 || value > (key === 'max_depth' ? 40 : 4294967295))) throw new Error(`bisect.${key} is invalid`)
    }
    for (const key of ['enabled', 'cursor_follow_selection']) {
      if (settings[key] !== undefined && typeof settings[key] !== 'boolean') throw new Error(`bisect.${key} must be boolean`)
    }
    if (settings.ui !== undefined && !isRecord(settings.ui)) throw new Error('bisect.ui must be a table')
    if (settings.lifecycle !== undefined) {
      if (!isRecord(settings.lifecycle)) throw new Error('bisect.lifecycle must be a table')
      const clicks = ['left_click', 'right_click', 'middle_click', 'double_click']
      for (const [event, action] of Object.entries(settings.lifecycle)) {
        const allowed = ['keep', 'restart', 'return', 'idle', ...routingModes, ...(event === 'after_finish' ? clicks : ['finish'])]
        if (!['after_finish', 'after_click'].includes(event) || typeof action !== 'string' || (!allowed.includes(action) && !action.includes(':'))) throw new Error(`bisect.lifecycle.${event} is invalid`)
      }
    }
    for (const [key, value] of Object.entries(settings.ui ?? {})) {
      const color = (v: unknown): boolean => typeof v === 'string' && /^#[\da-f]{8}$/i.test(v)
      const valid = key === 'line_width' ? Number.isInteger(value) && Number(value) >= 0 && Number(value) <= 20
        : ['line_color', 'highlight_color'].includes(key) && (color(value) || isRecord(value) && Object.keys(value).length === 2 && ['light', 'dark'].every(k => color(value[k])))
      if (!valid) throw new Error(`bisect.ui.${key} is invalid`)
    }
  }
  const quickSwitch = parsed.quick_switch as ConfigDocument | undefined
  if (quickSwitch && !validQuickSwitchCharacters(quickSwitch.hint_characters ?? 'wertyuiop', String(quickSwitch.key ?? 'q'))) {
    throw new Error('quick_switch.hint_characters requires at least two unique character keys, excluding the trigger key')
  }
  if (quickSwitch) {
    const alphabet = new Set([...String(quickSwitch.hint_characters ?? 'wertyuiop').toLowerCase()])
    for (const configured of Object.keys(quickSwitch.bindings ?? {})) {
      for (const chord of configured.split(/\s+/).filter(Boolean)) {
        for (const isMac of [false, true]) {
          const keys = resolvedChordKeys(parsed, chord, isMac)
          if (keys.length === 1 && alphabet.has(keys[0])) throw new Error(`quick_switch.bindings key ${chord} conflicts with hint_characters`)
        }
      }
    }
  }
  const scrollSettings: unknown = parsed.scroll
  const scrollSpeed = isRecord(scrollSettings) ? scrollSettings.speed : undefined
  if (scrollSpeed !== undefined && (typeof scrollSpeed !== 'number' || !Number.isFinite(scrollSpeed) || scrollSpeed < 0)) {
    throw new Error('scroll.speed must be finite and non-negative')
  }
  const indicator = parsed.mode_indicator as ConfigDocument | undefined
  const textSelect = parsed.text_select as ConfigDocument | undefined
  if (textSelect !== undefined) {
    if (!isRecord(textSelect)) throw new Error('text_select must be a table')
    for (const key of Object.keys(textSelect)) {
      if (!['enabled', 'scan_scope', 'inherits', 'temporary_mode', 'temporary_mode_keys', 'temporary_mode_passthrough_keys', 'hint_characters', 'ui', 'lifecycle', 'bindings'].includes(key)) throw new Error(`text_select.${key} is unknown`)
    }
    if (textSelect.scan_scope !== undefined && !['window', 'active', 'screen', 'all'].includes(textSelect.scan_scope)) {
      throw new Error('text_select.scan_scope must be window, active, screen or all')
    }
  }
  const pairInRange = (value: unknown, min: number, max: number): boolean => Array.isArray(value)
    && value.length === 2 && value.every(v => typeof v === 'number' && Number.isInteger(v) && v >= min && v <= max)
  const textSelectUi = textSelect?.ui
  if (textSelectUi !== undefined) {
    if (!isRecord(textSelectUi)) throw new Error('text_select.ui must be a table')
    for (const [key, value] of Object.entries(textSelectUi)) {
      const valid = key === 'border_width' ? typeof value === 'number' && Number.isFinite(value) && value >= 0 && value <= 20
        : key === 'panel_width' ? typeof value === 'number' && Number.isInteger(value) && value >= 320 && value <= 2400
        : ['word_min_width', 'char_min_width'].includes(key) ? typeof value === 'number' && Number.isInteger(value) && value >= 24 && value <= 240
        : key === 'padding' ? pairInRange(value, 0, 100)
          : key === 'pointer_offset' ? pairInRange(value, -32768, 32767)
            : key === 'border_color' && (typeof value === 'string' ? /^#[\da-f]{8}$/i.test(value)
              : isRecord(value) && Object.keys(value).length === 2 && ['light', 'dark'].every(k => typeof value[k] === 'string' && /^#[\da-f]{8}$/i.test(value[k])))
      if (!valid) throw new Error(`text_select.ui.${key} is invalid`)
    }
  }
  for (const ui of [indicator?.ui, ...Object.values(indicator?.modes ?? {}).map(entry => (entry as ConfigDocument)?.ui)]) {
    if (ui?.indicator_offset !== undefined && (!Array.isArray(ui.indicator_offset) || ui.indicator_offset.length !== 2 || !ui.indicator_offset.every((v: unknown) => typeof v === 'number' && Number.isInteger(v) && v >= -32768 && v <= 32767))) throw new Error('indicator_offset requires [X, Y] integers')
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
  const highlight = (parsed.ui_hint as ConfigDocument | undefined)?.search_highlight
  if (highlight !== undefined) {
    if (!isRecord(highlight)) throw new Error('ui_hint.search_highlight must be a table')
    for (const [key, value] of Object.entries(highlight)) {
      const valid = key === 'border_width' ? typeof value === 'number' && Number.isFinite(value) && value >= 0 && value <= 20
        : ['background_color', 'text_color', 'border_color'].includes(key) && (typeof value === 'string' ? /^#[\da-f]{8}$/i.test(value)
          : isRecord(value) && Object.keys(value).length === 2 && ['light', 'dark'].every(k => typeof value[k] === 'string' && /^#[\da-f]{8}$/i.test(value[k])))
      if (!valid) throw new Error(`ui_hint.search_highlight.${key} is invalid`)
    }
  }
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
  for (const block of ['search_input_ui', 'search_info_ui']) {
    const panel = uiHint?.[block]
    if (!panel) continue
    const path = `ui_hint.${block}`
    if (panel.position !== undefined) panelPositionPercentages(panel.position)
    if (panel.position_mode !== undefined && !(block === 'search_input_ui' ? ['screen', 'window'] : ['screen', 'window', 'search_input']).includes(panel.position_mode)) {
      throw new Error(`${path}.position_mode is invalid`)
    }
  }
  const searchEditKeys = uiHint?.search_edit_keys
  if (searchEditKeys !== undefined) readSearchEditBindings(searchEditKeys)
  const copyKeys = uiHint?.search_copy_keys
  if (copyKeys !== undefined && (!Array.isArray(copyKeys) || copyKeys.length !== 4 || !copyKeys.every(key => typeof key === 'string'))) throw new Error('search_copy_keys requires four stable field shortcuts')
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
    warnings,
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
  const effective = mergeValue(defaults, document, '') as ConfigDocument
  // A missing trigger list disables this optional layer, including when the
  // entire user section is absent. Never inherit triggers from product defaults.
  for (const mode of routingModes) {
    if (effective[mode] && document[mode]?.temporary_mode_keys === undefined) effective[mode].temporary_mode_keys = []
  }
  // Match the native upgrade fallback: an older Normal map must not hide a
  // newly added text mode, but occupied keys and remapped entries take priority.
  const enters = (action: unknown, mode: string): boolean => action === mode
    || Array.isArray(action) && action.some(item => enters(item, mode))
  const tables = [effective.normal?.bindings ?? {}]
  if ((effective.normal?.inherits ?? ['hotkeys']).some((name: string) => name === 'hotkeys' || name === 'idle')) tables.push(effective.hotkeys ?? {})
  for (const [mode, key] of [['text_select', 'c'], ['text_input', '\\']]) {
    if (document[mode] !== undefined || tables.some(table => Object.values(table).some(action => enters(action, mode)))) continue
    const targeting = effective.normal?.targeting
    if (targeting) {
      const grid = effective[targeting.method === 'recursive_grid' ? 'recursive_grid' : 'grid']
      if (String(targeting.keys ?? grid?.keys ?? '').includes(key)
        || (targeting.layers ?? grid?.layers ?? []).some((layer: ConfigDocument) => String(layer.keys ?? '').includes(key))) continue
    }
    const occupied = tables.some(table => Object.keys(table).some(keys => keys.split(/\s+/).some(chord =>
      [false, true].some(isMac => {
        const resolved = resolvedChordKeys(effective, chord, isMac)
        const expected = resolvedChordKeys(effective, key, isMac)
        return expected.some(value => resolved.includes(value))
      }))))
    if (!occupied && effective.normal?.bindings) effective.normal.bindings[key] = mode
  }
  // Match native upgrade fallbacks for both region-scan shortcuts.
  const declares = (table: unknown, key: string): boolean => isRecord(table) && Object.keys(table).some(keys =>
    keys.split(/\s+/).some(chord => [false, true].some(isMac => {
      const resolved = resolvedChordKeys(effective, chord, isMac)
      const shortcut = resolvedChordKeys(effective, key, isMac)
      return resolved.length === 1 && shortcut.length === 1 && resolved[0] === shortcut[0]
    })))
  for (const [key, action] of [['u', 'area_hint'], ['i', 'area_text']]) {
    const normalUses = declares(document.normal?.bindings, key) || (document.normal?.app_configs ?? []).some((entry: ConfigDocument) => declares(entry.bindings, key))
    const hotkeysUse = declares(document.hotkeys, key) || (document.app_configs ?? []).some((entry: ConfigDocument) => declares(entry.bindings, key))
    for (const mode of ['grid', 'recursive_grid', 'bisect']) {
      const settings = effective[mode]
      const inherits = settings?.inherits ?? ['hotkeys', 'normal']
      const shortcutKeys = [false, true].flatMap(isMac => resolvedChordKeys(effective, key, isMac)).filter(physical => [...physical].length === 1)
      const labelUses = mode !== 'bisect' && shortcutKeys.some(physical => String(settings?.keys ?? '').toLowerCase().includes(physical)
        || (settings?.layers ?? []).some((layer: ConfigDocument) => String(layer.keys ?? '').toLowerCase().includes(physical)))
      const inheritedUses = inherits.some((name: string) => name === 'normal' ? normalUses : ['hotkeys', 'idle'].includes(name) && hotkeysUse)
      const local = document[mode]?.bindings
      const explicitlyBound = declares(local, key)
      const sameSpelling = isRecord(local) && Object.keys(local).some(keys => keys.split(/\s+/).some(chord => chord.toLowerCase() === key))
      if (settings?.bindings?.[key] === action && (explicitlyBound ? !sameSpelling : labelUses || inheritedUses)) delete settings.bindings[key]
    }
  }
  return effective
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
  const controls = new Set(['bisect_left', 'bisect_down', 'bisect_up', 'bisect_right', 'first_level', 'previous_level', 'reopen_selection', 'confirm_selection', 'cancel_selection', 'search_selection', 'area_hint', 'area_text', 'window_area_number', 'window_next_page', 'window_previous_page', 'window_multi_cancel'])
  if (['grid.bindings', 'recursive_grid.bindings', 'bisect.bindings', 'ui_hint.bindings', 'window_editor.bindings', 'window_restore.bindings', 'window.multi_select.bindings'].includes(path) && isRecord(defaultValue) && isRecord(configuredValue)) {
    const expand = (table: ConfigDocument) => Object.fromEntries(Object.entries(table).flatMap(([keys, value]) => keys.split(/\s+/).map(key => [key, value])))
    const configured = expand(configuredValue)
    const fallback = Object.fromEntries(Object.entries(expand(defaultValue)).filter(([key, action]) => controls.has(String(action)) || path === 'window.multi_select.bindings' && !key.includes('+') && String(action).startsWith('send ')))
    return { ...fallback, ...configured }
  }
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
