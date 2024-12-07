import { defineConfig } from 'vitepress'

// https://vitepress.dev/reference/site-config
export default defineConfig({
  title: "pitchfork",
  description: "Daemons with DX",
  themeConfig: {
    // https://vitepress.dev/reference/default-theme-config
    nav: [
      { text: 'Home', link: '/' },
    ],

    sidebar: [
      { text: 'Getting Started', link: '/getting-started' },
    ],

    socialLinks: [
      { icon: 'github', link: 'https://github.com/jdx/pitchfork' }
    ],

    logo: '/logo.png',
  },
  head: [
    ['link', { rel: 'icon', href: '/img/favicon.ico' }],
  ],
})
