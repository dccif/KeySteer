export type BindingDocument = Record<string, any>

export interface ResolvedBinding {
  value: unknown
  source: string
}

/// Resolve the same ordered mode inheritance used by KeySteer: local bindings
/// win, then each declared source is consulted from left to right. A local
/// `none` deliberately blocks an inherited action.
export function resolveBinding(
  document: BindingDocument,
  mode: string,
  chord: string,
): ResolvedBinding | undefined {
  return collectBindings(document, mode, new Set()).get(chord)
}

export function effectiveBindings(
  document: BindingDocument,
  mode: string,
): Map<string, ResolvedBinding> {
  const all = collectBindings(document, mode, new Set())
  for (const [chord, binding] of all) {
    if (binding.value === 'none') all.delete(chord)
  }
  return all
}

function collectBindings(
  document: BindingDocument,
  mode: string,
  visiting: Set<string>,
): Map<string, ResolvedBinding> {
  if (visiting.has(mode)) return new Map()
  visiting.add(mode)
  const bindings = new Map<string, ResolvedBinding>()
  for (const [configuredKey, value] of Object.entries(bindingTable(document, mode))) {
    for (const chord of configuredKey.split(/\s+/).filter(Boolean)) {
      bindings.set(chord, { value, source: mode })
    }
  }
  for (const source of inheritedModes(document, mode)) {
    for (const [chord, binding] of collectBindings(document, source, visiting)) {
      if (!bindings.has(chord)) bindings.set(chord, binding)
    }
  }
  visiting.delete(mode)
  return bindings
}

function bindingTable(document: BindingDocument, mode: string): Record<string, unknown> {
  if (mode === 'hotkeys') return asRecord(document.hotkeys)
  return asRecord(asRecord(document[mode]).bindings)
}

function inheritedModes(document: BindingDocument, mode: string): string[] {
  if (mode === 'hotkeys') return []
  const inherits = asRecord(document[mode]).inherits
  return Array.isArray(inherits) ? inherits.filter((value): value is string => typeof value === 'string') : []
}

function asRecord(value: unknown): Record<string, any> {
  return value && typeof value === 'object' && !Array.isArray(value) ? value as Record<string, any> : {}
}

function aliasesFor(document: BindingDocument, isMac: boolean): Record<string, string> {
  const source = asRecord(document.key_aliases)
  const platform = asRecord(source[isMac ? 'macos' : 'windows'])
  const aliases: Record<string, string> = { primary: isMac ? 'cmd' : 'ctrl' }
  for (const [key, value] of Object.entries({ ...source, ...platform })) {
    if (typeof value === 'string') aliases[key.toLowerCase()] = value.toLowerCase()
  }
  return aliases
}

function physicalName(key: string, aliases: Record<string, string>): string {
  key = key.toLowerCase()
  const visited = new Set<string>()
  while (aliases[key] && !visited.has(key)) { visited.add(key); key = aliases[key] }
  return key.replace('command', 'cmd').replace('control', 'ctrl').replace('option', 'alt').replace('escape', 'esc')
}

function keyMatches(configured: string, physical: string): boolean {
  return configured === physical || ['shift', 'ctrl', 'alt', 'cmd', 'win'].includes(configured) && physical.endsWith(`_${configured}`)
}

/** Physical matching used by Window's independently configurable bindings. */
export function resolvePhysicalBinding(document: BindingDocument, mode: string, pressed: string[], key: string, isMac: boolean, localOnly = false): ResolvedBinding | undefined {
  const aliases = aliasesFor(document, isMac)
  let best: ResolvedBinding | undefined, specificity = 0
  const table = localOnly ? collectBindings({ [mode]: { bindings: bindingTable(document, mode) } }, mode, new Set()) : collectBindings(document, mode, new Set())
  for (const [chord, binding] of table) {
    const keys = chord === '+' ? ['+'] : chord.split('+').map(v => physicalName(v, aliases))
    if (!keys.some(k => keyMatches(k, key)) || !keys.every(k => pressed.some(p => keyMatches(k, p)))) continue
    if (pressed.some(p => /^(left_|right_)?(shift|ctrl|alt|cmd|win)$/.test(p) && !keys.some(k => keyMatches(k, p)))) continue
    if (keys.length > specificity) { specificity = keys.length; best = binding }
  }
  return best
}

export function temporaryPhysicalKeys(document: BindingDocument, mode: string, pressed: string[], isMac: boolean): string[] {
  const settings = asRecord(document[mode])
  if (!settings.temporary_mode) return []
  const aliases = aliasesFor(document, isMac)
  const active = new Set<string>()
  for (const chord of Array.isArray(settings.temporary_mode_keys) ? settings.temporary_mode_keys : []) {
    const keys = String(chord).split('+').map(k => physicalName(k, aliases))
    if (keys.every(k => pressed.some(p => keyMatches(k, p)))) {
      pressed.filter(p => keys.some(k => keyMatches(k, p))).forEach(p => active.add(p))
    }
  }
  return [...active]
}
