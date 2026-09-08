export type SimulatorMode = 'idle' | 'normal' | 'grid' | 'recursive_grid' | 'ui_hint'

export interface Point {
  x: number
  y: number
}

export interface TargetingState {
  grid: {
    path: string[]
    maxDepth: number
  }
  recursiveGrid: {
    path: string[]
    maxDepth: number
  }
}

export interface SimulatorState {
  mode: SimulatorMode
  keyHelpVisible: boolean
  pointer: Point
  pressedButtons: Set<'left' | 'right' | 'middle'>
  targeting: TargetingState
  lastEvent: string
}

export const MOVEMENT_ACTIONS = new Set([
  'move_left',
  'move_down',
  'move_up',
  'move_right',
])

export function createSimulatorState(): SimulatorState {
  return {
    mode: 'normal',
    keyHelpVisible: false,
    pointer: { x: 50, y: 50 },
    pressedButtons: new Set(),
    targeting: {
      grid: { path: [], maxDepth: 3 },
      recursiveGrid: { path: [], maxDepth: 10 },
    },
    lastEvent: '模拟器就绪',
  }
}

export function movePointer(state: SimulatorState, action: string, distance: number): void {
  if (action === 'move_left') state.pointer.x -= distance
  if (action === 'move_right') state.pointer.x += distance
  if (action === 'move_up') state.pointer.y -= distance
  if (action === 'move_down') state.pointer.y += distance
  state.pointer.x = clamp(state.pointer.x, 0, 100)
  state.pointer.y = clamp(state.pointer.y, 0, 100)
  state.lastEvent = action
}

export function applyModeAction(state: SimulatorState, action: string): boolean {
  if (!isSimulatorMode(action)) return false
  if (action === 'idle') state.keyHelpVisible = false
  state.mode = action
  state.lastEvent = `进入 ${action}`
  return true
}

export function toggleButton(
  state: SimulatorState,
  button: 'left' | 'right' | 'middle',
): void {
  if (state.pressedButtons.has(button)) state.pressedButtons.delete(button)
  else state.pressedButtons.add(button)
  state.lastEvent = `${button} ${state.pressedButtons.has(button) ? 'pressed' : 'released'}`
}

export function resetTargetingPath(state: SimulatorState, mode: 'grid' | 'recursive_grid'): void {
  if (mode === 'grid') state.targeting.grid.path = []
  else state.targeting.recursiveGrid.path = []
}

function isSimulatorMode(value: string): value is SimulatorMode {
  return ['idle', 'normal', 'grid', 'recursive_grid', 'ui_hint'].includes(value)
}

function clamp(value: number, minimum: number, maximum: number): number {
  return Math.min(maximum, Math.max(minimum, value))
}

export function applyKeyHelpAction(state: SimulatorState, action: string, enabled = true): boolean {
  if (action !== 'key_help') return false
  if (state.mode !== 'idle') {
    state.keyHelpVisible = enabled && !state.keyHelpVisible
  }
  state.lastEvent = action
  return true
}
