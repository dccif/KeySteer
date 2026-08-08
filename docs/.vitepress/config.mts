import vueJsx from '@vitejs/plugin-vue-jsx'
import { withMermaid } from 'vitepress-plugin-mermaid'

export default withMermaid({
  lang: 'zh-CN',
  title: 'KeySteer',
  description: '用键盘操控鼠标的原生小工具：快、轻量、可定制。',
  cleanUrls: true,
  ignoreDeadLinks: [/^\/generated\//],
  head: [
    ['link', { rel: 'icon', type: 'image/png', href: '/generated/keysteer-icon.png' }],
    ['meta', { name: 'theme-color', content: '#6578d4' }],
  ],
  vite: {
    plugins: [vueJsx()],
    ssr: {
      noExternal: ['vitepress-plugin-mermaid', 'mermaid'],
    },
  },
  themeConfig: {
    logo: '/generated/keysteer-icon.png',
    nav: [
      { text: '快速上手', link: '/guide/getting-started' },
      { text: '模式', link: '/modes/' },
      { text: '配置参考', link: '/reference/configuration' },
      { text: '一起开发', link: '/development/architecture' },
      { text: '配置与模拟器', link: '/editor/' },
    ],
    sidebar: [
      {
        text: '开始使用',
        items: [
          { text: '快速上手', link: '/guide/getting-started' },
          { text: 'macOS', link: '/guide/macos' },
        ],
      },
      {
        text: '模式',
        items: [
          { text: '模式总览', link: '/modes/' },
          { text: 'Normal 普通模式', link: '/modes/normal' },
          { text: 'Grid 网格模式', link: '/modes/grid' },
          { text: 'Recursive Grid 递归网格', link: '/modes/recursive-grid' },
          { text: 'UI 标签模式', link: '/modes/ui-hint' },
        ],
      },
      {
        text: '参考',
        items: [
          { text: '配置文件', link: '/reference/configuration' },
          { text: '模式与动作', link: '/reference/modes-and-actions' },
        ],
      },
      {
        text: '开发',
        items: [
          { text: '架构', link: '/development/architecture' },
          { text: '扩展指南', link: '/development/extension-guide' },
          { text: '开发流程与测试', link: '/development/workflow' },
          { text: '插件开发', link: '/development/plugin-development' },
        ],
      },
      {
        text: '工具',
        items: [{ text: '配置与模拟器', link: '/editor/' }],
      },
    ],
    socialLinks: [{ icon: 'github', link: 'https://github.com/dccif/mousemover' }],
    search: {
      provider: 'local',
      options: {
        translations: {
          button: {
            buttonText: '搜索',
            buttonAriaLabel: '搜索文档',
          },
          modal: {
            displayDetails: '显示详细结果',
            resetButtonTitle: '清除搜索',
            backButtonTitle: '关闭搜索',
            noResultsText: '没有找到匹配结果',
            footer: {
              selectText: '选择',
              selectKeyAriaLabel: 'Enter',
              navigateText: '切换',
              navigateUpKeyAriaLabel: '上箭头',
              navigateDownKeyAriaLabel: '下箭头',
              closeText: '关闭',
              closeKeyAriaLabel: 'Esc',
            },
          },
        },
      },
    },
    outline: { label: '本页内容' },
    docFooter: { prev: '上一页', next: '下一页' },
    lastUpdated: { text: '最近更新' },
  },
})
