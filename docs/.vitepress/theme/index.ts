import DefaultTheme from 'vitepress/theme'
import type { Theme } from 'vitepress'
import { h, onMounted } from 'vue'
import { initBanner } from './banner'
import EndevFooter from './EndevFooter.vue'
import { data as starsData } from '../stars.data'
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
  setup() {
    onMounted(() => {
      const addStarCount = () => {
        const githubLink = document.querySelector(
          '.VPSocialLinks a[href*="github.com/jdx/pitchfork"]',
        )
        if (githubLink && !githubLink.querySelector('.star-count')) {
          const starBadge = document.createElement('span')
          starBadge.className = 'star-count'
          starBadge.textContent = starsData.stars
          starBadge.title = 'GitHub Stars'
          githubLink.appendChild(starBadge)
        }
      }

      addStarCount()
      setTimeout(addStarCount, 100)
      const observer = new MutationObserver(addStarCount)
      observer.observe(document.body, { childList: true, subtree: true })
    })
  },
} satisfies Theme
