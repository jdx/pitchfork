import DefaultTheme from 'vitepress/theme'
import type { Theme } from 'vitepress'
import { initBanner } from './banner'
import './custom.css'

export default {
  extends: DefaultTheme,
  enhanceApp() {
    initBanner()
  },
} satisfies Theme
