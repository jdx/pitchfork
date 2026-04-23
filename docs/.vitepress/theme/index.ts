import DefaultTheme from 'vitepress/theme'
import type { Theme } from 'vitepress'
import { h } from 'vue'
import { initBanner } from './banner'
import EndevFooter from './EndevFooter.vue'
import './custom.css'

export default {
  extends: DefaultTheme,
  Layout() {
    return h(DefaultTheme.Layout, null, {
      'layout-bottom': () => h(EndevFooter),
    })
  },
  enhanceApp() {
    initBanner()
  },
} satisfies Theme
