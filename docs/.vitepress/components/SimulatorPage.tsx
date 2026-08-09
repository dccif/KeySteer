import { defineComponent } from 'vue'
import { withBase } from 'vitepress'
import ConfigStudio from './ConfigStudio'
import '../theme/custom.css'

export default defineComponent({
  name: 'SimulatorPage',
  setup() {
    return () => (
      <main class="ks-standalone">
        <header class="ks-standalone-header">
          <a href={withBase('/')}>← 返回文档</a>
          <div>
            <strong>KeySteer 配置与模拟器（beta）</strong>
          </div>
          <a href={withBase('/generated/keysteer.default.toml')} download="keysteer.default.toml">
            查看默认 TOML
          </a>
        </header>
        <ConfigStudio />
      </main>
    )
  },
})
