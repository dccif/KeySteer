// Pure layout language; kept behaviorally equivalent to api/window_layout.rs.
export interface WindowRect { x: number; y: number; width: number; height: number }
export type LayoutDirection = 'left' | 'down' | 'up' | 'right'
type Axis = 'x' | 'y'
export interface QuickPlacement { horizontal: AxisPlacement | null; vertical: AxisPlacement | null }
interface AxisPlacement { near: boolean; ratio: number }
export type LayoutNode = { kind: 'slot'; id: number; window: number | null }
  | { kind: 'split'; axis: Axis; ratio: number; first: LayoutNode; second: LayoutNode }
export interface LayoutTree { root: LayoutNode; selected: number; nextSlot: number }
export interface LayoutSlot { id: number; window: number | null; rect: WindowRect }
export interface LayoutWindow extends WindowRect { id: number; minWidth?: number; minHeight?: number }
export const DEFAULT_SPLIT_RATIOS = [.25, 1 / 3, .5, 2 / 3, .75]
export const LAYOUT_RATIOS = [.25, 1 / 3, .5, 2 / 3, .75, 1]
const ratioNames = ['1/4', '1/3', '1/2', '2/3', '3/4', '1']
const axisOf = (direction: LayoutDirection): Axis => direction === 'left' || direction === 'right' ? 'x' : 'y'
const near = (direction: LayoutDirection): boolean => direction === 'left' || direction === 'up'
const center = (rect: WindowRect, axis: Axis): number => rect[axis] + (axis === 'x' ? rect.width : rect.height) / 2
const clamp = (value: number, lo: number, hi: number): number => Math.max(lo, Math.min(hi, value))

export function quickStep(quick: QuickPlacement, direction: LayoutDirection, ratios: readonly number[] = LAYOUT_RATIOS): void {
  const axis = axisOf(direction) === 'x' ? 'horizontal' : 'vertical'
  const value = quick[axis]
  if (!value) quick[axis] = { near: near(direction), ratio: ratios.reduce((best, value, i) => Math.abs(value - .5) < Math.abs(ratios[best] - .5) ? i : best, 0) }
  else value.ratio = clamp(value.ratio + (value.near === near(direction) ? -1 : 1), 0, ratios.length - 1)
}
export function quickRect(quick: QuickPlacement, ratios: readonly number[] = LAYOUT_RATIOS): WindowRect {
  const axis = (value: AxisPlacement | null): [number, number] => !value ? [0, 1]
    : [value.near ? 0 : 1 - ratios[value.ratio], ratios[value.ratio]]
  const [x, width] = axis(quick.horizontal), [y, height] = axis(quick.vertical)
  return { x, y, width, height }
}
export function quickCaption(quick: QuickPlacement, ratios: readonly number[] = LAYOUT_RATIOS): string {
  const axis = (v: AxisPlacement | null, a: string, b: string): string => v ? `${v.near ? a : b} ${ratioNames[LAYOUT_RATIOS.indexOf(ratios[v.ratio])] ?? Number(ratios[v.ratio].toFixed(3))}` : '1'
  return `${axis(quick.horizontal, '左', '右')} × ${axis(quick.vertical, '上', '下')}`
}
export function layoutRect(rect: WindowRect, area: { width: number; height: number }, gap: number): WindowRect {
  gap = clamp(gap, 0, Math.min(area.width, area.height) / 8)
  return { x: rect.x * area.width + gap / 2, y: rect.y * area.height + gap / 2,
    width: rect.width * area.width - gap, height: rect.height * area.height - gap }
}
function splitRect(rect: WindowRect, axis: Axis, ratio: number): [WindowRect, WindowRect] {
  return axis === 'x'
    ? [{ ...rect, width: rect.width * ratio }, { ...rect, x: rect.x + rect.width * ratio, width: rect.width * (1 - ratio) }]
    : [{ ...rect, height: rect.height * ratio }, { ...rect, y: rect.y + rect.height * ratio, height: rect.height * (1 - ratio) }]
}
export function treeSlots(tree: LayoutTree): LayoutSlot[] {
  const slots: LayoutSlot[] = []
  function walk(node: LayoutNode, rect: WindowRect): void {
    if (node.kind === 'slot') slots.push({ id: node.id, window: node.window, rect })
    else { const [a, b] = splitRect(rect, node.axis, node.ratio); walk(node.first, a); walk(node.second, b) }
  }
  walk(tree.root, { x: 0, y: 0, width: 1, height: 1 })
  return slots
}
function slotNode(node: LayoutNode, id: number): Extract<LayoutNode, { kind: 'slot' }> | undefined {
  return node.kind === 'slot' ? node.id === id ? node : undefined : slotNode(node.first, id) ?? slotNode(node.second, id)
}
export function importTree(windows: LayoutWindow[], target: number | null, area: { width: number; height: number }): LayoutTree {
  type Entry = { slot: number; window: LayoutWindow }
  function build(values: Entry[], rect: WindowRect): LayoutNode {
    if (values.length === 1) return { kind: 'slot', id: values[0].slot, window: values[0].window.id }
    let best: { score: number; axis: Axis; cut: number; ratio: number; values: Entry[] } | undefined
    for (const axis of ['x', 'y'] as const) {
      values.sort((a, b) => center(a.window, axis) - center(b.window, axis) || a.slot - b.slot)
      const length = axis === 'x' ? rect.width : rect.height
      for (let cut = 1; cut < values.length; cut++) {
        const end = Math.max(...values.slice(0, cut).map(v => v.window[axis] + (axis === 'x' ? v.window.width : v.window.height)))
        const start = Math.min(...values.slice(cut).map(v => v.window[axis]))
        const score = (start - end) / Math.max(1, length)
        if (score > 0 && (!best || score > best.score)) best = { score, axis, cut, ratio: clamp(((start + end) / 2 - rect[axis]) / length, .05, .95), values: [...values] }
      }
    }
    if (!best) {
      const spread = (axis: Axis): number => (Math.max(...values.map(v => center(v.window, axis))) - Math.min(...values.map(v => center(v.window, axis)))) / Math.max(1, axis === 'x' ? rect.width : rect.height)
      const axis = spread('x') >= spread('y') ? 'x' : 'y'
      values.sort((a, b) => center(a.window, axis) - center(b.window, axis) || a.slot - b.slot)
      const cut = Math.floor(values.length / 2)
      best = { score: 0, axis, cut, ratio: cut / values.length, values }
    }
    const [a, b] = splitRect(rect, best.axis, best.ratio)
    return { kind: 'split', axis: best.axis, ratio: best.ratio, first: build(best.values.slice(0, best.cut), a), second: build(best.values.slice(best.cut), b) }
  }
  const tree: LayoutTree = { root: windows.length ? build(windows.map((window, i) => ({ slot: i + 1, window })), { x: 0, y: 0, ...area }) : { kind: 'slot', id: 1, window: null }, selected: 1, nextSlot: Math.max(1, windows.length) + 1 }
  tree.selected = treeSlots(tree).find(s => target !== null && s.window === target)?.id ?? 1
  return tree
}
export function automaticTree(windows: LayoutWindow[], target: number | null, area: { width: number; height: number }, gap: number): LayoutTree | null {
  if (!windows.length) return importTree(windows, target, area)
  const count = windows.length
  const columns = Array.from({ length: count }, (_, i) => i + 1)
  const score = (cols: number) => Math.abs(Math.log((area.width / cols) / (area.height / Math.ceil(count / cols))))
  columns.sort((a, b) => score(a) - score(b))
  function row(start: number, end: number): LayoutNode {
    if (end - start === 1) return { kind: 'slot', id: start + 1, window: windows[start].id }
    const mid = start + Math.floor((end - start) / 2)
    return { kind: 'split', axis: 'x', ratio: (mid - start) / (end - start), first: row(start, mid), second: row(mid, end) }
  }
  function grid(cols: number, start: number, end: number): LayoutNode {
    if (end - start === 1) return row(start * cols, Math.min(end * cols, count))
    const mid = start + Math.floor((end - start) / 2)
    return { kind: 'split', axis: 'y', ratio: (mid - start) / (end - start), first: grid(cols, start, mid), second: grid(cols, mid, end) }
  }
  for (const cols of columns) {
    const tree: LayoutTree = { root: grid(cols, 0, Math.ceil(count / cols)), selected: Math.max(1, windows.findIndex(w => w.id === target) + 1), nextSlot: count + 1 }
    if (fitTree(tree, windows, area, gap)) return tree
  }
  return null
}
export function splitSlot(tree: LayoutTree, direction: LayoutDirection): boolean {
  const used = new Set(treeSlots(tree).map(slot => slot.id))
  if (used.size >= 256) return false
  tree.nextSlot = 1
  while (used.has(tree.nextSlot)) tree.nextSlot++
  function split(node: LayoutNode): LayoutNode {
    if (node.kind === 'slot') {
      if (node.id !== tree.selected) return node
      const empty: LayoutNode = { kind: 'slot', id: tree.nextSlot++, window: null }
      return { kind: 'split', axis: axisOf(direction), ratio: .5, first: near(direction) ? empty : node, second: near(direction) ? node : empty }
    }
    return { ...node, first: split(node.first), second: split(node.second) }
  }
  const old = tree.nextSlot
  tree.root = split(tree.root)
  return old !== tree.nextSlot
}
export function resizeSplit(tree: LayoutTree, direction: LayoutDirection, ratios: readonly number[] = DEFAULT_SPLIT_RATIOS): boolean {
  function resize(node: LayoutNode): boolean {
    if (node.kind === 'slot') return false
    const child = slotNode(node.first, tree.selected) ? node.first : slotNode(node.second, tree.selected) ? node.second : undefined
    if (!child) return false
    if (resize(child)) return true
    if (node.axis !== axisOf(direction)) return false

    const next = near(direction) ? ratios.findLast(r => r < node.ratio - 1e-6) : ratios.find(r => r > node.ratio + 1e-6)
    if (next !== undefined) node.ratio = next
    return true
  }
  return resize(tree.root)
}
export function navigateSlot(tree: LayoutTree, direction: LayoutDirection): void {
  const slots = treeSlots(tree), current = slots.find(s => s.id === tree.selected)
  if (!current) return
  const axis = axisOf(direction), other = axis === 'x' ? 'y' : 'x', sign = near(direction) ? -1 : 1
  const along = (s: LayoutSlot): number => (center(s.rect, axis) - center(current.rect, axis)) * sign
  const score = (s: LayoutSlot): number => along(s) + Math.abs(center(s.rect, other) - center(current.rect, other)) * 2
  const next = slots.filter(s => along(s) > 1e-6).sort((a, b) => score(a) - score(b) || a.id - b.id)[0]
  if (next) tree.selected = next.id
}
export function moveToSlot(tree: LayoutTree, window: number, destination: number): boolean {
  const source = treeSlots(tree).find(s => s.window === window)?.id
  const target = slotNode(tree.root, destination)
  if (!target || source === destination) return false
  const occupant = target.window
  target.window = window
  if (source !== undefined) slotNode(tree.root, source)!.window = occupant
  tree.selected = destination
  return true
}
export function retainTreeWindows(tree: LayoutTree, live: number[]): void {
  function walk(node: LayoutNode): void {
    if (node.kind === 'slot') { if (node.window !== null && !live.includes(node.window)) node.window = null }
    else { walk(node.first); walk(node.second) }
  }
  walk(tree.root)
}
export function fitTree(tree: LayoutTree, windows: LayoutWindow[], area: { width: number; height: number }, gap: number): boolean {
  interface Measure { width: number; height: number; first?: Measure; second?: Measure }
  gap = clamp(gap, 0, Math.min(area.width, area.height) / 8)
  const minimums = new Map(windows.map(w => [w.id, { width: w.minWidth ?? 100, height: w.minHeight ?? 80 }]))
  function measure(node: LayoutNode): Measure {
    if (node.kind === 'slot') {
      const min = node.window === null ? { width: 24, height: 24 } : minimums.get(node.window) ?? { width: 100, height: 80 }
      return { width: min.width + gap, height: min.height + gap }
    }
    const first = measure(node.first), second = measure(node.second)
    return { width: node.axis === 'x' ? first.width + second.width : Math.max(first.width, second.width),
      height: node.axis === 'y' ? first.height + second.height : Math.max(first.height, second.height), first, second }
  }
  function constrain(node: LayoutNode, measured: Measure, rect: WindowRect): void {
    if (node.kind === 'slot') return
    const a = measured.first!, b = measured.second!
    const lo = node.axis === 'x' ? a.width / rect.width : a.height / rect.height
    const hi = node.axis === 'x' ? 1 - b.width / rect.width : 1 - b.height / rect.height
    node.ratio = clamp(node.ratio, lo, Math.max(lo, hi))
    const [first, second] = splitRect(rect, node.axis, node.ratio)
    constrain(node.first, a, first); constrain(node.second, b, second)
  }
  const measured = measure(tree.root)
  if (measured.width > area.width + 1e-6 || measured.height > area.height + 1e-6) return false
  constrain(tree.root, measured, { x: 0, y: 0, ...area })
  return true
}

export function removeSlot(tree: LayoutTree): boolean {
  let removed = false
  function walk(node: LayoutNode): LayoutNode {
    if (node.kind === 'slot') return node
    if (node.first.kind === 'slot' && node.first.id === tree.selected) { removed = true; return node.second }
    if (node.second.kind === 'slot' && node.second.id === tree.selected) { removed = true; return node.first }
    return { ...node, first: walk(node.first), second: walk(node.second) }
  }
  tree.root = walk(tree.root)
  if (removed) tree.selected = treeSlots(tree)[0].id
  return removed
}
export function resizeSplitBy(tree: LayoutTree, direction: LayoutDirection, pixels: number, area: { width: number; height: number }): boolean {
  function resize(node: LayoutNode, rect: WindowRect): boolean {
    if (node.kind === 'slot') return false
    const [a, b] = splitRect(rect, node.axis, node.ratio)
    if (slotNode(node.first, tree.selected)) { if (resize(node.first, a)) return true }
    else if (slotNode(node.second, tree.selected)) { if (resize(node.second, b)) return true }
    else return false
    if (node.axis !== axisOf(direction)) return false
    node.ratio = clamp(node.ratio + (near(direction) ? -pixels : pixels) / Math.max(1, node.axis === 'x' ? rect.width : rect.height), 0, 1)
    return true
  }
  return resize(tree.root, { x: 0, y: 0, ...area })
}


/** Grow/shrink the selected region, fitting every neighbor before committing. */
export function resizeRegionBy(tree: LayoutTree, direction: LayoutDirection, pixels: number, area: { width: number; height: number }, windows: LayoutWindow[], gap: number): boolean {
  if (!Number.isFinite(pixels) || pixels <= 0) return false
  const axis = axisOf(direction), sign = near(direction) ? -1 : 1
  const paths: Array<{ path: boolean[]; first: boolean }> = []
  function collect(node: LayoutNode, path: boolean[]): void {
    if (node.kind === 'slot') return
    const first = !!slotNode(node.first, tree.selected)
    if (!first && !slotNode(node.second, tree.selected)) return
    if (node.axis === axis) paths.push({ path, first })
    collect(first ? node.first : node.second, [...path, !first])
  }
  function measure(value: LayoutTree): [number, number] | undefined {
    const slot = treeSlots(value).find(slot => slot.id === value.selected)
    return slot && (axis === 'x' ? [slot.rect.width * area.width, center(slot.rect, axis) * area.width] : [slot.rect.height * area.height, center(slot.rect, axis) * area.height])
  }
  const original = measure(tree)
  if (!original) return false
  collect(tree.root, [])
  if (!paths.length) return false
  for (const side of [true, false, undefined]) {
    const [size] = measure(tree)!
    const remaining = Math.max(0, pixels - sign * (size - original[0]))
    const step = side === undefined ? remaining : Math.min(remaining, pixels / 2)
    if (step < 1e-6) continue
    let best: LayoutTree | undefined, bestChange = 0, bestDrift = Infinity
    for (const { path, first } of [...paths].reverse()) {
      if (side !== undefined && side !== first) continue
      const candidate = JSON.parse(JSON.stringify(tree)) as LayoutTree
      let node = candidate.root
      for (const second of path) { if (node.kind === 'split') node = second ? node.second : node.first }
      if (node.kind !== 'split') continue
      const fraction = first ? node.ratio : 1 - node.ratio
      node.ratio = clamp(node.ratio + sign * step * fraction / Math.max(size, 1) * (first ? 1 : -1), 0, 1)
      if (!fitTree(candidate, windows, area, gap)) continue
      const [nextSize, nextCenter] = measure(candidate)!
      const change = sign * (nextSize - size), drift = Math.abs(nextCenter - original[1])
      if (change <= 1e-6 || change > step + 1e-6) continue
      if (change > bestChange + 1e-6 || Math.abs(change - bestChange) <= 1e-6 && drift < bestDrift) {
        best = candidate; bestChange = change; bestDrift = drift
      }
    }
    if (best) tree.root = best.root
  }
  return true
}
