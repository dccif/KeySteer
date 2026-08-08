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
