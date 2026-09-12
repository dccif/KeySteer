/** Semantic help sections mirror presentation/key_help/window.rs. */
export interface HelpEntry { keys: string; action: string; id?: string; returnCandidate?: boolean }
export interface WindowHelpSections { left: HelpEntry[]; right: HelpEntry[]; modes: HelpEntry[]; exit: string; exitLabel: string }

export function windowHelpSections(entries: HelpEntry[], mode: string, resizing = false): WindowHelpSections {
  const actions = new Map(entries.map(entry => [entry.id ?? entry.action, { ...entry }]))
  const destination = entries.filter(entry => entry.returnCandidate).map(entry => entry.id ?? entry.action).sort((a, b) => Number(a === 'idle') - Number(b === 'idle') || a.localeCompare(b))[0] ?? 'idle'
  const exit = actions.get(destination)?.keys ?? ''
  const targetLabel = destination === 'idle' ? 'Idle' : actions.get(destination)?.action ?? destination
  const exitLabel = `${destination === 'idle' ? 'Exit' : 'Back'} → ${targetLabel}`
  actions.delete(destination)
  const take = (name: string, output: HelpEntry[]) => {
    const entry = actions.get(name)
    if (entry) { output.push(entry); actions.delete(name) }
  }
  const pair = (names: string[], caption: string, output: HelpEntry[]) => {
    const entries = names.map(name => actions.get(name))
    if (entries.every(Boolean)) {
      output.push({ keys: entries.map(entry => entry!.keys).join(' / '), action: caption })
      names.forEach(name => actions.delete(name))
    } else names.forEach(name => take(name, output))
  }
  const family = (names: string[], caption: string, output: HelpEntry[]) => {
    const entries = names.map(name => actions.get(name))
    if (entries.every(entry => entry && !entry.keys.includes(' / '))) {
      const parts = entries.map(entry => {
        const index = entry!.keys.lastIndexOf('+')
        return [entry!.keys.slice(0, Math.max(0, index)), entry!.keys.slice(index + 1)]
      })
      if (parts.every(([prefix]) => prefix === parts[0][0])) {
        const leaves = parts.map(([, key]) => key).join('/')
        output.push({ keys: parts[0][0] ? `${parts[0][0]}+${leaves}` : leaves, action: caption })
        names.forEach(name => actions.delete(name)); return
      }
    }
    names.forEach(name => take(name, output))
  }
  const audioScopes = (app: string[], system: string[], label: string, arrows: string, output: HelpEntry[]) => {
    const complete = [...app, ...system].every(name => actions.has(name))
    const shifted = app.every((name, i) => {
      const a = actions.get(name), b = actions.get(system[i])
      return a && b && !a.keys.includes(' / ') && b.keys === `SHIFT+${a.keys}`
    })
    const local: HelpEntry[] = [], global: HelpEntry[] = []
    family(app, `App ${label} ${arrows}`, local)
    family(system, `System ${label} ${arrows}`, global)
    if (complete && local.length === 1 && global.length === 1) {
      output.push({ keys: shifted ? local[0].keys : `${local[0].keys} / ${global[0].keys}`,
        action: shifted ? `${label} ${arrows} · Shift: system` : `App / system ${label} ${arrows}` })
    } else output.push(...local, ...global)
  }
  const operations: HelpEntry[] = []
  if (mode === 'window_tab') family(['move_left', 'move_down', 'move_up', 'move_right'], 'Move / switch tab', operations)
  family(['window_left', 'window_down', 'window_up', 'window_right'], resizing ? 'Resize ←↓↑→' : 'Move ←↓↑→', operations)
  family(['window_layout_left', 'window_layout_down', 'window_layout_up', 'window_layout_right'], mode === 'window_editor' ? 'Select ←↓↑→' : 'Layout ←↓↑→', operations)
  family(['window_split_left', 'window_split_down', 'window_split_up', 'window_split_right'], 'Split ←↓↑→', operations)
  family(['window_ratio_left', 'window_ratio_right'], 'Width − / +', operations)
  family(['window_ratio_up', 'window_ratio_down'], 'Height − / +', operations)
  family(['window_tab_move_left', 'window_tab_move_right'], 'Move tab ← / →', operations)
  pair(['window_tab_next', 'window_tab_previous'], 'Next / previous tab', operations)
  for (const name of ['window_size', 'window_center', 'window_screen_next', 'window_screen_previous', 'size_cycle', 'window_close', 'window_remove_region', 'window_save_layout',
    'Area number', 'window_area_number', 'window_tab_end', 'window_tab_group', 'window_number_end', 'window_tab_remove', 'window_tab_dissolve', 'window_delete', 'window_confirm', 'Previous page', 'Next page']) take(name, operations)
  const modes: HelpEntry[] = [], common: HelpEntry[] = []
  for (const name of ['window_quick', 'window_editor', 'window_restore', 'window_tab']) take(name, modes)
  const captions: Record<string, string> = { window_quick: 'Quick', window_editor: 'Edit', window_restore: 'Restore', window_tab: 'Tabs' }
  modes.forEach(entry => { entry.action = captions[entry.id ?? ''] ?? entry.action })
  pair(['window_select', 'window_select_previous'], 'Next / previous window', common)
  audioScopes(['window_volume_down', 'window_volume_up'], ['window_system_volume_down', 'window_system_volume_up'], 'Volume', '− / +', common)
  audioScopes(['window_audio_previous', 'window_audio_next'], ['window_system_audio_previous', 'window_system_audio_next'], 'Output', '← / →', common)
  audioScopes(['window_volume_mute'], ['window_system_volume_mute'], 'Mute', '/ unmute', common)
  const undo = actions.get('window_undo'), redo = actions.get('window_redo')
  if (undo && redo && !undo.keys.includes(' / ') && !redo.keys.includes(' / ')) {
    common.push({ keys: `${undo.keys} / ${redo.keys}`, action: 'Undo / redo' })
    actions.delete('window_undo'); actions.delete('window_redo')
  }
  for (const name of ['window_select', 'window_select_previous', 'window_undo', 'window_redo', 'window_reset_initial', 'idle', 'window', 'normal', 'key_help']) take(name, common)
  const section = (title: string, entries: HelpEntry[]) => entries.length ? [{ keys: '', action: title }, ...entries] : []
  const title = 'ACTIONS'
  return { left: [...section(title, operations), ...section('OTHER ACTIONS', [...actions.values()].sort((a, b) => (a.id ?? a.action).localeCompare(b.id ?? b.action)))],
    right: section('COMMON', common), modes, exit, exitLabel }
}

export function windowHelpGrid(sections: WindowHelpSections, availableWidth: number): { entries: HelpEntry[]; columns: number } {
  if (availableWidth < 620 || !sections.left.length || !sections.right.length) return { entries: [...sections.left, ...sections.right], columns: 1 }
  const count = Math.max(sections.left.length, sections.right.length)
  const pad = (entries: HelpEntry[]) => Array.from({ length: count }, (_, i) => entries[i] ?? { keys: '', action: '' })
  return { entries: [...pad(sections.left), ...pad(sections.right)], columns: 2 }
}


/** Stable mode capabilities for the cached shortcut reference, independent of input/readiness. */
export function windowHelpActionSupported(mode: string, action: string): boolean {
  const modeEntries = ['window', 'window_quick', 'window_editor', 'window_restore', 'window_tab']
  if (modeEntries.includes(action) || (!action.startsWith('window_') && action !== 'size_cycle')) return true
  if (action === 'window_delete') return mode === 'window_restore'
  if (mode === 'window_restore') return action === 'window_confirm' || action === 'window_delete'
  if (mode === 'window_tab') return action.startsWith('window_tab_') || ['window_number_end', 'window_undo', 'window_redo', 'window_save_layout'].includes(action)
  const common = ['window_audio_previous', 'window_audio_next', 'window_system_volume_down', 'window_system_volume_up', 'window_system_volume_mute', 'window_system_audio_previous', 'window_system_audio_next', 'window_volume_down', 'window_volume_up', 'window_volume_mute', 'window_select', 'window_select_previous', 'window_undo', 'window_redo', 'window_reset_initial']
  if (mode === 'window_quick') return action.startsWith('window_layout_') || common.includes(action)
  if (mode === 'window_editor') return /^window_(layout_|split_|ratio_)/.test(action) || common.includes(action) || ['window_save_layout', 'window_remove_region'].includes(action)
  return common.includes(action) || ['window_tile', 'window_left', 'window_down', 'window_up', 'window_right', 'window_size', 'window_screen_next', 'window_screen_previous', 'size_cycle', 'window_center', 'window_close'].includes(action)
}
