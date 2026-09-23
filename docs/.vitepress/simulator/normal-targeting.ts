import type { ConfigDocument } from '../config-studio/document.ts'
import type { Point } from './state.ts'

export interface BlindTargetingState {
  path: number[]
  terminal: boolean
  pendingReset: boolean
}

export interface TargetingLayout {
  grid_cols: number
  grid_rows: number
  keys: string
}

export function createBlindTargetingState(): BlindTargetingState {
  return { path: [], terminal: false, pendingReset: true }
}

export function targetingConfig(document: ConfigDocument): ConfigDocument | undefined {
  const settings = document.normal?.targeting
  return settings && typeof settings === 'object' && !Array.isArray(settings) ? settings : undefined
}

export function targetingMethod(settings: ConfigDocument): 'grid' | 'recursive_grid' {
  return settings.method === 'recursive_grid' ? 'recursive_grid' : 'grid'
}

export function targetingLayout(document: ConfigDocument, depth: number): TargetingLayout | undefined {
  const settings = targetingConfig(document)
  if (!settings) return undefined
  const method = targetingMethod(settings)
  const base = document[method] ?? {}
  const cols = Number(settings.grid_cols ?? base.grid_cols)
  const rows = Number(settings.grid_rows ?? base.grid_rows)
  const keys = String(settings.keys ?? base.keys ?? '')
  const layers = method === 'recursive_grid' ? (settings.layers ?? base.layers ?? []) : []
  const layer = Array.isArray(layers) ? layers.find(item => item?.depth === depth) : undefined
  return {
    grid_cols: Number(layer?.grid_cols ?? cols),
    grid_rows: Number(layer?.grid_rows ?? rows),
    keys: String(layer?.keys ?? keys),
  }
}

export function targetingKeys(document: ConfigDocument): Set<string> {
  const settings = targetingConfig(document)
  if (!settings) return new Set()
  const method = targetingMethod(settings)
  const alphabet = new Set([...String(settings.keys ?? document[method]?.keys ?? '')])
  const layers = method === 'recursive_grid' ? settings.layers ?? document.recursive_grid?.layers : undefined
  if (Array.isArray(layers)) for (const layer of layers) for (const key of String(layer?.keys ?? '')) alphabet.add(key)
  return new Set([...alphabet, 'tab', 'backspace', 'space'])
}

export function resetBlindTargeting(state: BlindTargetingState): void {
  state.path = []
  state.terminal = false
  state.pendingReset = false
}

export function blindTargetingInput(
  document: ConfigDocument,
  state: BlindTargetingState,
  key: string,
  canvas: { width: number; height: number } = { width: 960, height: 600 },
): { handled: boolean; pointer?: Point } {
  const settings = targetingConfig(document)
  if (!settings || !targetingKeys(document).has(key)) return { handled: false }
  if (state.pendingReset) resetBlindTargeting(state)
  if (key === 'space') { resetBlindTargeting(state); return { handled: true } }
  if (key === 'tab' || key === 'backspace') {
    state.path.pop(); state.terminal = false
    return { handled: true }
  }
  if (state.terminal) return { handled: true }
  const layout = targetingLayout(document, state.path.length)
  if (!layout || layout.grid_cols < 1 || layout.grid_rows < 1) return { handled: false }
  const index = [...layout.keys].indexOf(key)
  if (index < 0 || index >= layout.grid_cols * layout.grid_rows) return { handled: true }
  let region = { x: 0, y: 0, width: 100, height: 100 }
  for (const [depth, selected] of [...state.path, index].entries()) {
    const current = targetingLayout(document, depth)!
    region = {
      x: region.x + selected % current.grid_cols * region.width / current.grid_cols,
      y: region.y + Math.floor(selected / current.grid_cols) * region.height / current.grid_rows,
      width: region.width / current.grid_cols,
      height: region.height / current.grid_rows,
    }
  }
  state.path.push(index)
  const method = targetingMethod(settings)
  const source = document[method] ?? {}
  const maxDepth = Number(settings.max_depth ?? source.max_depth ?? (method === 'grid' ? 3 : 10))
  state.pendingReset = maxDepth === 1
  const next = targetingLayout(document, state.path.length)!
  const minWidth = Number(settings.min_size_width ?? source.min_size_width ?? 1)
  const minHeight = Number(settings.min_size_height ?? source.min_size_height ?? 1)
  state.terminal = method === 'grid'
    ? state.path.length >= maxDepth
    : state.path.length + 1 >= maxDepth || region.width / 100 * canvas.width / next.grid_cols < minWidth
      || region.height / 100 * canvas.height / next.grid_rows < minHeight
  return { handled: true, pointer: { x: region.x + region.width / 2, y: region.y + region.height / 2 } }
}
