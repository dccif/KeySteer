import { defineComponent } from 'vue'
import { useData, withBase } from 'vitepress'
import ConfigStudio from './ConfigStudio'
import '../theme/custom.css'

export default defineComponent({
  name: 'SimulatorPage',
  setup() {
    const { lang } = useData()
    const english = () => lang.value === 'en-US'
    return () => (
      <main class="ks-standalone">
        <header class="ks-standalone-header">
          <a href={withBase(english() ? '/en/' : '/')}>← {english() ? 'Back to documentation' : '返回文档'}</a>
          <div>
            <strong>{english() ? 'KeySteer Configuration & Simulator (beta)' : 'KeySteer 配置与模拟器（beta）'}</strong>
          </div>
          <a href={withBase('/generated/keysteer.default.toml')} download="keysteer.default.toml">
            {english() ? 'View default TOML' : '查看默认 TOML'}
          </a>
        </header>
        <ConfigStudio />
      </main>
    )
  },
})
