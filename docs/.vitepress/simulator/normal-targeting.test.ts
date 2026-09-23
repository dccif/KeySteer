import test from 'node:test'
import assert from 'node:assert/strict'
import { blindTargetingInput, createBlindTargetingState, targetingKeys, targetingLayout } from './normal-targeting.ts'

const document = {
  normal: { targeting: { method: 'grid', grid_cols: 2, grid_rows: 2, keys: 'asdf', max_depth: 2 } },
  grid: { grid_cols: 5, grid_rows: 4, keys: '12345qwertasdfgzxcvb', max_depth: 3 },
  recursive_grid: { grid_cols: 3, grid_rows: 3, keys: 'qweasdzxc', max_depth: 10, min_size_width: 1, min_size_height: 1 },
}

test('Normal targeting uses its own geometry without changing standalone Grid', () => {
  const state = createBlindTargetingState()
  assert.equal(document.grid.grid_cols, 5)
  assert.equal(targetingLayout(document, 0)?.keys, 'asdf')
  assert.deepEqual(blindTargetingInput(document, state, 'a').pointer, { x: 25, y: 25 })
  assert.deepEqual(blindTargetingInput(document, state, 's').pointer, { x: 37.5, y: 12.5 })
  assert.equal(state.terminal, true)
  assert.equal(blindTargetingInput(document, state, 'd').pointer, undefined)
  blindTargetingInput(document, state, 'tab')
  assert.deepEqual(blindTargetingInput(document, state, 'd').pointer, { x: 12.5, y: 37.5 })
  blindTargetingInput(document, state, 'space')
  assert.deepEqual(state.path, [])
})

test('Recursive Grid layers replace inheritance and reset on the next selection', () => {
  const recursive = {
    ...document,
    normal: { targeting: { method: 'recursive_grid', layers: [{ depth: 0, grid_cols: 4, grid_rows: 1, keys: '1234' }], max_depth: 3 } },
    recursive_grid: { ...document.recursive_grid, layers: [{ depth: 0, keys: 'abcdefghi' }] },
  }
  const state = createBlindTargetingState()
  assert.equal(targetingKeys(recursive).has('f'), false)
  assert.equal(targetingKeys(recursive).has('1'), true)
  assert.deepEqual(blindTargetingInput(recursive, state, '1').pointer, { x: 12.5, y: 50 })
  state.pendingReset = true
  assert.deepEqual(blindTargetingInput(recursive, state, '2').pointer, { x: 37.5, y: 50 })
  recursive.normal.targeting.layers = []
  assert.equal(targetingLayout(recursive, 0)?.keys, 'qweasdzxc')
})

test('Absent targeting leaves normal input unclaimed', () => {
  const state = createBlindTargetingState()
  assert.deepEqual(blindTargetingInput({ normal: {}, grid: document.grid }, state, 'a'), { handled: false })
})

test('Single-level targeting continuously selects root cells for either method', () => {
  for (const method of ['grid', 'recursive_grid']) {
    const config = { ...document, normal: { targeting: { method, grid_cols: 3, grid_rows: 2, keys: 'asdzxc', max_depth: 1, reset_on: [] } } }
    const state = createBlindTargetingState()
    assert.deepEqual([...targetingKeys(config)], [...'asdzxc'])
    for (const key of ['a', 'c', 's', 's', 'z', 'x', 'd', 'tab', 'space', 'a']) {
      const result = blindTargetingInput(config, state, key)
      const index = 'asdzxc'.indexOf(key)
      if (index < 0) assert.deepEqual(result, { handled: false })
      else {
        assert.ok(Math.abs(result.pointer!.x - ((index % 3) * 100 / 3 + 100 / 6)) < 1e-9)
        assert.equal(result.pointer!.y, Math.floor(index / 3) * 50 + 25)
      }
    }
  }
})
