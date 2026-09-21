export type SettingsTab = 'keys' | 'behavior' | 'appearance'
export const tabLabels: Record<SettingsTab, string> = { keys: '按键', behavior: '行为', appearance: '外观' }
export const categories = [
  { id: 'mouse', label: '鼠标控制' }, { id: 'target', label: '目标定位' },
  { id: 'windows', label: '窗口管理' }, { id: 'global', label: '全局设置' },
  { id: 'files', label: '配置文件' },
] as const
export interface SettingsPage { id: string; category: string; label: string; mode?: string; tabs: SettingsTab[] }
export const pages: SettingsPage[] = [
  { id: 'normal', category: 'mouse', label: '鼠标控制 · Normal', mode: 'normal', tabs: ['keys', 'behavior'] },
  ...[['grid', '分层网格 · Grid'], ['recursive_grid', '递归网格 · Recursive Grid'], ['ui_hint', '界面定位 · UI Hint']].map(([id, label]) => ({ id, label, category: 'target', mode: id, tabs: ['keys', 'behavior', 'appearance'] as SettingsTab[] })),
  ...[['window', '移动与缩放'], ['window_quick', '快速布局'], ['window_editor', '布局编辑'], ['window_restore', '布局恢复'], ['window_tab', '标签分组']].map(([id, label]) => ({ id, label, category: 'windows', mode: id, tabs: ['keys', 'behavior', 'appearance'] as SettingsTab[] })),
  { id: 'window_card', category: 'windows', label: '共用外观', tabs: ['appearance'] },
  { id: 'hotkeys', category: 'global', label: '全局快捷键', mode: 'hotkeys', tabs: ['keys'] },
  { id: 'quick_switch', category: 'global', label: '快速切换', tabs: ['behavior', 'appearance'] },
  { id: 'key_help', category: 'global', label: '提示与主题', tabs: ['appearance'] },
  { id: 'mode_usage', category: 'global', label: '使用统计', tabs: ['behavior'] },
  { id: 'files', category: 'files', label: '导入导出', tabs: [] },
  { id: 'toml', category: 'files', label: 'TOML 查看', tabs: [] },
]

/** The UI organization never changes the persisted TOML path. */
export function fieldLocation(path: string): { page: string; tab: SettingsTab } {
  const root = path.split('.')[0]
  if (root === 'pointer' || root === 'scroll' || root === 'normal') return { page: 'normal', tab: 'behavior' }
  if (root === 'theme' || root === 'key_help') return { page: 'key_help', tab: 'appearance' }
  if (path.startsWith('window.card.')) return { page: 'window_card', tab: 'appearance' }
  const visual = /\.(ui|card)\./.test(path) || /\.(border_width|placement|label_[a-z_]+|sub_key_[a-z_]+)$/.test(path)
  return { page: root, tab: visual ? 'appearance' : 'behavior' }
}
export interface SearchEntry { path: string; label: string; page: string; tab: SettingsTab }
export const utilitySearchFields: SearchEntry[] = [
  ...Object.entries({ key: '触发键', enabled: '启用快速切换', hold_ms: '长按毫秒', position: '面板位置', blacklist: '面板黑名单' }).map(([field, label]) => ({ path: `quick_switch.${field}`, label, page: 'quick_switch', tab: 'behavior' as const })),
  ...Object.entries({ font_size: '字号', border_width: '边框宽度', border_radius: '圆角', padding_x: '左右内边距', padding_y: '上下内边距', background_color: '背景颜色', text_color: '文字颜色', border_color: '边框颜色' }).map(([field, label]) => ({ path: `quick_switch.ui.${field}`, label: `快速切换 ${label}`, page: 'quick_switch', tab: 'appearance' as const })),
  { path: 'mode_usage.save_after_entries', label: '累计进入次数后保存', page: 'mode_usage', tab: 'behavior' },
]
export function searchSettings(entries: SearchEntry[], query: string): SearchEntry[] {
  const words = query.trim().toLocaleLowerCase().split(/\s+/).filter(Boolean)
  if (!words.length) return []
  return entries.filter(entry => {
    const page = pages.find(page => page.id === entry.page)
    const haystack = `${entry.path} ${entry.label} ${page?.label} ${page?.mode ?? ''}`.toLocaleLowerCase()
    return words.every(word => haystack.includes(word))
  }).slice(0, 30)
}
