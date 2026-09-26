import { parse, stringify } from 'smol-toml'

export interface ConfigChange {
  path: string
  kind: 'added' | 'removed' | 'modified'
  before?: string
  after?: string
}

const table = (value: unknown): value is Record<string, unknown> =>
  value !== null && typeof value === 'object' && !Array.isArray(value) && !(value instanceof Date)

// Sort tables recursively, but retain array order and TOML scalar types.
function canonical(value: unknown): unknown {
  if (Array.isArray(value)) return value.map(canonical)
  if (table(value)) return Object.fromEntries(Object.keys(value).sort().map(key => [key, canonical(value[key])]))
  return value
}

function display(value: unknown): string | undefined {
  return value === undefined ? undefined : stringify({ value: canonical(value) } as Parameters<typeof stringify>[0]).trim().replace(/^value = /, '')
}

/** Compare explicit TOML values, without applying KeySteer defaults or schema restrictions. */
export function diffToml(before: string, after: string): ConfigChange[] {
  const changes: ConfigChange[] = []
  function visit(left: unknown, right: unknown, path: string[]) {
    // Unchanged scalars (including NaN) need no TOML serialization.
    if (Object.is(left, right)) return
    if (table(left) && table(right)) {
      for (const key of [...new Set([...Object.keys(left), ...Object.keys(right)])].sort()) {
        visit(Object.hasOwn(left, key) ? left[key] : undefined, Object.hasOwn(right, key) ? right[key] : undefined, [...path, key])
      }
      return
    }
    const oldValue = display(left), newValue = display(right)
    if (oldValue === newValue) return
    changes.push({ path: path.map(key => /^[A-Za-z0-9_-]+$/.test(key) ? key : JSON.stringify(key)).join('.'),
      kind: left === undefined ? 'added' : right === undefined ? 'removed' : 'modified', before: oldValue, after: newValue })
  }
  visit(parse(before), parse(after), [])
  return changes
}
