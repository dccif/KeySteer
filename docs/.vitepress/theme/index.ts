import { defineComponent, h } from 'vue'
import DefaultTheme from 'vitepress/theme'
import DownloadButton from '../components/DownloadSection'
import './custom.css'

const Layout = defineComponent({
  name: 'KeySteerLayout',
  setup() {
    return () => h(DefaultTheme.Layout, null, {
      'home-hero-info-after': () => h(DownloadButton),
    })
  },
})

export default {
  ...DefaultTheme,
  Layout,
}
