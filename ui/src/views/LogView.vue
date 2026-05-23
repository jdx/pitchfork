<script setup lang="ts">
import { computed, ref, watch, nextTick } from 'vue'
import { useLogStream } from '@/composables/useApi'
import { useRouter } from 'vue-router'
import { parseLogLines, type ParsedLogLine } from '@/utils/log'

const props = defineProps<{ id: string }>()
const router = useRouter()
const { lines, error, connected } = useLogStream(computed(() => decodeURIComponent(props.id)))
const logContainer = ref<HTMLDivElement | null>(null)
const autoScroll = ref(true)
const showTimestamps = ref(false)

const decodedId = computed(() => decodeURIComponent(props.id))
const parsedLines = computed<ParsedLogLine[]>(() => parseLogLines(lines.value))

function goBack() {
  router.push(`/daemon/${encodeURIComponent(props.id)}`)
}

watch(lines, () => {
  if (autoScroll.value) {
    nextTick(() => {
      if (logContainer.value) {
        logContainer.value.scrollTop = logContainer.value.scrollHeight
      }
    })
  }
}, { deep: true })

function onScroll() {
  if (!logContainer.value) return
  const { scrollTop, scrollHeight, clientHeight } = logContainer.value
  autoScroll.value = scrollHeight - scrollTop - clientHeight < 20
}
</script>

<template>
  <div class="log-view">
    <div class="log-header">
      <div class="log-title">
        <button class="back-link" @click="goBack">
          <span class="back-arrow">←</span>
        </button>
        <div class="title-text">
          <h1>Logs</h1>
          <span class="daemon-name">{{ decodedId }}</span>
        </div>
      </div>
      <div class="log-controls">
        <label class="toggle">
          <input v-model="autoScroll" type="checkbox" />
          <span class="toggle-label">Auto-scroll</span>
        </label>
        <label class="toggle">
          <input v-model="showTimestamps" type="checkbox" />
          <span class="toggle-label">Timestamps</span>
        </label>
        <div class="line-count">{{ lines.length.toLocaleString() }} lines</div>
      </div>
    </div>

    <div v-if="error" class="alert alert-error">
      <span class="alert-icon">!</span> {{ error }}
    </div>

    <div ref="logContainer" class="log-container" @scroll="onScroll">
      <div
        v-for="(line, i) in parsedLines"
        :key="i"
        class="log-line"
      >
        <span v-if="showTimestamps && line.timestamp" class="line-ts">{{ line.timestamp }}</span>
        <span v-else-if="showTimestamps" class="line-ts">--:--:--</span>
        <span class="line-num">{{ (i + 1).toString().padStart(5, '0') }}</span>
        <span class="line-content" v-html="line.html" />
      </div>
      <div v-if="lines.length === 0" class="empty">
        <div class="empty-icon">◈</div>
        <p>Waiting for log output...</p>
      </div>
      <div v-if="connected && lines.length > 0" class="live-indicator">
        <span class="live-dot" /> streaming
      </div>
    </div>
  </div>
</template>

<style scoped lang="less">
@import '@/styles/mixins.less';

.log-view { .flex-column(); height: calc(100vh - 88px); }

.log-header { .flex-between(); gap: @space-xl; margin-bottom: @space-xl; flex-wrap: wrap; }

.log-title { display: flex; align-items: center; gap: 0.75rem; min-width: 0; }

.back-link {
  .flex-center();
  width: 32px;
  height: 32px;
  border-radius: @r-lg;
  background: @sf-4;
  border: 1px solid rgba(255, 255, 255, 0.06);
  color: @sf-50;
  cursor: pointer;
  font-size: 1rem;
  flex-shrink: 0;
  transition: @tr-base;

  &:hover { background: @sf-8; color: @sf-80; }
}

.back-arrow { line-height: 1; }

.title-text { min-width: 0; }
.title-text h1 { margin: 0; font-size: 1.15rem; font-weight: 700; color: @c-white; letter-spacing: -0.01em; }

.daemon-name { font-size: 0.78rem; color: @sf-30; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; display: block; }

.log-controls { display: flex; align-items: center; gap: @space-xl; flex-shrink: 0; }

.toggle {
  display: flex;
  align-items: center;
  gap: 0.4rem;
  cursor: pointer;
  font-size: 0.78rem;
  color: @sf-40;
  user-select: none;

  input { width: 14px; height: 14px; accent-color: @c-accent-dim; cursor: pointer; }
}

.toggle-label { transition: color 0.15s; }
.toggle:hover .toggle-label { color: @sf-60; }

.line-count { .font-mono(0.75rem; @sf-25); font-variant-numeric: tabular-nums; }

.alert { .alert-base(); background: rgba(220, 38, 38, 0.08); border: 1px solid rgba(220, 38, 38, 0.15); color: @c-accent; }
.alert-icon { font-weight: 700; flex-shrink: 0; }

.log-container {
  flex: 1;
  overflow-y: auto;
  background: rgba(255, 255, 255, 0.015);
  border: 1px solid rgba(255, 255, 255, 0.05);
  border-radius: @r-2xl;
  padding: 0.6rem 0;
  .font-mono(0.78rem; @sf-65);
  line-height: 1.6;
}

.log-line {
  display: flex;
  padding: 0.08rem 0.85rem;
  white-space: pre-wrap;
  word-break: break-all;
  gap: 0.6rem;

  &:hover { background: rgba(255, 255, 255, 0.02); }
}

.line-ts { color: @sf-15; flex-shrink: 0; min-width: 64px; font-variant-numeric: tabular-nums; }
.line-num { color: rgba(255, 255, 255, 0.12); min-width: 42px; text-align: right; user-select: none; flex-shrink: 0; font-variant-numeric: tabular-nums; }
.line-content { flex: 1; color: @sf-65; }
.line-content :deep(span) { display: inline; }

.empty { .flex-center(); flex-direction: column; height: 100%; color: @sf-15; text-align: center; gap: 0.5rem; }
.empty-icon { font-size: 2rem; color: @sf-5; }
.empty p { margin: 0; font-size: 0.85rem; }

.live-indicator { display: flex; align-items: center; gap: 0.35rem; padding: 0.5rem 0.85rem; font-size: 0.72rem; color: @sf-20; font-variant-numeric: tabular-nums; }

.live-dot {
  width: 6px;
  height: 6px;
  border-radius: 50%;
  background: @c-success;
  animation: pulse 1.5s ease-in-out infinite;
}

@keyframes pulse {
  0%, 100% { opacity: 1; }
  50% { opacity: 0.3; }
}
</style>
