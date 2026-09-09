import { toRaw } from 'vue'
import { parse, stringify } from 'smol-toml'
import { parseSplitRatios } from '../simulator/window-ratios.ts'

export type ConfigDocument = Record<string, any>

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
  'window.bindings',
  'window_quick.bindings', 'window_editor.bindings', 'window_restore.bindings', 'window_delete.bindings',
  'grid.bindings',
  'recursive_grid.bindings',
  'ui_hint.bindings',
  'plugin_modes',
])

/** Clone Vue-backed configuration state into a plain writable document. */
export function cloneConfigDocument(document: ConfigDocument): ConfigDocument {
  return structuredClone(toRaw(document))
}

/** Parse an uploaded TOML file into the editable value model used by the UI. */
export function parseConfigDocument(source: string): ParsedConfigDocument {
  const parsed = parse(source)
  if (!isRecord(parsed)) throw new Error('TOML 顶层必须是配置表')
  const oldWindow = parsed.window as Record<string, unknown> | undefined
  for (const key of ['exit_mode', 'gap', 'split_ratios', 'layout_keys', 'double_tap_ms']) {
    if (oldWindow && key in oldWindow) throw new Error(`window.${key} 已移除，请迁移至独立窗口模式配置；返回目标使用 bindings`)
  }
  function checkBindings(value: unknown): void {
    if (Array.isArray(value)) { value.forEach(checkBindings); return }
    if (!isRecord(value)) return
    for (const [key, child] of Object.entries(value)) {
      if ((key === 'bindings' || key === 'hotkeys') && isRecord(child)) for (const action of Object.values(child).flat()) {
        if (['window_maximize', 'window_cycle_state'].includes(String(action))) throw new Error(`${action} 已移除，请使用 size_cycle`)
        if (['window_layout', 'window_edit', 'window_saved_layouts', 'window_cancel', 'window_exit'].includes(String(action))) throw new Error(`${action} 已移除，请使用 window_quick / window_editor / window_restore 或普通模式绑定`)
      }
      checkBindings(child)
    }
  }
  checkBindings(parsed)
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
  if (configuredValue === undefined) return structuredClone(toRaw(defaultValue))
  if (replacementTables.has(path) || Array.isArray(configuredValue) || !isRecord(configuredValue)) {
    return structuredClone(toRaw(configuredValue))
  }
  if (!isRecord(defaultValue)) return structuredClone(toRaw(configuredValue))

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
