import { defineComponent } from 'vue'
import { useData, withBase } from 'vitepress'
import ConfigStudio from './ConfigStudio'
import { provideStudioLocale } from '../config-studio/i18n'
import '../theme/custom.css'

export default defineComponent({
  name: 'SimulatorPage',
  setup() {
    const { lang } = useData()
    const locale = provideStudioLocale(lang)
    const english = () => locale.value === 'en'
    return () => (
      <main class="ks-standalone" lang={english() ? 'en' : 'zh-CN'}>
        <header class="ks-standalone-header">
          <a href={withBase(english() ? '/en/' : '/')}>← {english() ? 'Back to documentation' : '返回文档'}</a>
          <div>
            <strong>{english() ? 'KeySteer Configuration & Simulator (beta)' : 'KeySteer 配置与模拟器（beta）'}</strong>
          </div>
          <select class="ks-language-picker" aria-label={english() ? 'Interface language' : '界面语言'} value={locale.value} onChange={event => { locale.value = (event.target as HTMLSelectElement).value as 'zh' | 'en' }}>
            <option value="zh">简体中文</option><option value="en">English</option>
          </select>
          <a href={withBase('/generated/keysteer.default.toml')} download="keysteer.default.toml">
            {english() ? 'View default TOML' : '查看默认 TOML'}
          </a>
        </header>
        <ConfigStudio />
      </main>
    )
  },
})
