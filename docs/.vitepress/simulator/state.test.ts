import assert from 'node:assert/strict'
import test from 'node:test'
import {
  applyModeAction,
  createSimulatorState,
  movePointer,
  resetTargetingPath,
  toggleButton,
} from './state.ts'

test('pointer movement is clamped to the simulated screen', () => {
  const state = createSimulatorState()
  movePointer(state, 'move_left', 80)
  movePointer(state, 'move_down', 70)
  assert.deepEqual(state.pointer, { x: 0, y: 100 })
})

test('mode changes reserve independent grid paths', () => {
  const state = createSimulatorState()
  state.targeting.grid.path.push('q')
  state.targeting.recursiveGrid.path.push('a', 's')
  assert.equal(applyModeAction(state, 'recursive_grid'), true)
  resetTargetingPath(state, 'grid')
  assert.deepEqual(state.targeting.grid.path, [])
  assert.deepEqual(state.targeting.recursiveGrid.path, ['a', 's'])
})

test('button toggles retain pressed state', () => {
  const state = createSimulatorState()
  toggleButton(state, 'left')
  assert.equal(state.pressedButtons.has('left'), true)
  toggleButton(state, 'left')
  assert.equal(state.pressedButtons.has('left'), false)
})
