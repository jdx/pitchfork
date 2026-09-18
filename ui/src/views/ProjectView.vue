<script setup lang="ts">
import { computed, ref, toRef } from 'vue'
import { useProject } from '@/composables/useApi'
import { formatBytes, formatSince } from '@/utils/format'
import StackGroups from '@/components/StackGroups.vue'

const props = defineProps<{ project: string }>()
const { project: data, loading, error, refresh } = useProject(toRef(props, 'project'))

function safeGet(key: string): string | null {
  try { return localStorage.getItem(key) } catch { return null }
}
const prefersCard = ref(safeGet('pitchfork.viewMode') === 'card')

const worktrees = computed(() => data.value?.worktrees ?? [])

// Only show disk usage when pitchfork actually knows it for every worktree;
// a partly-known column would read as "0 bytes used".
const showDisk = computed(() =>
  worktrees.value.length > 0 && worktrees.value.every(w => w.disk_usage_bytes != null),
)
</script>

<template>
  <div class="project">
    <div class="page-header">
      <div>
        <div class="crumbs">
          <router-link to="/projects" class="crumb">Projects</router-link>
          <span class="crumb-sep">/</span>
        </div>
        <h1 class="page-title">{{ project }}</h1>
        <span class="page-subtitle">
          <span v-if="data">{{ data.dir }} &middot; {{ data.daemons.running }}/{{ data.daemons.total }} daemons running</span>
        </span>
      </div>
      <button class="btn-ghost" :disabled="loading" @click="refresh">Refresh</button>
    </div>

    <div v-if="error" class="alert alert-error">{{ error }}</div>

    <template v-if="data">
      <section class="section">
        <div class="section-header">
          <h2 class="section-title">Worktrees</h2>
          <span class="section-count">{{ worktrees.length }}</span>
        </div>
        <table class="wt-table">
          <thead>
            <tr>
              <th class="col-name">Worktree</th>
              <th class="col-ns">Namespace</th>
              <th class="col-n">Running</th>
              <th class="col-n">Stopped</th>
              <th class="col-n">Daemons</th>
              <th v-if="showDisk" class="col-n">Disk</th>
              <th class="col-since">Last activity</th>
            </tr>
          </thead>
          <tbody>
            <tr v-for="w in worktrees" :key="w.name" class="row">
              <td class="cell-name">
                <router-link class="wt-link" :to="`/projects/${encodeURIComponent(project)}/${encodeURIComponent(w.name)}`">
                  {{ w.name }}
                </router-link>
                <span v-if="w.is_primary" class="tag">primary</span>
                <div class="cell-dir">{{ w.path }}</div>
              </td>
              <td class="cell-ns">{{ w.namespace }}</td>
              <td class="cell-n">{{ w.daemons.running }}</td>
              <td class="cell-n">{{ w.daemons.stopped }}</td>
              <td class="cell-n">{{ w.daemons.total }}</td>
              <td v-if="showDisk" class="cell-n">{{ formatBytes(w.disk_usage_bytes ?? 0) }}</td>
              <td class="cell-since">{{ formatSince(w.last_activity) }}</td>
            </tr>
          </tbody>
        </table>
      </section>

      <section v-if="data.stack" class="section">
        <div class="section-header">
          <h2 class="section-title">Stack &middot; primary checkout</h2>
          <span class="section-count">{{ data.stack.branch }}</span>
        </div>
        <p class="section-note">
          Starting is always a click: opening this page does not start anything.
        </p>
        <StackGroups :stack="data.stack" :prefers-card="prefersCard" @refresh="refresh" />
      </section>
    </template>
  </div>
</template>

<style scoped lang="less">
@import '@/styles/mixins.less';

.project { width: 100%; max-width: @max-content; margin: 0 auto; }

.page-header { .flex-between(); margin-bottom: @space-xl; gap: @space-xl; padding-bottom: 0.6rem; border-bottom: 1px solid rgba(255, 255, 255, 0.04); }
.page-title { margin: 0; font-size: 1.4rem; font-weight: 700; color: @c-white; letter-spacing: -0.02em; }
.page-subtitle { font-size: 0.8rem; color: @sf-30; }
.crumbs { .font-sans(0.75rem; @sf-30; 500); display: flex; gap: 0.3rem; }
.crumb { color: @sf-40; text-decoration: none; &:hover { color: @c-accent-dim; } }
.crumb-sep { color: @sf-15; }
.btn-ghost { .ghost-btn(); }
.alert { .alert-error(); }

.section { margin-bottom: @space-3xl; }
.section-header { .flex-center(); gap: 0.5rem; margin-bottom: 0.5rem; }
.section-title { margin: 0; .font-sans(1rem; @sf-65; 600); }
.section-count { .font-sans(0.72rem; @sf-40; 500); background: @sf-5; padding: 0.12rem 0.45rem; border-radius: @r-sm; }
.section-note { margin: 0 0 0.6rem 0; .font-sans(0.75rem; @sf-30; 400); }

.wt-table { .table-base(); }
.col-name { width: 34%; text-align: left; }
.col-ns { width: 16%; text-align: left; }
.col-n { width: 10%; text-align: center; }
.col-since { width: 14%; text-align: right; }

.row { border-bottom: 1px solid rgba(255, 255, 255, 0.03); &:last-child { border-bottom: none; } }
.cell-name { padding: 0.55rem 0.75rem; }
.wt-link { .font-sans(0.9rem; @c-white; 600); text-decoration: none; &:hover { color: @c-accent-dim; } }
.tag { margin-left: 0.4rem; .font-sans(0.62rem; @c-accent-dim; 600); background: @sf-accent-10; padding: 0.08rem 0.35rem; border-radius: @r-sm; text-transform: uppercase; }
.cell-dir { .font-mono(0.72rem; @sf-30); }
.cell-ns { .font-mono(0.78rem; @sf-45); }
.cell-n { text-align: center; .font-sans(0.82rem; @sf-45; 500); font-variant-numeric: tabular-nums; }
.cell-since { text-align: right; padding-right: 0.75rem; .font-sans(0.78rem; @sf-30; 500); }
</style>
