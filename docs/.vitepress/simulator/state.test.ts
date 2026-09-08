import assert from 'node:assert/strict'
import test from 'node:test'
import {
  applyModeAction,
  applyKeyHelpAction,
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

test('key_help toggles without resetting selection and closes on idle', () => {
  const state = createSimulatorState()
  applyModeAction(state, 'grid')
  state.targeting.grid.path.push('a')
  assert.equal(applyKeyHelpAction(state, 'key_help'), true)
  assert.equal(state.keyHelpVisible, true)
  assert.deepEqual(state.targeting.grid.path, ['a'])
  applyKeyHelpAction(state, 'key_help')
  assert.equal(state.keyHelpVisible, false)
  applyKeyHelpAction(state, 'key_help', false)
  assert.equal(state.keyHelpVisible, false)
  applyKeyHelpAction(state, 'key_help')
  applyModeAction(state, 'idle')
  assert.equal(state.keyHelpVisible, false)
  applyKeyHelpAction(state, 'key_help')
  assert.equal(state.keyHelpVisible, false)
})
