<script setup lang="ts">
import { useRoute } from 'vue-router'
import { Toaster } from 'vue-sonner'

const route = useRoute()
const active = (name: string) => route.name === name ? 'active' : ''
const logoUrl = '/img/logo.png'
</script>

<template>
  <nav class="nav">
    <div class="nav-inner">
      <router-link to="/" class="logo">
        <img :src="logoUrl" alt="Pitchfork" />
        <span class="logo-text">pitchfork</span>
      </router-link>
      <div class="nav-links">
        <router-link to="/" :class="['link', active('home')]" title="Dashboard">
          <span class="icon">◈</span>
          <span class="label">Daemons</span>
        </router-link>
        <router-link to="/proxies" :class="['link', active('proxies')]" title="Proxies">
          <span class="icon">⧉</span>
          <span class="label">Proxies</span>
        </router-link>
      </div>
    </div>
  </nav>
  <main class="app">
    <RouterView />
  </main>
  <Toaster position="bottom-right" theme="dark" />
</template>

<style scoped lang="less">
@import '@/styles/mixins.less';

.nav {
  position: sticky;
  top: 0;
  z-index: @z-nav;
  background: rgba(0, 0, 0, 0.75);
  backdrop-filter: blur(20px) saturate(1.8);
  -webkit-backdrop-filter: blur(20px) saturate(1.8);
  border-bottom: 1px solid @sf-6;
}

.nav-inner {
  max-width: @max-page;
  margin: 0 auto;
  padding: 0 @space-3xl;
  display: flex;
  align-items: center;
  height: 56px;
  gap: @space-3xl;
}

.logo {
  display: flex;
  align-items: center;
  flex-shrink: 0;
  gap: @space-md;
  text-decoration: none;

  img { height: 24px; width: auto; filter: brightness(1.2); }
}

.logo-text {
  font-family: @ff-brand;
  font-size: 1.5rem;
  font-weight: 400;
  color: @c-accent;
  text-shadow: @c-accent-glow 0px 0px 17.9008px, rgba(255, 69, 0, 0.498) 0px 0px 34.8347px;
  white-space: nowrap;
  line-height: 1;
  -webkit-font-smoothing: antialiased;
}

.nav-links { display: flex; align-items: center; gap: @space-sm; flex: 1; }

.link {
  display: flex;
  align-items: center;
  gap: 0.4rem;
  padding: 0.4rem 0.8rem;
  border-radius: @r-lg;
  color: @sf-40;
  text-decoration: none;
  font-size: 0.85rem;
  font-weight: 500;
  transition: @tr-base;

  &:hover { background: @sf-4; color: @sf-70; }
  &.active { background: @sf-accent-10; color: @c-accent-dim; }
}

.icon { font-size: 0.9em; opacity: 0.7; }

.app { max-width: @max-page; margin: 0 auto; padding: @space-3xl; }
</style>

<style>
body {
  font-family: 'Inter', -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, sans-serif;
  -webkit-font-smoothing: antialiased;
  -moz-osx-font-smoothing: grayscale;
}

html {
  scrollbar-width: thin;
  scrollbar-color: rgba(255, 255, 255, 0.08) transparent;
}

::-webkit-scrollbar { width: 6px; height: 6px; }
::-webkit-scrollbar-track { background: transparent; }
::-webkit-scrollbar-thumb { background: rgba(255, 255, 255, 0.08); border-radius: 3px; }
::-webkit-scrollbar-thumb:hover { background: rgba(255, 255, 255, 0.15); }
</style>
