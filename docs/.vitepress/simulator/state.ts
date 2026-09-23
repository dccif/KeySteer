import { createWindowState, leaveWindow, isWindowMode, type WindowState, type WindowMode } from './window.ts'
import { createBlindTargetingState, type BlindTargetingState } from './normal-targeting.ts'

export type SimulatorMode = 'idle' | 'normal' | 'text_input' | 'grid' | 'recursive_grid' | 'ui_hint' | WindowMode

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
  window: WindowState
  mode: SimulatorMode
  keyHelpVisible: boolean
  windowHelpOverride: boolean | null
  pointer: Point
  pressedButtons: Set<'left' | 'right' | 'middle'>
  targeting: TargetingState
  blindTargeting: BlindTargetingState
  lastEvent: string
}

export const MOVEMENT_ACTIONS = new Set([
  'move_left',
  'move_down',
  'move_up',
  'move_right',
])

export function createSimulatorState(mouseKeyHelp = false): SimulatorState {
  return {
    window: createWindowState(),
    mode: 'normal',
    keyHelpVisible: mouseKeyHelp,
    windowHelpOverride: null,
    pointer: { x: 50, y: 50 },
    pressedButtons: new Set(),
    targeting: {
      grid: { path: [], maxDepth: 3 },
      recursiveGrid: { path: [], maxDepth: 10 },
    },
    blindTargeting: createBlindTargetingState(),
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

export function applyModeAction(state: SimulatorState, action: string, mouseKeyHelp = false): boolean {
  if (!isSimulatorMode(action)) return false
  if (action === 'idle' || action === 'text_input') state.keyHelpVisible = false
  else if (state.mode === 'idle' || isWindowMode(state.mode)) state.keyHelpVisible = mouseKeyHelp
  if (action === 'text_input') state.pressedButtons.clear()
  if (state.mode.startsWith('window')) leaveWindow(state)
  state.mode = action
  state.blindTargeting.pendingReset = true
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
  return ['idle', 'normal', 'text_input', 'grid', 'recursive_grid', 'ui_hint'].includes(value)
}

function clamp(value: number, minimum: number, maximum: number): number {
  return Math.min(maximum, Math.max(minimum, value))
}

export function applyKeyHelpAction(state: SimulatorState, action: string, _mouseDefaultVisible = false, windowDefaultVisible = true): boolean {
  if (action !== 'key_help') return false
  if (state.mode !== 'idle') {
    if (isWindowMode(state.mode) && !state.window.temporary) {
      state.windowHelpOverride = !(state.windowHelpOverride ?? windowDefaultVisible)
    } else {
      state.keyHelpVisible = !state.keyHelpVisible
    }
  }
  state.lastEvent = action
  return true
}
