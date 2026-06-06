import { createRouter, createWebHistory } from 'vue-router'
import HomeView from '@/views/HomeView.vue'
import DaemonDetailView from '@/views/DaemonDetailView.vue'
import LogView from '@/views/LogView.vue'
import ProxiesView from '@/views/ProxiesView.vue'

const rawBase = (window as any).__PITCHFORK_BASE__ as string | undefined
const base = rawBase && rawBase !== '__PF_BASE_PLACEHOLDER__' ? rawBase : undefined

const router = createRouter({
  history: createWebHistory(base),
  routes: [
    { path: '/', name: 'home', component: HomeView },
    { path: '/daemon/:id', name: 'daemon', component: DaemonDetailView, props: true },
    { path: '/logs/:id', name: 'logs', component: LogView, props: true },
    { path: '/proxies', name: 'proxies', component: ProxiesView },
  ],
})

export default router
