import { createRouter, createWebHistory } from 'vue-router'
import HomeView from '@/views/HomeView.vue'
import DaemonDetailView from '@/views/DaemonDetailView.vue'
import LogView from '@/views/LogView.vue'
import ProxiesView from '@/views/ProxiesView.vue'
import ProjectsView from '@/views/ProjectsView.vue'
import ProjectView from '@/views/ProjectView.vue'
import StackView from '@/views/StackView.vue'

const rawBase = (window as any).__PITCHFORK_BASE__ as string | undefined
const base = rawBase && rawBase !== '__PF_BASE_PLACEHOLDER__' ? rawBase : undefined

const router = createRouter({
  history: createWebHistory(base),
  routes: [
    { path: '/', name: 'home', component: HomeView },
    { path: '/daemon/:id', name: 'daemon', component: DaemonDetailView, props: true },
    { path: '/logs/:id', name: 'logs', component: LogView, props: true },
    { path: '/proxies', name: 'proxies', component: ProxiesView },
    { path: '/projects', name: 'projects', component: ProjectsView },
    { path: '/projects/:project', name: 'project', component: ProjectView, props: true },
    {
      path: '/projects/:project/:worktree',
      name: 'stack',
      component: StackView,
      props: true,
    },
  ],
})

export default router
