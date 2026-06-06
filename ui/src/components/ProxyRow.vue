<script setup lang="ts">
import type { ProxyWorktreeEntry } from '@/types/api'
import { useRouter } from 'vue-router'
import { useDaemonActions } from '@/composables/useApi'
import { formatUptime } from '@/utils/format'
import { computed } from 'vue'

const props = defineProps<{ proxy: ProxyWorktreeEntry }>()
const emit = defineEmits<{ refresh: [] }>()
const router = useRouter()
const { restart, start, stop, acting } = useDaemonActions()

const daemonKey = computed(() => props.proxy.daemon_qualified ?? '')

async function onRestart() { if (!daemonKey.value) return; await restart(daemonKey.value); emit('refresh') }
async function onStart() { if (!daemonKey.value) return; await start(daemonKey.value); emit('refresh') }
async function onStop() { if (!daemonKey.value) return; await stop(daemonKey.value); emit('refresh') }
function goLogs() { if (!daemonKey.value) return; router.push(`/logs/${encodeURIComponent(daemonKey.value)}`) }

function statusClass(s: string | null): string { return s ?? '' }
const isActing = () => acting.value.has(daemonKey.value)
</script>

<template>
  <tr class="row">
    <td class="cell-branch">
      <div class="branch-main">{{ proxy.branch }}</div>
      <div class="branch-sub">{{ proxy.daemon_qualified }}</div>
    </td>
    <td class="cell-url">
      <a v-if="proxy.proxy_url" :href="proxy.proxy_url" target="_blank" rel="noopener noreferrer" class="url-link" @click.stop>{{ proxy.proxy_url }}</a>
      <span v-else>—</span>
    </td>
    <td class="cell-status">
      <span v-if="proxy.status" class="badge" :class="statusClass(proxy.status)">{{ proxy.status }}</span>
      <span v-else>—</span>
    </td>
    <td class="cell-uptime"><span>{{ formatUptime(proxy.uptime_secs) }}</span></td>
    <td class="cell-n">
      <span v-if="proxy.port" class="port-live">{{ proxy.port }}</span>
      <span v-else>—</span>
    </td>
    <td class="cell-actions">
      <button
        v-if="proxy.status === 'stopped' || proxy.status === 'failed' || proxy.status === 'errored' || proxy.status === 'available' || !proxy.status"
        class="act-btn act-start" :disabled="isActing()" @click.stop="onStart"
      >
        <svg class="act-icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round"><polygon points="5 3 19 12 5 21 5 3"/></svg>
        Start
      </button>
      <button v-else-if="proxy.status === 'running' || proxy.status === 'waiting'" class="act-btn act-stop" title="Stop daemon" :disabled="isActing()" @click.stop="onStop">
        <svg class="act-icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round"><rect x="6" y="6" width="12" height="12"/></svg>
        Stop
      </button>
      <button v-else class="act-btn act-stop" disabled>
        <svg class="act-icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round"><rect x="6" y="6" width="12" height="12"/></svg>
        Stop
      </button>
      <button v-if="proxy.daemon_qualified" class="act-btn act-restart" title="Restart daemon" :disabled="isActing()" @click.stop="onRestart">
        <svg class="act-icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><polyline points="1 4 1 10 7 10"/><path d="M3.51 15a9 9 0 1 0 2.13-9.36L1 10"/></svg>
        Restart
      </button>
      <button class="act-btn act-logs" title="View logs" @click.stop="goLogs">
        <svg class="act-icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z"/><polyline points="14 2 14 8 20 8"/><line x1="16" y1="13" x2="8" y2="13"/><line x1="16" y1="17" x2="8" y2="17"/></svg>
        Logs
      </button>
    </td>
  </tr>
</template>

<style scoped lang="less">
@import '@/styles/mixins.less';

.row {
  transition: background 0.15s ease;
  border-bottom: 1px solid rgba(255, 255, 255, 0.03);

  &:hover { background: @sf-5; }
  &:last-child { border-bottom: none; }
}

.cell-branch    { .table-cell(); width: 18%; }
.cell-url       { .table-cell(); width: 20%; }
.cell-status    { .table-cell(); text-align: center; width: 11%; }
.cell-uptime    { .table-cell(); text-align: center; width: 10%; }
.cell-n         { .table-cell(); text-align: center; width: 8%; }
.cell-actions   { .table-cell(); text-align: right; width: 33%; white-space: nowrap; }

.branch-main { .font-mono(0.9rem; @sf-85; 500); }
.branch-sub  { .font-mono(0.78rem; @sf-35); margin-top: 0.1rem; }

.url-link {
  .font-mono(0.8rem; @c-link);
  .truncate();
  text-decoration: none;
  display: block;
  max-width: 100%;

  &:hover { text-decoration: underline; }
}

.cell-uptime span { .font-mono(0.85rem; @sf-75); }

.port-live { .font-mono(0.85rem; @c-success; 500); }
.dim       { .font-mono(0.82rem; @sf-15); }

.badge {
  .badge-md();

  &.running { .status-running(); }
  &.stopped { .status-stopped(); }
  &.waiting, &.stopping { .status-waiting(); }
  &.failed, &.errored { .status-failed(); }
  &.available { .status-available(); }
}

.act-btn {
  .btn-base();

  & + & { margin-left: 0.5rem; }
}

.act-start   { .btn-accent(); }
.act-stop    { .btn-neutral(); }
.act-restart { .btn-neutral(); }
.act-logs    { .btn-neutral(); }

.act-icon { width: 14px; height: 14px; flex-shrink: 0; }
</style>
