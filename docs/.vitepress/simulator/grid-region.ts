/** Grid coordinates in percentages of the simulator's logical canvas. */
export function gridRegion(cols: number, rows: number, keys: string, path: readonly string[]) {
  let x = 0, y = 0, width = 100, height = 100
  const columns = Math.max(1, Math.floor(cols))
  const lines = Math.max(1, Math.floor(rows))
  const labels = Array.from(keys)
  for (const key of path) {
    const index = labels.indexOf(key)
    if (index < 0 || index >= columns * lines) break
    width /= columns
    height /= lines
    x += index % columns * width
    y += Math.floor(index / columns) * height
  }
  return { x, y, width, height }
}
