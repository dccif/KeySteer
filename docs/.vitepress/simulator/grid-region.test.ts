import assert from 'node:assert/strict'
import test from 'node:test'
import { gridRegion } from './grid-region.ts'

test('nested selection stays inside the selected cell, and backtracking restores its parent', () => {
  assert.deepEqual(gridRegion(3, 3, 'qweasdzxc', []), { x: 0, y: 0, width: 100, height: 100 })
  const first = gridRegion(3, 3, 'qweasdzxc', ['d'])
  const next = gridRegion(3, 3, 'qweasdzxc', ['d', 'q'])
  assert.equal(first.x, 200 / 3)
  assert.equal(first.y, 100 / 3)
  assert.equal(next.x, first.x)
  assert.equal(next.y, first.y)
  assert.equal(next.width, first.width / 3)
  assert.deepEqual(gridRegion(3, 3, 'qweasdzxc', ['d', 'q'].slice(0, -1)), first)
})

test('rectangular grids use row-major key positions and ignore unavailable cells', () => {
  assert.deepEqual(gridRegion(5, 4, '12345qwertasdfgzxcvb', ['v', '5']), { x: 76, y: 75, width: 4, height: 6.25 })
  assert.deepEqual(gridRegion(2, 2, 'abcde', ['e']), gridRegion(2, 2, 'abcde', []))
})
