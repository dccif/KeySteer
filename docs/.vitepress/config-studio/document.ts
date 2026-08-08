import { toRaw } from 'vue'

export type ConfigDocument = Record<string, any>

/** Clone Vue-backed configuration state into a plain writable document. */
export function cloneConfigDocument(document: ConfigDocument): ConfigDocument {
  return structuredClone(toRaw(document))
}
