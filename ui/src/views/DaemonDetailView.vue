<script setup lang="ts">
import { computed } from 'vue'
import { useDaemon, useDaemonActions, useProcessTree } from '@/composables/useApi'
import { useRouter } from 'vue-router'
import { formatBytes, formatUptime } from '@/utils/format'
import ProcessTreeNode from '@/components/ProcessTreeNode.vue'

const props = defineProps<{ id: string }>()
const router = useRouter()
const daemonId = computed(() => props.id)
const { daemon, loading, error, refresh } = useDaemon(daemonId)
const { start, stop, restart, enable, disable, acting } = useDaemonActions()
const { tree: processTree, loading: treeLoading } = useProcessTree(daemonId)

const isRunning = computed(() => daemon.value?.status.type === 'running')
const isActing = computed(() => daemon.value ? acting.value.has(daemon.value.id.qualified) : false)

function goLogs() {
  router.push(`/logs/${encodeURIComponent(props.id)}`)
}
function goBack() {
  router.push('/')
}

function statusMeta(s: { type: string }) {
  switch (s.type) {
    case 'running': return { label: 'Running', cls: 'running' }
    case 'available': return { label: 'Available', cls: 'available' }
    case 'stopped': return { label: 'Stopped', cls: 'stopped' }
    case 'failed': return { label: 'Failed', cls: 'failed' }
    case 'errored': return { label: 'Errored', cls: 'errored' }
    case 'waiting': return { label: 'Waiting', cls: 'waiting' }
    case 'stopping': return { label: 'Stopping', cls: 'stopping' }
    default: return { label: s.type, cls: 'stopped' }
  }
}

async function onStart() {
  if (!daemon.value) return
  await start(daemon.value.id.qualified)
  refresh()
}
async function onStop() {
  if (!daemon.value) return
  await stop(daemon.value.id.qualified)
  refresh()
}
async function onRestart() {
  if (!daemon.value) return
  await restart(daemon.value.id.qualified)
  refresh()
}
async function onToggle() {
  if (!daemon.value) return
  if (daemon.value.is_disabled) {
    await enable(daemon.value.id.qualified)
  } else {
    await disable(daemon.value.id.qualified)
  }
  refresh()
}
</script>

<template>
  <div class="detail">
    <button class="back-link" @click="goBack">
      <svg class="back-icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><line x1="19" y1="12" x2="5" y2="12"/><polyline points="12 19 5 12 12 5"/></svg>
      All Daemons
    </button>

    <div v-if="loading" class="loading">
      <div class="spinner" />
      <span>Loading daemon...</span>
    </div>

    <div v-else-if="error" class="alert alert-error">
      <span class="alert-icon">!</span> {{ error }}
    </div>

    <div v-else-if="daemon" class="content">
      <div class="detail-header">
        <div class="identity">
          <div class="status-badge" :class="statusMeta(daemon.status).cls">
            {{ statusMeta(daemon.status).label }}
          </div>
          <h1 class="daemon-title">{{ daemon.id.name }}</h1>
          <div class="daemon-ns">{{ daemon.id.namespace }}</div>
        </div>
        <div class="detail-actions">
          <button
            v-if="daemon.status.type === 'stopped' || daemon.status.type === 'failed'
              || daemon.status.type === 'errored' || daemon.status.type === 'available'"
            class="act-btn act-start"
            :disabled="isActing"
            @click="onStart"
          >
            <svg class="icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round"><polygon points="5 3 19 12 5 21 5 3"/></svg>
            Start
          </button>
          <button
            v-else
            class="act-btn act-stop"
            :disabled="isActing"
            @click="onStop"
          >
            <svg class="icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round"><rect x="6" y="6" width="12" height="12"/></svg>
            Stop
          </button>
          <button class="act-btn act-restart" :disabled="isActing" @click="onRestart">
            <svg class="icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><polyline points="1 4 1 10 7 10"/><path d="M3.51 15a9 9 0 1 0 2.13-9.36L1 10"/></svg>
            Restart
          </button>
          <button class="act-btn act-logs" @click="goLogs">
            <svg class="icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z"/><polyline points="14 2 14 8 20 8"/><line x1="16" y1="13" x2="8" y2="13"/><line x1="16" y1="17" x2="8" y2="17"/></svg>
            Logs
          </button>
          <button
            class="act-btn"
            :class="daemon.is_disabled ? 'act-start' : 'act-muted'"
            @click="onToggle"
          >
            {{ daemon.is_disabled ? 'Enable' : 'Disable' }}
          </button>
        </div>
      </div>

      <div class="detail-grid">
        <div class="info-card">
          <div class="info-label">PID</div>
          <div class="info-value">{{ daemon.pid ?? '—' }}</div>
        </div>
        <div class="info-card">
          <div class="info-label">Shell PID</div>
          <div class="info-value">{{ daemon.shell_pid ?? '—' }}</div>
        </div>
        <div class="info-card">
          <div class="info-label">Active Port</div>
          <div class="info-value">{{ daemon.active_port ?? '—' }}</div>
        </div>
        <div class="info-card">
          <div class="info-label">Resolved Ports</div>
          <div class="info-value">{{ daemon.resolved_port.join(', ') || '—' }}</div>
        </div>
        <div class="info-card">
          <div class="info-label">Slug</div>
          <div class="info-value">{{ daemon.slug ?? '—' }}</div>
        </div>
        <div class="info-card">
          <div class="info-label">Autostop</div>
          <div class="info-value">{{ daemon.autostop ? 'Yes' : 'No' }}</div>
        </div>
        <div class="info-card">
          <div class="info-label">Retries</div>
          <div class="info-value">{{ daemon.retry_count }}</div>
        </div>
        <div class="info-card">
          <div class="info-label">Disabled</div>
          <div class="info-value" :class="daemon.is_disabled ? 'text-danger' : 'text-muted'">
            {{ daemon.is_disabled ? 'Yes' : 'No' }}
          </div>
        </div>
        <div class="info-card">
          <div class="info-label">CPU</div>
          <div class="info-value">{{ isRunning && daemon.cpu_percent != null ? daemon.cpu_percent.toFixed(1) + '%' : '—' }}</div>
        </div>
        <div class="info-card">
          <div class="info-label">Memory</div>
          <div class="info-value">{{ isRunning && daemon.memory_bytes != null ? formatBytes(daemon.memory_bytes) : '—' }}</div>
        </div>
        <div class="info-card">
          <div class="info-label">Uptime</div>
          <div class="info-value">{{ isRunning && daemon.uptime_secs != null ? formatUptime(daemon.uptime_secs) : '—' }}</div>
        </div>
        <div class="info-card">
          <div class="info-label">Proxy URL</div>
          <div class="info-value" style="font-size: 0.78rem;">{{ daemon.proxy_url ?? '—' }}</div>
        </div>
      </div>

      <!-- Extended Configuration -->
      <div class="section-title" style="margin-top: 1.5rem;">Configuration</div>
      <div class="detail-grid">
        <div class="info-card" v-if="daemon.ready_delay">
          <div class="info-label">Ready Delay</div>
          <div class="info-value">{{ daemon.ready_delay }}s</div>
        </div>
        <div class="info-card" v-if="daemon.ready_output">
          <div class="info-label">Ready Output</div>
          <div class="info-value">{{ daemon.ready_output }}</div>
        </div>
        <div class="info-card" v-if="daemon.ready_http_url">
          <div class="info-label">Ready HTTP</div>
          <div class="info-value">{{ daemon.ready_http_url }}</div>
        </div>
        <div class="info-card" v-if="daemon.ready_port">
          <div class="info-label">Ready Port</div>
          <div class="info-value">{{ daemon.ready_port }}</div>
        </div>
        <div class="info-card" v-if="daemon.ready_cmd">
          <div class="info-label">Ready Cmd</div>
          <div class="info-value">{{ daemon.ready_cmd }}</div>
        </div>
        <div class="info-card" v-if="daemon.port_config">
          <div class="info-label">Port Config</div>
          <div class="info-value">{{ daemon.port_config }}</div>
        </div>
        <div class="info-card" v-if="daemon.mise != null">
          <div class="info-label">Mise</div>
          <div class="info-value">{{ daemon.mise ? 'Yes' : 'No' }}</div>
        </div>
        <div class="info-card" v-if="daemon.user">
          <div class="info-label">User</div>
          <div class="info-value">{{ daemon.user }}</div>
        </div>
        <div class="info-card" v-if="daemon.memory_limit">
          <div class="info-label">Memory Limit</div>
          <div class="info-value">{{ daemon.memory_limit }}</div>
        </div>
        <div class="info-card" v-if="daemon.cpu_limit">
          <div class="info-label">CPU Limit</div>
          <div class="info-value">{{ daemon.cpu_limit }}</div>
        </div>
        <div class="info-card" v-if="daemon.stop_signal">
          <div class="info-label">Stop Signal</div>
          <div class="info-value">{{ daemon.stop_signal }}</div>
        </div>
        <div class="info-card" v-if="daemon.stop_timeout">
          <div class="info-label">Stop Timeout</div>
          <div class="info-value">{{ daemon.stop_timeout }}</div>
        </div>
        <div class="info-card" v-if="daemon.pty != null">
          <div class="info-label">PTY</div>
          <div class="info-value">{{ daemon.pty ? 'Yes' : 'No' }}</div>
        </div>
        <div class="info-card" v-if="daemon.proxy != null">
          <div class="info-label">Proxy</div>
          <div class="info-value">{{ daemon.proxy ? 'Yes' : 'No' }}</div>
        </div>
        <div class="info-card" v-if="daemon.watch.length">
          <div class="info-label">Watch Mode</div>
          <div class="info-value">{{ daemon.watch_mode }}</div>
        </div>
      </div>

      <!-- Depends -->
      <div v-if="daemon.depends.length" class="section">
        <div class="section-title">Depends</div>
        <div class="section-body">
          <span v-for="dep in daemon.depends" :key="dep" class="tag">{{ dep }}</span>
        </div>
      </div>

      <!-- Watch -->
      <div v-if="daemon.watch.length" class="section">
        <div class="section-title">Watch</div>
        <div class="section-body">
          <span v-for="w in daemon.watch" :key="w" class="tag">{{ w }}</span>
        </div>
      </div>

      <!-- Env -->
      <div v-if="daemon.env" class="section">
        <div class="section-title">Environment</div>
        <div class="section-body">
          <div v-for="(val, key) in daemon.env" :key="key" class="env-row">
            <code class="env-key">{{ key }}</code>
            <span class="env-sep">=</span>
            <code class="env-val">{{ val }}</code>
          </div>
        </div>
      </div>

      <!-- Cron Schedule -->
      <div v-if="daemon.cron_schedule" class="section">
        <div class="section-title">Cron Schedule</div>
        <div class="section-body">
          <code>{{ daemon.cron_schedule }}</code>
        </div>
      </div>

      <!-- Command -->
      <div v-if="daemon.command" class="section">
        <div class="section-title">Command</div>
        <div class="section-body">
          <code class="block">{{ daemon.command }}</code>
        </div>
      </div>

      <!-- Directory -->
      <div v-if="daemon.dir" class="section">
        <div class="section-title">Directory</div>
        <div class="section-body">
          <code class="block">{{ daemon.dir }}</code>
        </div>
      </div>

      <!-- Process Tree -->
      <div class="section">
        <div class="section-title">Process Tree</div>
        <div v-if="treeLoading && !processTree.length" class="section-body text-muted">
          <div class="loading-inline"><div class="spinner-sm" /> Loading process tree...</div>
        </div>
        <div v-else-if="!processTree.length" class="section-body text-muted">
          No running processes
        </div>
        <div v-else class="process-tree">
          <ProcessTreeNode
            v-for="node in processTree"
            :key="node.pid"
            :node="node"
            :depth="0"
          />
        </div>
      </div>
    </div>
  </div>
</template>

<style scoped lang="less">
@import '@/styles/mixins.less';

.detail { max-width: 900px; margin: 0 auto; }

.back-link {
  display: inline-flex;
  align-items: center;
  gap: 0.4rem;
  background: none;
  border: none;
  color: @sf-35;
  font-size: 0.85rem;
  cursor: pointer;
  padding: 0;
  margin-bottom: 1.25rem;
  transition: color 0.15s;

  &:hover { color: @sf-70; }
}

.back-icon { width: 14px; height: 14px; }

.loading { .flex-center(); gap: 0.75rem; padding: 3rem; justify-content: center; color: @sf-40; font-size: 0.9rem; }
.loading-inline { .flex-center(); gap: 0.5rem; font-size: 0.85rem; }

.spinner { width: 18px; height: 18px; border: 2px solid rgba(255, 255, 255, 0.08); border-top-color: rgba(255, 255, 255, 0.4); border-radius: 50%; animation: spin 0.8s linear infinite; }
.spinner-sm { width: 12px; height: 12px; border: 2px solid rgba(255, 255, 255, 0.08); border-top-color: rgba(255, 255, 255, 0.4); border-radius: 50%; animation: spin 0.8s linear infinite; }

.alert { .alert-base(); background: rgba(220, 38, 38, 0.08); border: 1px solid rgba(220, 38, 38, 0.15); color: @c-accent; }
.alert-icon { font-weight: 700; flex-shrink: 0; }

.detail-header { .flex-between(); align-items: flex-start; gap: @space-xl; margin-bottom: 1.5rem; flex-wrap: wrap; }
.identity { min-width: 0; }

.status-badge {
  display: inline-block;
  font-size: 0.72rem;
  font-weight: 600;
  padding: 0.2rem 0.6rem;
  border-radius: @r-md;
  margin-bottom: 0.5rem;
  text-transform: capitalize;

  &.running   { background: @sf-success-10; color: @c-success; }
  &.available { background: @sf-4; color: @sf-35; }
  &.stopped   { background: @sf-4; color: @sf-30; }
  &.failed    { background: @sf-danger-8; color: @c-danger; }
  &.errored   { background: @sf-danger-8; color: @c-danger; }
  &.waiting   { background: @sf-warning-8; color: @c-warning; }
  &.stopping  { background: @sf-warning-8; color: @c-warning; }
}

.daemon-title { margin: 0; font-size: 1.5rem; font-weight: 700; color: @c-white; letter-spacing: -0.02em; }
.daemon-ns { font-size: 0.85rem; color: @sf-30; margin-top: 0.15rem; }

.detail-actions { display: flex; gap: 0.4rem; flex-wrap: wrap; }
.icon { width: 14px; height: 14px; flex-shrink: 0; }

.act-btn {
  padding: 0.4rem 0.75rem;
  border-radius: @r-md;
  font-size: 0.78rem;
  font-weight: 600;
  cursor: pointer;
  transition: @tr-base;
  border: none;
  display: inline-flex;
  align-items: center;
  gap: 0.3rem;
  line-height: 1;

  &:disabled { opacity: 0.3; cursor: not-allowed; }
}

.act-start   { background: rgba(48, 164, 108, 0.12); color: @c-success; &:hover:not(:disabled) { background: rgba(48, 164, 108, 0.22); } }
.act-stop    { background: rgba(220, 38, 38, 0.12); color: @c-accent; &:hover:not(:disabled) { background: rgba(220, 38, 38, 0.22); } }
.act-restart { background: rgba(255, 255, 255, 0.05); color: @sf-45; &:hover:not(:disabled) { background: rgba(255, 255, 255, 0.1); color: @sf-75; } }
.act-logs    { background: rgba(255, 255, 255, 0.05); color: @sf-45; &:hover:not(:disabled) { background: rgba(255, 255, 255, 0.1); color: @sf-75; } }
.act-muted   { background: rgba(255, 255, 255, 0.05); color: @sf-35; &:hover:not(:disabled) { background: rgba(255, 255, 255, 0.1); color: @sf-75; } }

.detail-grid {
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(180px, 1fr));
  gap: 0.75rem;
  margin-bottom: 0.5rem;
}

.info-card {
  background: @sf-2;
  border: 1px solid rgba(255, 255, 255, 0.05);
  border-radius: @r-xl;
  padding: 0.9rem @space-xl;
  transition: background 0.15s;

  &:hover { background: @sf-4; }
}

.info-label { .label-micro(); margin-bottom: 0.35rem; }
.info-value { font-size: 0.92rem; font-weight: 600; color: @sf-80; }

.text-danger { color: @c-danger; }
.text-muted  { color: @sf-30; }

.section { margin-bottom: 1.25rem; }
.section-title { .label-micro(); margin-bottom: 0.5rem; }

.section-body {
  background: rgba(255, 255, 255, 0.015);
  border: 1px solid rgba(255, 255, 255, 0.05);
  border-radius: @r-xl;
  padding: 0.9rem @space-xl;
  font-size: 0.85rem;
  color: @sf-55;
}

.section-body code {
  .font-mono(0.8rem; @sf-60);
  background: none;
}
.section-body code.block { display: block; white-space: pre-wrap; word-break: break-all; }

.tag {
  display: inline-block;
  background: @sf-4;
  border: 1px solid rgba(255, 255, 255, 0.06);
  border-radius: @r-sm;
  padding: 0.2rem 0.5rem;
  margin: 0.15rem;
  font-size: 0.78rem;
  color: @sf-55;
}

.env-row { display: flex; align-items: center; gap: 0.3rem; padding: 0.15rem 0; font-size: 0.78rem; }
.env-key { color: @sf-50; font-weight: 600; }
.env-sep { color: @sf-20; }
.env-val { color: @sf-35; }

.process-tree {
  background: rgba(0, 0, 0, 0.2);
  border: 1px solid rgba(255, 255, 255, 0.05);
  border-radius: @r-xl;
  padding: 0.5rem 0.75rem;
  .font-mono(0.78rem; @sf-65);
}

.mobile-sm({
  .detail-header { flex-direction: column; }
  .detail-actions { width: 100%; }
  .detail-actions .act-btn { flex: 1; justify-content: center; }
  .detail-grid { grid-template-columns: repeat(2, 1fr); gap: 0.5rem; }
  .info-card { padding: 0.6rem 0.75rem; }
  .info-value { font-size: 0.85rem; }
});
</style>
