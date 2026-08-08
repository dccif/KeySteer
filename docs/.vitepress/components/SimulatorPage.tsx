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
          <a href={withBase('/editor/')}>← 返回文档</a>
          <span>KeySteer 配置与模拟器(Beta)</span>
          <a href={withBase('/generated/keysteer.default.toml')} download="keysteer.default.toml">
            默认配置
          </a>
        </header>
        <ConfigStudio />
      </main>
    )
  },
})
