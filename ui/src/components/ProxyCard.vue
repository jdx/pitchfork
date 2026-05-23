<script setup lang="ts">
import type { ProxyWorktreeEntry } from '@/types/api'
import { useRouter } from 'vue-router'
import { useDaemonActions } from '@/composables/useApi'
import { formatUptime } from '@/utils/format'

const props = defineProps<{ proxy: ProxyWorktreeEntry }>()
const emit = defineEmits<{ refresh: [] }>()
const router = useRouter()
const { restart, start, stop, acting } = useDaemonActions()

const daemonKey = props.proxy.daemon_qualified ?? ''

async function onRestart() { if (!daemonKey) return; await restart(daemonKey); emit('refresh') }
async function onStart() { if (!daemonKey) return; await start(daemonKey); emit('refresh') }
async function onStop() { if (!daemonKey) return; await stop(daemonKey); emit('refresh') }
function goLogs() { if (!daemonKey) return; router.push(`/logs/${encodeURIComponent(daemonKey)}`) }

function statusClass(s: string | null): string { return s ?? '' }
const isActing = () => acting.value.has(daemonKey)
</script>

<template>
  <div class="card">
    <div class="card-header">
      <div>
        <div class="branch-name">{{ proxy.branch }}</div>
        <div class="daemon-sub">{{ proxy.daemon_qualified }}</div>
      </div>
      <span v-if="proxy.status" class="badge" :class="statusClass(proxy.status)">
        {{ proxy.status }}
      </span>
    </div>

    <div class="card-meta">
      <div class="meta-item">
        <span class="meta-label">URL</span>
        <a v-if="proxy.proxy_url" :href="proxy.proxy_url" target="_blank" class="meta-value url" @click.stop>
          {{ proxy.proxy_url }}
        </a>
        <span v-else class="meta-value dim">—</span>
      </div>
      <div class="meta-item">
        <span class="meta-label">Port</span>
        <span class="meta-value" :class="proxy.port ? 'port-live' : 'dim'">
          {{ proxy.port ?? '—' }}
        </span>
      </div>
      <div class="meta-item">
        <span class="meta-label">Uptime</span>
        <span class="meta-value" :class="{ dim: proxy.uptime_secs == null }">{{ formatUptime(proxy.uptime_secs) }}</span>
      </div>
    </div>

    <div class="card-actions">
      <button
        v-if="proxy.status === 'stopped' || proxy.status === 'failed' || proxy.status === 'errored' || proxy.status === 'available' || !proxy.status"
        class="act-btn act-start" :disabled="isActing()" @click.stop="onStart"
      >
        <svg class="act-icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round"><polygon points="5 3 19 12 5 21 5 3"/></svg>
        Start
      </button>
      <button v-else-if="proxy.status === 'running' || proxy.status === 'waiting'" class="act-btn act-stop" :disabled="isActing()" @click.stop="onStop">
        <svg class="act-icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round"><rect x="6" y="6" width="12" height="12"/></svg>
        Stop
      </button>
      <button v-else class="act-btn act-stop" disabled>
        <svg class="act-icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round"><rect x="6" y="6" width="12" height="12"/></svg>
        Stop
      </button>
      <button v-if="proxy.daemon_qualified" class="act-btn act-restart" :disabled="isActing()" @click.stop="onRestart">
        <svg class="act-icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><polyline points="1 4 1 10 7 10"/><path d="M3.51 15a9 9 0 1 0 2.13-9.36L1 10"/></svg>
        Restart
      </button>
      <button class="act-btn act-logs" @click.stop="goLogs">
        <svg class="act-icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z"/><polyline points="14 2 14 8 20 8"/><line x1="16" y1="13" x2="8" y2="13"/><line x1="16" y1="17" x2="8" y2="17"/></svg>
        Logs
      </button>
    </div>
  </div>
</template>

<style scoped lang="less">
@import '@/styles/mixins.less';

.card { .card-surface(); }

.card-header { .flex-between(); gap: 0.6rem; margin-bottom: 0.5rem; }

.branch-name { .font-mono(0.95rem; @sf-85; 500); }
.daemon-sub  { .font-mono(0.78rem; @sf-35); margin-top: 0.15rem; }

.badge {
  .badge-sm();
  border: 1px solid;

  &.running   { background: @sf-success-12; color: @c-success; border-color: @sf-success-20; }
  &.stopped   { background: @sf-3; color: @sf-30; border-color: @sf-8; }
  &.waiting,
  &.stopping  { background: @sf-warning-8; color: @c-warning; border-color: @sf-warning-15; }
  &.failed,
  &.errored   { background: @sf-danger-8; color: @c-danger; border-color: @sf-danger-15; }
  &.available { background: @sf-info-8; color: @c-info; border-color: @sf-info-15; }
}

.card-meta {
  display: flex;
  flex-direction: column;
  gap: 0.35rem;
  margin-bottom: 0.75rem;
}

.meta-item { display: flex; align-items: baseline; gap: 0.5rem; }
.meta-label { .label-micro(); min-width: 3rem; flex-shrink: 0; }

.meta-value {
  .font-mono(0.8rem; @sf-60);
  word-break: break-all;

  &.url { color: @c-link; text-decoration: none; }
  &.url:hover { text-decoration: underline; }
  &.port-live { color: @c-success; font-weight: 500; }
  &.dim { color: @sf-15; }
}

.card-actions { display: flex; gap: 0.4rem; border-top: 1px solid rgba(255, 255, 255, 0.04); padding-top: 0.6rem; }

.act-btn {
  .btn-base();
  flex: 1;
  padding: 0.5rem 0.3rem;
  font-weight: 600;

  & + & { margin-left: 0.5rem; }
}

.act-start   { .btn-accent(); }
.act-stop    { .btn-neutral(); }
.act-restart { .btn-neutral(); }
.act-logs    { .btn-neutral(); }

.act-icon { width: 14px; height: 14px; flex-shrink: 0; }
</style>
