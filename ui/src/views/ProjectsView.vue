<script setup lang="ts">
import { useProjects } from '@/composables/useApi'
import { formatSince } from '@/utils/format'

const { projects, loading, error, refresh } = useProjects()
</script>

<template>
  <div class="projects">
    <div class="page-header">
      <div>
        <h1 class="page-title">Projects</h1>
        <span class="page-subtitle">{{ projects.length }} registered</span>
      </div>
      <button class="btn-ghost" :disabled="loading" @click="refresh">Refresh</button>
    </div>

    <div v-if="error" class="alert alert-error">{{ error }}</div>

    <table v-if="projects.length" class="project-table">
      <thead>
        <tr>
          <th class="col-name">Project</th>
          <th class="col-n">Worktrees</th>
          <th class="col-n">Running</th>
          <th class="col-n">Daemons</th>
          <th class="col-since">Last activity</th>
        </tr>
      </thead>
      <tbody>
        <tr v-for="p in projects" :key="p.name" class="row">
          <td class="cell-name">
            <router-link class="project-link" :to="`/projects/${encodeURIComponent(p.name)}`">
              {{ p.name }}
            </router-link>
            <div class="cell-dir">{{ p.dir }}</div>
          </td>
          <td class="cell-n">{{ p.worktree_count }}</td>
          <td class="cell-n">{{ p.daemons.running }}</td>
          <td class="cell-n">{{ p.daemons.total }}</td>
          <td class="cell-since">{{ formatSince(p.last_activity) }}</td>
        </tr>
      </tbody>
    </table>

    <div v-else-if="!loading" class="empty-state">
      <h3>No projects registered</h3>
      <p>Register one with <code>pitchfork supervisor namespace add &lt;name&gt; &lt;dir&gt;</code>.</p>
    </div>
  </div>
</template>

<style scoped lang="less">
@import '@/styles/mixins.less';

.projects { width: 100%; max-width: @max-content; margin: 0 auto; }

.page-header { .flex-between(); margin-bottom: @space-xl; gap: @space-xl; padding-bottom: 0.6rem; border-bottom: 1px solid rgba(255, 255, 255, 0.04); }
.page-title { margin: 0; font-size: 1.4rem; font-weight: 700; color: @c-white; letter-spacing: -0.02em; }
.page-subtitle { font-size: 0.8rem; color: @sf-30; }
.btn-ghost { .ghost-btn(); }
.alert { .alert-error(); }

.project-table { .table-base(); }

.col-name { width: 45%; text-align: left; }
.col-n { width: 12%; text-align: center; }
.col-since { width: 19%; text-align: right; }

.row { border-bottom: 1px solid rgba(255, 255, 255, 0.03); &:last-child { border-bottom: none; } }

.cell-name { padding: 0.55rem 0.75rem; }
.project-link { .font-sans(0.9rem; @c-white; 600); text-decoration: none; &:hover { color: @c-accent-dim; } }
.cell-dir { .font-mono(0.72rem; @sf-30); }
.cell-n { text-align: center; .font-sans(0.82rem; @sf-45; 500); font-variant-numeric: tabular-nums; }
.cell-since { text-align: right; padding-right: 0.75rem; .font-sans(0.78rem; @sf-30; 500); }

.empty-state { text-align: center; padding: 3rem @space-xl; border: 1px dashed rgba(255, 255, 255, 0.06); border-radius: @r-2xl; background: @sf-1; }
.empty-state h3 { margin: 0 0 0.2rem 0; font-size: 1.05rem; font-weight: 600; color: @sf-45; }
.empty-state p { margin: 0; font-size: 0.85rem; color: @sf-25; }
</style>
