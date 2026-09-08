const defaults = [1 / 4, 1 / 3, 1 / 2, 2 / 3, 3 / 4]
const cache = new WeakMap<object, { source: unknown[]; values: number[] }>()

/** Parse configuration fractions once; weak keys do not retain old documents. */
export function parseSplitRatios(input: unknown): readonly number[] {
  if (input === undefined) return defaults
  const error = () => new Error('window.split_ratios 必须是非空数组，可混用分数字符串（如 "1/4"）和小数，每项必须有限、大于 0 且小于 1')
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
    if (!match) throw error()
    const numerator = Number(match[1]), denominator = Number(match[2])
    if (numerator <= 0 || numerator >= denominator || denominator > 0xffffffff) throw error()
    const value = numerator / denominator
    values.push(value)
  }
  const normalized = [...new Set(values.sort((a, b) => a - b))]
  cache.set(input, { source: input.slice(), values: normalized })
  return normalized
}
