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

function openProxy(e: Event) {
  e.stopPropagation()
  if (props.daemon.proxy_url) {
    window.open(props.daemon.proxy_url, '_blank', 'noopener,noreferrer')
  }
}
</script>

<template>
  <tr class="row" @click="goDetail">
    <td class="cell-name">
      <div class="name-main">{{ daemon.id.name }}</div>
      <div class="name-ns">{{ daemon.id.namespace }}</div>
    </td>
    <td class="cell-status">
      <span class="badge" :class="statusClass(daemon.status)">
        <template v-if="daemon.status.type === 'running' && (daemon.active_port != null || daemon.resolved_port.length)">{{ statusText(daemon.status) }}&nbsp;·&nbsp;{{ daemon.active_port ?? daemon.resolved_port[0] }}</template>
        <template v-else>{{ statusText(daemon.status) }}</template>
      </span>
    </td>
    <td class="cell-uptime">
      <span>{{ formatUptime(daemon.uptime_secs) }}</span>
    </td>
    <td class="cell-n">
      <span v-if="daemon.status.type === 'running' && daemon.cpu_percent != null" class="val-cpu">{{ daemon.cpu_percent.toFixed(1) }}%</span>
      <span v-else class="dim">—</span>
    </td>
    <td class="cell-n">
      <span v-if="daemon.status.type === 'running' && daemon.memory_bytes != null" class="val-mem">{{ formatBytes(daemon.memory_bytes) }}</span>
      <span v-else class="dim">—</span>
    </td>
    <td class="cell-actions" @click.stop>
      <button
        v-if="daemon.status.type === 'stopped' || daemon.status.type === 'failed' || daemon.status.type === 'errored' || daemon.status.type === 'available'"
        class="act-btn act-start" :disabled="isActing()" @click="onStart"
      >
        <svg class="icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round"><polygon points="5 3 19 12 5 21 5 3"/></svg>
        Start
      </button>
      <button v-else-if="daemon.status.type === 'running' || daemon.status.type === 'waiting'" class="act-btn act-stop" :disabled="isActing()" @click="onStop">
        <svg class="icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round"><rect x="6" y="6" width="12" height="12"/></svg>
        Stop
      </button>
      <button v-else class="act-btn act-stop" disabled>
        <svg class="icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round"><rect x="6" y="6" width="12" height="12"/></svg>
        Stop
      </button>
      <button
        class="act-btn act-restart"
        v-if="daemon.status.type !== 'available'"
        :disabled="isActing()"
        @click="onRestart"
      >
        <svg class="icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><polyline points="1 4 1 10 7 10"/><path d="M3.51 15a9 9 0 1 0 2.13-9.36L1 10"/></svg>
        Restart
      </button>
      <button v-if="daemon.proxy_url" class="act-btn act-open" @click.stop="openProxy">
        <svg class="icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M18 13v6a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V8a2 2 0 0 1 2-2h6"/><polyline points="15 3 21 3 21 9"/><line x1="10" y1="14" x2="21" y2="3"/></svg>
        Open
      </button>
      <button class="act-btn act-logs" @click="goLogs">
        <svg class="icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z"/><polyline points="14 2 14 8 20 8"/><line x1="16" y1="13" x2="8" y2="13"/><line x1="16" y1="17" x2="8" y2="17"/></svg>
        Logs
      </button>
    </td>
  </tr>
</template>

<style scoped lang="less">
@import '@/styles/mixins.less';

.row {
  cursor: pointer;
  transition: background 0.15s ease;
  border-bottom: 1px solid rgba(255, 255, 255, 0.03);

  &:hover { background: @sf-5; }
  &:last-child { border-bottom: none; }
}

.cell-name    { .table-cell(); width: 26%; }
.cell-status  { .table-cell(); text-align: center; width: 11%; }
.cell-uptime  { .table-cell(); text-align: center; width: 10%; }
.cell-n       { .table-cell(); text-align: center; width: 8%; }
.cell-actions { .table-cell(); text-align: right; width: 32%; white-space: nowrap; }

.name-main {
  .font-mono(0.92rem; @sf-85; 500);
  .truncate();
  letter-spacing: -0.01em;
}
.name-ns { .font-mono(0.72rem; @sf-32); }

.badge {
  .badge-md();

  &.running { .status-running(); }
  &.stopped { .status-stopped(); }
  &.waiting, &.stopping { .status-waiting(); }
  &.failed, &.errored { .status-failed(); }
  &.available { .status-available(); }
}

.cell-uptime span { .font-mono(0.85rem; @sf-75); }

.dim { .font-mono(0.82rem; @sf-15); }
.cell-n .val-cpu { .font-mono(0.85rem; @c-cpu); }
.cell-n .val-mem { .font-mono(0.85rem; @c-mem); }
.cell-n span { .font-mono(0.85rem; @sf-75); }

.icon { width: 14px; height: 14px; flex-shrink: 0; }

.act-btn {
  .btn-base();

  & + & { margin-left: 0.5rem; }
}

.act-start   { .btn-accent(); }
.act-stop    { .btn-neutral(); }
.act-restart { .btn-neutral(); }
.act-logs    { .btn-neutral(); }
.act-open    { .btn-neutral(); }
</style>
