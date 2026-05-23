<script setup lang="ts">
import { useDaemonActions } from '@/composables/useApi'
import { useRouter } from 'vue-router'
import { formatBytes, formatUptime } from '@/utils/format'
import type { DaemonEntry } from '@/types/api'

const props = defineProps<{ daemon: DaemonEntry }>()
const emit = defineEmits<{ refresh: [] }>()
const router = useRouter()
const { start, stop, restart, acting } = useDaemonActions()

function statusClass(s: DaemonEntry['status']): string { return s.type }

function statusText(s: DaemonEntry['status']): string {
  switch (s.type) {
    case 'failed': return `failed: ${s.message}`
    case 'errored': return `ERRORED · ${s.code}`
    default: return s.type
  }
}

function goDetail() { router.push(`/daemon/${encodeURIComponent(props.daemon.id.qualified)}`) }
function goLogs() { router.push(`/logs/${encodeURIComponent(props.daemon.id.qualified)}`) }

async function onStart(e: Event) { e.stopPropagation(); await start(props.daemon.id.qualified); emit('refresh') }
async function onStop(e: Event) { e.stopPropagation(); await stop(props.daemon.id.qualified); emit('refresh') }
async function onRestart(e: Event) { e.stopPropagation(); await restart(props.daemon.id.qualified); emit('refresh') }

const isActing = () => acting.value.has(props.daemon.id.qualified)
</script>

<template>
  <div class="card" @click="goDetail">
    <div class="card-header">
      <div class="daemon-name">{{ daemon.id.name }}</div>
      <span class="badge" :class="statusClass(daemon.status)">
        <template v-if="daemon.status.type === 'running' && (daemon.active_port != null || daemon.resolved_port.length)">{{ statusText(daemon.status) }}&nbsp;·&nbsp;{{ daemon.active_port ?? daemon.resolved_port[0] }}</template>
        <template v-else>{{ statusText(daemon.status) }}</template>
      </span>
    </div>
    <div class="daemon-id">{{ daemon.id.namespace }}</div>
    <div class="metrics" :class="{ 'not-running': daemon.status.type !== 'running' }">
      <div class="metric">
        <div class="metric-label">UPTIME</div>
        <div class="metric-value">{{ formatUptime(daemon.uptime_secs) }}</div>
      </div>
      <div class="metric">
        <div class="metric-label">CPU</div>
        <div class="metric-value" :class="daemon.status.type === 'running' && daemon.cpu_percent != null ? 'val-cpu' : ''">
          {{ daemon.status.type === 'running' && daemon.cpu_percent != null ? daemon.cpu_percent.toFixed(1) + '%' : '—' }}
        </div>
      </div>
      <div class="metric">
        <div class="metric-label">MEM</div>
        <div class="metric-value" :class="daemon.status.type === 'running' && daemon.memory_bytes != null ? 'val-mem' : ''">
          {{ daemon.status.type === 'running' && daemon.memory_bytes != null ? formatBytes(daemon.memory_bytes) : '—' }}
        </div>
      </div>
    </div>
    <div class="card-actions" @click.stop>
      <button v-if="daemon.status.type === 'stopped' || daemon.status.type === 'failed' || daemon.status.type === 'errored' || daemon.status.type === 'available'" class="act-btn act-start" :disabled="isActing()" @click="onStart">
        <svg class="icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round"><polygon points="5 3 19 12 5 21 5 3"/></svg>
        Start
      </button>
      <button v-else-if="daemon.status.type === 'running' || daemon.status.type === 'waiting'" class="act-btn act-stop" @click="onStop">
        <svg class="icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round"><rect x="6" y="6" width="12" height="12"/></svg>
        Stop
      </button>
      <button v-else class="act-btn act-stop" disabled>
        <svg class="icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round"><rect x="6" y="6" width="12" height="12"/></svg>
        Stop
      </button>
      <button v-if="daemon.status.type !== 'available'" class="act-btn act-restart" @click="onRestart">
        <svg class="icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><polyline points="1 4 1 10 7 10"/><path d="M3.51 15a9 9 0 1 0 2.13-9.36L1 10"/></svg>
        Restart
      </button>
      <button class="act-btn act-logs" @click="goLogs">
        <svg class="icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z"/><polyline points="14 2 14 8 20 8"/><line x1="16" y1="13" x2="8" y2="13"/><line x1="16" y1="17" x2="8" y2="17"/></svg>
        Logs
      </button>
    </div>
  </div>
</template>

<style scoped lang="less">
@import '@/styles/mixins.less';

.card { .card-surface(); }

.card-header { .flex-between(); gap: 0.6rem; margin-bottom: 0.2rem; }

.daemon-name {
  .font-mono(1.1rem; @sf-88; 500);
  .truncate();
  letter-spacing: -0.01em;
}

.daemon-id {
  .font-mono(0.75rem; @sf-35);
  margin-bottom: 0.9rem;
}

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

.metrics {
  display: grid;
  grid-template-columns: repeat(3, 1fr);
  gap: 0.5rem;
  margin-bottom: @space-xl;
}

.metric { text-align: center; }

.metric-label { .label-micro(); margin-bottom: 0.15rem; }
.metric-value { .font-mono(0.92rem; @sf-70; 500); }

.metric-value.val-cpu { color: @c-cpu; }
.metric-value.val-mem { color: @c-mem; }

.card-actions { display: flex; gap: 0.5rem; }

.icon { width: 14px; height: 14px; flex-shrink: 0; }

.act-btn {
  .btn-base();
  flex: 1;
  padding: 0.5rem 0.3rem;
}

.act-start   { .btn-accent(); }
.act-stop    { .btn-neutral(); }
.act-restart { .btn-neutral(); }
.act-logs    { .btn-neutral(); }

.mobile({
  .metrics.not-running { display: none; }
  .daemon-name { font-size: 1rem; }
  .daemon-id { font-size: 0.72rem; }
  .badge { font-size: 0.65rem; padding: 0.15rem 0.5rem; }
  .metric-value { font-size: 0.85rem; }
  .act-btn { font-size: 0.75rem; padding: 0.45rem 0; }
});
</style>
