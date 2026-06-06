<script setup lang="ts">
import { shallowRef, ref, computed, onMounted, onUnmounted } from 'vue'
import { api } from '@/composables/useApi'
import type { ProxyWorktreeEntry } from '@/types/api'
import ProxyTable from '@/components/ProxyTable.vue'
import NamespaceGroup from '@/components/NamespaceGroup.vue'

const POLL_INTERVAL = 3000

const proxies = shallowRef<ProxyWorktreeEntry[]>([])
const loading = ref(true)
const fetching = ref(false)
const error = ref<string | null>(null)
let timer: ReturnType<typeof setInterval> | null = null

async function fetchProxies() {
  if (fetching.value) return
  fetching.value = true
  try {
    loading.value = true
    error.value = null
    proxies.value = await api<ProxyWorktreeEntry[]>('/proxies')
  } catch (e: any) {
    error.value = e.message ?? 'Failed to load proxies'
  } finally {
    fetching.value = false
    loading.value = false
  }
}

function startPolling() {
  if (timer) return
  fetchProxies()
  timer = setInterval(fetchProxies, POLL_INTERVAL)
}
function stopPolling() {
  if (timer) {
    clearInterval(timer)
    timer = null
  }
}

onMounted(startPolling)
onUnmounted(stopPolling)

function safeGet(key: string): string | null {
  try { return localStorage.getItem(key) } catch { return null }
}
function safeSet(key: string, value: string) {
  try { localStorage.setItem(key, value) } catch { /* ignore */ }
}

const groupBy = ref(safeGet('pitchfork.proxyGroupBy') !== 'ungrouped')
function toggleGroup() {
  groupBy.value = !groupBy.value
  safeSet('pitchfork.proxyGroupBy', groupBy.value ? 'grouped' : 'ungrouped')
}

const prefersCard = ref(safeGet('pitchfork.proxyViewMode') === 'card')
function toggleView() {
  prefersCard.value = !prefersCard.value
  safeSet('pitchfork.proxyViewMode', prefersCard.value ? 'card' : 'table')
}

const collapsed = ref<Set<string>>(new Set())
function toggleDaemon(daemon: string) {
  const next = new Set(collapsed.value)
  if (next.has(daemon)) next.delete(daemon)
  else next.add(daemon)
  collapsed.value = next
}

const sortedProxies = computed(() =>
  [...proxies.value].sort((a, b) => {
    const qa = a.daemon_qualified || a.slug
    const qb = b.daemon_qualified || b.slug
    return qa.localeCompare(qb)
  }),
)

const groupedProxies = computed(() => {
  const groups = new Map<string, ProxyWorktreeEntry[]>()
  for (const p of proxies.value) {
    const key = p.daemon_qualified || p.daemon_name || p.slug
    if (!groups.has(key)) groups.set(key, [])
    groups.get(key)!.push(p)
  }
  return new Map(
    [...groups.entries()]
      .sort((a, b) => a[0].localeCompare(b[0]))
      .map(([k, v]) => [k, v.sort((a, b) => a.branch.localeCompare(b.branch))]),
  )
})

const activeCount = computed(() => proxies.value.filter(p => p.status === 'running').length)
</script>

<template>
  <div class="proxies">
    <div class="page-header">
      <div>
        <h1 class="page-title">Proxies</h1>
        <span class="page-subtitle">
          {{ groupBy ? `${activeCount} active &middot; ${proxies.length} total` : `${proxies.length} worktrees` }}
        </span>
      </div>
      <div class="header-actions">
        <button class="btn-ghost" @click="toggleGroup" title="Toggle grouping">
          <svg v-if="groupBy" class="icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M22 19a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h5l2 3h9a2 2 0 0 1 2 2z"/></svg>
          <svg v-else class="icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><line x1="8" y1="6" x2="21" y2="6"/><line x1="8" y1="12" x2="21" y2="12"/><line x1="8" y1="18" x2="21" y2="18"/><line x1="3" y1="6" x2="3.01" y2="6"/><line x1="3" y1="12" x2="3.01" y2="12"/><line x1="3" y1="18" x2="3.01" y2="18"/></svg>
        </button>
        <button class="btn-ghost btn-view" @click="toggleView" title="Toggle view">
          <svg v-if="prefersCard" class="icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><rect x="3" y="3" width="7" height="7"/><rect x="14" y="3" width="7" height="7"/><rect x="3" y="14" width="7" height="7"/><rect x="14" y="14" width="7" height="7"/></svg>
          <svg v-else class="icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><rect x="3" y="3" width="18" height="18" rx="2" ry="2"/><line x1="3" y1="9" x2="21" y2="9"/><line x1="3" y1="15" x2="21" y2="15"/><line x1="9" y1="3" x2="9" y2="21"/></svg>
        </button>
        <button class="btn-ghost" :disabled="loading" @click="fetchProxies">
          <svg class="refresh-icon" :class="{ spin: loading }" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M21 12a9 9 0 0 0-9-9 9.75 9.75 0 0 0-6.74 2.74L3 8"/><path d="M3 3v5h5"/><path d="M3 12a9 9 0 0 0 9 9 9.75 9.75 0 0 0 6.74-2.74L21 16"/><path d="M16 16h5v5"/></svg>
          <span class="btn-text">Refresh</span>
        </button>
      </div>
    </div>

    <div v-if="error" class="alert alert-error">{{ error }}</div>

    <div v-if="loading && proxies.length === 0" class="loading-skeleton">
      <div v-for="i in 6" :key="i" class="skeleton-row"></div>
    </div>

    <!-- Grouped view -->
    <template v-if="groupBy">
      <div v-if="groupedProxies.size === 0 && !loading" class="empty-state">
        <svg class="empty-icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"><path d="M10 13a5 5 0 0 0 7.54.54l3-3a5 5 0 0 0-7.07-7.07l-1.72 1.71"/><path d="M14 11a5 5 0 0 0-7.54-.54l-3 3a5 5 0 0 0 7.07 7.07l1.71-1.71"/></svg>
        <h3>No proxies registered</h3>
        <p>Add slugs in your global config under [slugs].</p>
      </div>

      <NamespaceGroup
        v-for="[daemon, group] in groupedProxies"
        :key="daemon"
        :name="daemon"
        :count="group.length"
        :expanded="!collapsed.has(daemon)"
        @toggle="toggleDaemon(daemon)"
      >
        <ProxyTable
          :proxies="group"
          :prefers-card="prefersCard"
          @refresh="fetchProxies"
        />
      </NamespaceGroup>
    </template>

    <!-- Flat view -->
    <template v-else>
      <div v-if="proxies.length === 0 && !loading" class="empty-state">
        <svg class="empty-icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"><path d="M10 13a5 5 0 0 0 7.54.54l3-3a5 5 0 0 0-7.07-7.07l-1.72 1.71"/><path d="M14 11a5 5 0 0 0-7.54-.54l-3 3a5 5 0 0 0 7.07 7.07l1.71-1.71"/></svg>
        <h3>No proxies registered</h3>
        <p>Add slugs in your global config under [slugs].</p>
      </div>
      <ProxyTable
        v-if="proxies.length > 0"
        :proxies="sortedProxies"
        :prefers-card="prefersCard"
        @refresh="fetchProxies"
      />
    </template>
  </div>
</template>

<style scoped lang="less">
@import '@/styles/mixins.less';

.proxies { width: 100%; max-width: @max-content; margin: 0 auto; }

.page-header { .flex-between(); margin-bottom: @space-xl; gap: @space-xl; padding-bottom: 0.6rem; border-bottom: 1px solid rgba(255, 255, 255, 0.04); }

.page-title { margin: 0; font-size: 1.4rem; font-weight: 700; color: @c-white; letter-spacing: -0.02em; }
.page-subtitle { font-size: 0.8rem; color: @sf-30; }

.header-actions { display: flex; align-items: center; gap: 0.4rem; }

.btn-ghost {
  .ghost-btn();

  .icon { width: 14px; height: 14px; }
}

.refresh-icon { width: 14px; height: 14px; }
.refresh-icon.spin { animation: spin 1s linear infinite; }

.alert { .alert-error(); }

.empty-state { text-align: center; padding: 3rem @space-xl; border: 1px dashed rgba(255, 255, 255, 0.06); border-radius: @r-2xl; background: @sf-1; }
.empty-icon { width: 48px; height: 48px; color: @sf-8; margin: 0 auto 0.5rem; }
.empty-state h3 { margin: 0 0 0.2rem 0; font-size: 1.05rem; font-weight: 600; color: @sf-45; }
.empty-state p { margin: 0; font-size: 0.85rem; color: @sf-25; }

.mobile({
  .btn-ghost .btn-text { display: none; }
  .btn-ghost { padding: 0.5rem; }
  .btn-view { display: none; }
  .page-title { font-size: 1.2rem; }
});

.loading-skeleton {
  display: flex;
  flex-direction: column;
  gap: 0.5rem;
  padding: 0.5rem 0;
}

.skeleton-row {
  height: 3.2rem;
  border-radius: @r-xl;
  background: rgba(255, 255, 255, 0.03);
  animation: pulse 1.5s ease-in-out infinite;
}

.skeleton-row:nth-child(2) { animation-delay: 0.15s; }
.skeleton-row:nth-child(3) { animation-delay: 0.30s; }
.skeleton-row:nth-child(4) { animation-delay: 0.45s; }
.skeleton-row:nth-child(5) { animation-delay: 0.60s; }
.skeleton-row:nth-child(6) { animation-delay: 0.75s; }

@keyframes pulse {
  0%, 100% { opacity: 0.4; }
  50% { opacity: 0.7; }
}
</style>
