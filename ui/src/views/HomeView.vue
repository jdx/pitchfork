<script setup lang="ts">
import { ref, computed, onMounted, onUnmounted } from 'vue'
import { useDaemons } from '@/composables/useApi'
import DaemonTable from '@/components/DaemonTable.vue'
import NamespaceGroup from '@/components/NamespaceGroup.vue'

const { daemons, loading, error, refresh, startPolling, stopPolling } = useDaemons()

const stateDaemons = computed(() =>
  daemons.value.filter(d => !d.is_available),
)

const availableDaemons = computed(() =>
  daemons.value.filter(d => d.is_available),
)

const groupedAvailable = computed(() => {
  const groups = new Map<string, typeof daemons.value>()
  for (const d of availableDaemons.value) {
    const ns = d.id.namespace
    if (!groups.has(ns)) groups.set(ns, [])
    groups.get(ns)!.push(d)
  }
  return new Map([...groups.entries()].sort((a, b) => a[0].localeCompare(b[0])))
})

const sortedDaemons = computed(() =>
  [...daemons.value].sort((a, b) => a.id.qualified.localeCompare(b.id.qualified)),
)

const collapsed = ref<Set<string>>(new Set())

function toggleNs(ns: string) {
  const next = new Set(collapsed.value)
  if (next.has(ns)) next.delete(ns)
  else next.add(ns)
  collapsed.value = next
}

function safeGet(key: string): string | null {
  try { return localStorage.getItem(key) } catch { return null }
}
function safeSet(key: string, value: string) {
  try { localStorage.setItem(key, value) } catch { /* ignore */ }
}

const prefersCard = ref(safeGet('pitchfork.viewMode') === 'card')
function toggleView() {
  prefersCard.value = !prefersCard.value
  safeSet('pitchfork.viewMode', prefersCard.value ? 'card' : 'table')
}

const groupBy = ref(safeGet('pitchfork.groupBy') !== 'ungrouped')
function toggleGroup() {
  groupBy.value = !groupBy.value
  safeSet('pitchfork.groupBy', groupBy.value ? 'grouped' : 'ungrouped')
}

onMounted(startPolling)
onUnmounted(stopPolling)
</script>

<template>
  <div class="home">
    <div class="page-header">
      <div>
        <h1 class="page-title">Daemons</h1>
        <span class="page-subtitle">
          {{ groupBy ? `${stateDaemons.length} active &middot; ${availableDaemons.length} available` : `${daemons.length} total` }}
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
        <button class="btn-ghost" :disabled="loading" @click="refresh">
          <svg class="refresh-icon" :class="{ spin: loading }" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M21 12a9 9 0 0 0-9-9 9.75 9.75 0 0 0-6.74 2.74L3 8"/><path d="M3 3v5h5"/><path d="M3 12a9 9 0 0 0 9 9 9.75 9.75 0 0 0 6.74-2.74L21 16"/><path d="M16 16h5v5"/></svg>
          <span class="btn-text">Refresh</span>
        </button>
      </div>
    </div>

    <div v-if="error" class="alert alert-error">
      <svg class="alert-icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="10"/><line x1="12" y1="8" x2="12" y2="12"/><line x1="12" y1="16" x2="12.01" y2="16"/></svg>
      {{ error }}
    </div>

    <div v-if="daemons.length === 0 && loading" class="loading-skeleton">
      <div v-for="i in 6" :key="i" class="skeleton-row"></div>
    </div>

    <!-- Grouped view -->
    <template v-if="groupBy">
      <!-- Active daemons -->
      <section v-if="stateDaemons.length > 0" class="section">
        <div class="section-header">
          <h2 class="section-title">Active</h2>
          <span class="section-count">{{ stateDaemons.length }}</span>
        </div>
        <DaemonTable
          :daemons="stateDaemons"
          :prefers-card="prefersCard"
          @refresh="refresh"
        />
      </section>

      <!-- Available by namespace -->
      <section v-if="groupedAvailable.size > 0" class="section">
        <div class="section-header">
          <h2 class="section-title">Available</h2>
          <span class="section-count">{{ availableDaemons.length }}</span>
        </div>

<NamespaceGroup
v-for="[ns, group] in groupedAvailable"
:key="ns"
:name="ns"
:count="group.length"
:expanded="!collapsed.has(ns)"
@toggle="toggleNs(ns)"
>
<DaemonTable
:daemons="group"
:prefers-card="prefersCard"
@refresh="refresh"
/>
</NamespaceGroup>
      </section>
    </template>

    <!-- Ungrouped view -->
    <template v-else>
      <section v-if="sortedDaemons.length > 0" class="section">
        <div class="section-header">
          <h2 class="section-title">All Daemons</h2>
          <span class="section-count">{{ sortedDaemons.length }}</span>
        </div>
        <DaemonTable
          :daemons="sortedDaemons"
          :prefers-card="prefersCard"
          @refresh="refresh"
        />
      </section>
    </template>

    <div v-if="daemons.length === 0 && !loading" class="empty-state">
      <svg class="empty-icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"><rect x="2" y="3" width="20" height="14" rx="2" ry="2"/><line x1="8" y1="21" x2="16" y2="21"/><line x1="12" y1="17" x2="12" y2="21"/></svg>
      <h3>No daemons found</h3>
      <p>Define daemons in your config or add a namespace to discover more.</p>
    </div>
  </div>
</template>

<style scoped lang="less">
@import '@/styles/mixins.less';

.home { width: 100%; max-width: @max-content; margin: 0 auto; }

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
.alert-icon { width: 16px; height: 16px; flex-shrink: 0; }

.section { margin-bottom: @space-4xl; }

.section-header { display: flex; align-items: center; gap: 0.4rem; margin-bottom: 0.7rem; }
.section-title { margin: 0; font-size: 0.8rem; font-weight: 600; color: @sf-40; text-transform: uppercase; letter-spacing: 0.06em; }
.section-count { font-size: 0.75rem; color: @sf-20; background: @sf-3; padding: 0.08rem 0.35rem; border-radius: 3px; font-variant-numeric: tabular-nums; }


.empty-state { text-align: center; padding: 3rem @space-xl; border: 1px dashed rgba(255, 255, 255, 0.06); border-radius: @r-2xl; background: @sf-1; }
.empty-icon { width: 48px; height: 48px; color: @sf-8; margin: 0 auto 0.5rem; }
.empty-state h3 { margin: 0 0 0.2rem 0; font-size: 1.05rem; font-weight: 600; color: @sf-45; }
.empty-state p { margin: 0; font-size: 0.85rem; color: @sf-25; }

.mobile({
  .page-title { font-size: 1.1rem; }
  .btn-text { display: none; }
  .btn-view { display: none; }
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
