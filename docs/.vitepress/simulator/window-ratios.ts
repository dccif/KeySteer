const defaults = [1 / 4, 1 / 3, 1 / 2, 2 / 3, 3 / 4]
const cache = new WeakMap<object, { source: unknown[]; values: number[] }>()

export function splitRatioTicks(input: unknown): { value: number; label: string }[] {
  const values = parseSplitRatios(input)
  const source = (input ?? ['1/4', '1/3', '1/2', '2/3', '3/4']) as (string | number)[]
  return [...values.map(value => ({ value, label: source.filter(text => {
    const parts = String(text).split('/')
    return (parts.length === 2 ? Number(parts[0]) / Number(parts[1]) : Number(text)) === value
  }).map(String).join(' / ') })), { value: 1, label: '1' }]
}

/** Compact screen preview mirrors presentation/key_help/ruler.rs. */
export function quickRulerPlan(ratios: readonly number[], labels: readonly string[], selected: { x: number; y: number; width: number; height: number }, width: number, size: number, availableHeight = 140, aspect = 16 / 9) {
  const font = size * .9
  const label = (value: number) => labels[ratios.findIndex(t => Math.abs(t - value) < 1e-9)] ?? '1'
  const horizontal = label(selected.width), vertical = label(selected.height)
  const textWidth = (text: string) => [...text].reduce((n, c) => n + (c.codePointAt(0)! < 128 ? .75 : 1), 0) * font + 6
  const side = textWidth(vertical) + 10, bottom = font * 1.7 + 10
  const frameWidth = Math.min(Math.max(1, width - side - 8), 250, Math.max(36, availableHeight - bottom - 8) * aspect)
  const frame = { x: 4, y: 4, width: frameWidth, height: frameWidth / aspect }
  const selection = { x: frame.x + frame.width * selected.x, y: frame.y + frame.height * selected.y, width: frame.width * selected.width, height: frame.height * selected.height }
  const ticks = ratios.flatMap(value => [
    { x: frame.x + frame.width * (selected.x > 0 ? 1 - value : value), y: frame.y + frame.height, width: 1, height: Math.abs(value - selected.width) < 1e-9 ? 6 : 3, active: Math.abs(value - selected.width) < 1e-9 },
    { x: frame.x + frame.width, y: frame.y + frame.height * (selected.y > 0 ? 1 - value : value), width: Math.abs(value - selected.height) < 1e-9 ? 6 : 3, height: 1, active: Math.abs(value - selected.height) < 1e-9 },
  ])
  const captions = [
    { text: horizontal, x: Math.max(0, frame.x + frame.width / 2 - textWidth(horizontal) / 2), y: frame.y + frame.height + 8 },
    { text: vertical, x: frame.x + frame.width + 8, y: frame.y + frame.height / 2 - font * .75 },
  ]
  return { height: frame.y + frame.height + bottom, font, frame, selection, ticks, captions }
}

/** Parse configuration fractions once; weak keys do not retain old documents. */
export function parseSplitRatios(input: unknown): readonly number[] {
  if (input === undefined) return defaults
  const error = () => new Error('window_quick.split_ratios 必须是非空数组，可混用分数字符串（如 "1/4"）和小数，每项必须有限、大于 0 且小于 1')
  if (!Array.isArray(input) || input.length === 0) throw error()
  const cached = cache.get(input)
  if (cached && cached.source.length === input.length && cached.source.every((value, i) => value === input[i])) return cached.values
  const values: number[] = []
  for (const text of input) {
    if (typeof text === 'number') {
      if (!Number.isFinite(text) || text <= 0 || text >= 1) throw error()
      values.push(text)
      continue
    }
    if (typeof text !== 'string') throw error()
    const match = /^\s*([+]?\d+)\s*\/\s*([+]?\d+)\s*$/.exec(text)
    if (!match) {
      const value = Number(text)
      if (!/^[+-]?(?:\d+(?:\.\d*)?|\.\d+)(?:[eE][+-]?\d+)?$/.test(text.trim()) || !Number.isFinite(value) || value <= 0 || value >= 1) throw error()
      values.push(value); continue
    }
    const numerator = Number(match[1]), denominator = Number(match[2])
    if (numerator <= 0 || numerator >= denominator || denominator > 0xffffffff) throw error()
    const value = numerator / denominator
    values.push(value)
  }
  const normalized = [...new Set(values.sort((a, b) => a - b))]
  cache.set(input, { source: input.slice(), values: normalized })
  return normalized
}
