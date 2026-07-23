<script setup lang="ts">
import { computed, ref, watch, nextTick, onMounted } from 'vue'
import { useLogStream, type LogStreamFilters, api } from '@/composables/useApi'
import { useRouter } from 'vue-router'
import { parseLogLines, type ParsedLogLine } from '@/utils/log'

const props = defineProps<{ id: string }>()
const router = useRouter()

const logContainer = ref<HTMLDivElement | null>(null)
const autoScroll = ref(true)
const showTimestamps = ref(false)

const showFilters = ref(false)
const filterLevel = ref('')
const filterLogger = ref('')
const filterSearch = ref('')
const filterSearchRegex = ref(false)
const filterSearchCase = ref(false)
const filterJq = ref('')
const filterSince = ref('')
const filterUntil = ref('')

const availableLoggers = ref<string[]>([])
const jqFieldKeys = ref<string[]>([])

async function fetchLoggers() {
  try {
    const data = await api<string[]>(`/logs/${encodeURIComponent(props.id)}/loggers`)
    availableLoggers.value = data
  } catch {
    availableLoggers.value = []
  }
}

async function fetchFieldKeys() {
  try {
    const data = await api<string[]>(`/logs/${encodeURIComponent(props.id)}/field-keys`)
    jqFieldKeys.value = data
  } catch {
    jqFieldKeys.value = []
  }
}

onMounted(() => {
  fetchLoggers()
  fetchFieldKeys()
})
watch(() => props.id, () => {
  fetchLoggers()
  fetchFieldKeys()
})

const jqDropdownOpen = ref(false)
const jqActiveIndex = ref(0)

const jqBuiltins = [
  'select', 'map', 'map_values', 'length', 'keys', 'keys_unsorted',
  'values', 'to_entries', 'from_entries', 'with_entries', 'paths',
  'getpath', 'setpath', 'del', 'has', 'in', 'contains', 'inside',
  'type', 'tostring', 'tonumber', 'tojson', 'fromjson', 'arrays',
  'objects', 'iterables', 'booleans', 'numbers', 'strings', 'nulls',
  'scalars', 'empty', 'not', 'and', 'or', 'error', 'add', 'unique',
  'unique_by', 'sort', 'sort_by', 'group_by', 'min', 'max', 'min_by',
  'max_by', 'flatten', 'range', 'first', 'last', 'nth', 'reverse',
  'index', 'indices', 'join', 'split', 'ascii_downcase', 'ascii_upcase',
  'ltrimstr', 'rtrimstr', 'startswith', 'endswith', 'test', 'match',
  'capture', 'scan', 'sub', 'gsub', 'explode', 'implode', 'floor',
  'ceil', 'round', 'sqrt', 'pow', 'abs', 'recurse', 'env',
]

const topLevelKeys = ['timestamp', 'daemon_id', 'message', 'level', 'msg', 'logger', 'fields']

const jqSuggestions = computed(() => {
  const text = filterJq.value
  if (!text) return []

  // Find the current word being typed (after last whitespace/operator).
  // Dot is NOT a delimiter here — it's part of field access paths like
  // `.fields.user`, so we keep it in the word and split on it manually.
  const wordMatch = text.match(/[^\s()[\]{}:;,+\-*/=<>!&|]+$/)
  if (!wordMatch) return []
  const word = wordMatch[0]

  // Field access mode: word contains a dot (e.g. ".fields.user")
  const lastDotIdx = word.lastIndexOf('.')
  if (lastDotIdx >= 0) {
    const prefix = word.slice(lastDotIdx + 1).toLowerCase()
    const allKeys = [...new Set([...topLevelKeys, ...jqFieldKeys.value])]
    return allKeys
      .filter((k) => k.toLowerCase().startsWith(prefix))
      .map((k) => word.slice(0, lastDotIdx + 1) + k)
      .slice(0, 20)
  }

  // Function name mode (e.g. "select", "map")
  const prefix = word.toLowerCase()
  return jqBuiltins
    .filter((b) => b.toLowerCase().startsWith(prefix))
    .slice(0, 20)
})

watch(jqSuggestions, () => {
  jqActiveIndex.value = 0
})

function applyJqSuggestion(item: string) {
  const text = filterJq.value
  const wordMatch = text.match(/[^\s()[\]{}:;,+\-*/=<>!&|]+$/)
  if (wordMatch) {
    const before = text.slice(0, text.length - wordMatch[0].length)
    filterJq.value = before + item
  } else {
    filterJq.value = item
  }
  jqDropdownOpen.value = false
}

function onJqKeydown(e: KeyboardEvent) {
  const suggestions = jqSuggestions.value
  if (!jqDropdownOpen.value || suggestions.length === 0) return

  if (e.key === 'ArrowDown') {
    e.preventDefault()
    jqActiveIndex.value = (jqActiveIndex.value + 1) % suggestions.length
  } else if (e.key === 'ArrowUp') {
    e.preventDefault()
    jqActiveIndex.value = (jqActiveIndex.value - 1 + suggestions.length) % suggestions.length
  } else if (e.key === 'Enter') {
    e.preventDefault()
    applyJqSuggestion(suggestions[jqActiveIndex.value])
  } else if (e.key === 'Escape') {
    jqDropdownOpen.value = false
  }
}

const debouncedFilters = ref<LogStreamFilters>({})
let debounceTimer: ReturnType<typeof setTimeout> | null = null

watch(
  [filterLevel, filterLogger, filterSearch, filterSearchRegex, filterSearchCase, filterJq, filterSince, filterUntil],
  () => {
    if (debounceTimer) clearTimeout(debounceTimer)
    debounceTimer = setTimeout(() => {
      const search = filterSearch.value.trim()
      debouncedFilters.value = {
        level: filterLevel.value || undefined,
        logger: filterLogger.value || undefined,
        grep: search && !filterSearchRegex.value ? search : undefined,
        regex: search && filterSearchRegex.value ? search : undefined,
        caseSensitive: search && filterSearchCase.value ? true : undefined,
        jq: filterJq.value.trim() || undefined,
        since: filterSince.value || undefined,
        until: filterUntil.value || undefined,
      }
    }, 500)
  },
  { immediate: true },
)

const decodedId = computed(() => decodeURIComponent(props.id))
const { lines, error, connected } = useLogStream(decodedId, debouncedFilters)
const parsedLines = computed<ParsedLogLine[]>(() => parseLogLines(lines.value))

function goBack() {
  router.push(`/daemon/${encodeURIComponent(props.id)}`)
}

watch(() => lines.value.length, () => {
  if (autoScroll.value) {
    nextTick(() => {
      if (logContainer.value) {
        logContainer.value.scrollTop = logContainer.value.scrollHeight
      }
    })
  }
}, { immediate: true })

function onScroll() {
  if (!logContainer.value) return
  const { scrollTop, scrollHeight, clientHeight } = logContainer.value
  autoScroll.value = scrollHeight - scrollTop - clientHeight < 20
}

function clearFilters() {
  filterLevel.value = ''
  filterLogger.value = ''
  filterSearch.value = ''
  filterSearchRegex.value = false
  filterSearchCase.value = false
  filterJq.value = ''
  filterSince.value = ''
  filterUntil.value = ''
}

const activeFilterCount = computed(() => {
  let count = 0
  if (filterLevel.value) count++
  if (filterLogger.value) count++
  if (filterSearch.value.trim()) count++
  if (filterJq.value.trim()) count++
  if (filterSince.value) count++
  if (filterUntil.value) count++
  return count
})
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
        <button class="filter-toggle-btn" @click="showFilters = !showFilters">
          <span class="filter-toggle-icon">⚡</span>
          <span>Filters</span>
          <span v-if="activeFilterCount > 0" class="filter-badge">{{ activeFilterCount }}</span>
        </button>
        <div class="line-count">{{ lines.length.toLocaleString() }} lines</div>
      </div>
    </div>

    <div v-if="showFilters" class="filter-bar">
      <div class="filter-row">
        <div class="filter-group">
          <label class="filter-label">Level</label>
          <select v-model="filterLevel" class="filter-select">
            <option value="">All</option>
            <option value="error">Error</option>
            <option value="warn">Warn</option>
            <option value="info">Info</option>
            <option value="debug">Debug</option>
            <option value="trace">Trace</option>
          </select>
        </div>
        <div class="filter-group">
          <label class="filter-label">Logger</label>
          <select v-model="filterLogger" class="filter-select">
            <option value="">All</option>
            <option v-for="lg in availableLoggers" :key="lg" :value="lg">{{ lg }}</option>
          </select>
        </div>
        <div class="filter-group filter-group-wide">
          <label class="filter-label">Search</label>
          <div class="filter-search-wrap">
            <input
              v-model="filterSearch"
              type="text"
              class="filter-input"
              placeholder="Search message..."
            />
            <label class="filter-tick">
              <input v-model="filterSearchRegex" type="checkbox" />
              <span>Regex</span>
            </label>
            <label class="filter-tick">
              <input v-model="filterSearchCase" type="checkbox" />
              <span>Case</span>
            </label>
          </div>
        </div>
        <div class="filter-group filter-group-wide">
          <label class="filter-label">jq</label>
          <div class="jq-input-wrap">
            <input
              v-model="filterJq"
              type="text"
              class="filter-input"
              placeholder='.level == "error"'
              spellcheck="false"
              @focus="jqDropdownOpen = true"
              @blur="jqDropdownOpen = false"
              @keydown="onJqKeydown"
            />
            <ul
              v-show="jqDropdownOpen && jqSuggestions.length > 0"
              class="jq-dropdown"
              @mousedown.prevent
            >
              <li
                v-for="(item, idx) in jqSuggestions"
                :key="item"
                :class="{ 'is-active': idx === jqActiveIndex }"
                @mousedown.prevent="applyJqSuggestion(item)"
              >
                {{ item }}
              </li>
            </ul>
          </div>
        </div>
        <div class="filter-group">
          <label class="filter-label">Since</label>
          <input v-model="filterSince" type="datetime-local" class="filter-input filter-datetime" />
        </div>
        <div class="filter-group">
          <label class="filter-label">Until</label>
          <input v-model="filterUntil" type="datetime-local" class="filter-input filter-datetime" />
        </div>
        <button v-if="activeFilterCount > 0" class="filter-clear" @click="clearFilters">
          Clear
        </button>
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
        :class="line.level ? `log-line-${line.level}` : ''"
      >
        <span v-if="showTimestamps && line.timestamp" class="line-ts">
          <span class="line-ts-combined">{{ line.timestamp.slice(5, 10) }} {{ line.timestamp.slice(11) }}</span>
          <span class="line-ts-date">{{ line.timestamp.slice(5, 10) }}</span>
          <span class="line-ts-time">{{ line.timestamp.slice(11) }}</span>
        </span>
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

.filter-toggle-btn {
  .ghost-btn();
  position: relative;
}

.filter-toggle-icon { font-size: 0.85rem; }

.filter-badge {
  .badge-sm();
  .badge-status(@sf-danger-8; @c-danger);
  margin-left: 0.15rem;
}

.line-count { .font-mono(0.75rem; @sf-25); font-variant-numeric: tabular-nums; }

.filter-bar {
  margin-bottom: @space-xl;
  background: @sf-2;
  border: 1px solid rgba(255, 255, 255, 0.05);
  border-radius: @r-2xl;
  padding: @space-lg @space-xl;
}

.filter-row {
  display: grid;
  grid-template-columns: repeat(3, 1fr);
  gap: @space-md;
  align-items: end;
}

.filter-group {
  display: flex;
  flex-direction: column;
  gap: 0.25rem;
  min-width: 0;
}

.filter-label {
  .label-micro();
  padding-left: 0.15rem;
}

.filter-input,
.filter-select {
  background: @sf-3;
  border: 1px solid rgba(255, 255, 255, 0.06);
  border-radius: @r-md;
  padding: 0.4rem 0.55rem;
  color: @sf-70;
  font-family: @ff-mono;
  font-size: 0.78rem;
  line-height: 1.4;
  outline: none;
  transition: @tr-base;
  width: 100%;
  height: 2rem;
  box-sizing: border-box;

  &::placeholder { color: @sf-25; }

  &:focus {
    border-color: rgba(255, 255, 255, 0.12);
    background: @sf-4;
  }
}

.filter-select {
  cursor: pointer;
  appearance: none;
  background-image: url("data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' width='10' height='6' fill='none'%3E%3Cpath d='M1 1l4 4 4-4' stroke='%236b7280' stroke-width='1.5' stroke-linecap='round' stroke-linejoin='round'/%3E%3C/svg%3E");
  background-repeat: no-repeat;
  background-position: right 0.55rem center;
  padding-right: 1.6rem;
}

.filter-datetime {
  min-width: 0;
}

.filter-clear {
  .ghost-btn();
  flex-shrink: 0;
  margin-left: auto;
  color: @c-accent;

  &:hover {
    background: @sf-danger-8;
  }
}

.filter-group-wide {
  flex: 1 1 140px;
  min-width: 0;
}

.filter-search-wrap {
  display: flex;
  align-items: center;
  gap: 0.4rem;
}

.filter-search-wrap .filter-input {
  flex: 1;
}

.filter-tick {
  display: flex;
  align-items: center;
  gap: 0.25rem;
  cursor: pointer;
  font-size: 0.72rem;
  color: @sf-40;
  user-select: none;
  white-space: nowrap;
  flex-shrink: 0;

  input {
    width: 14px;
    height: 14px;
    accent-color: @c-accent-dim;
    cursor: pointer;
  }
}

.jq-input-wrap {
  position: relative;
}

.jq-dropdown {
  position: absolute;
  top: 100%;
  left: 0;
  right: 0;
  margin-top: 0.25rem;
  background: @sf-3;
  border: 1px solid rgba(255, 255, 255, 0.08);
  border-radius: @r-md;
  max-height: 200px;
  overflow-y: auto;
  z-index: 100;
  list-style: none;
  padding: 0.25rem 0;
  font-family: @ff-mono;
  font-size: 0.78rem;

  li {
    padding: 0.35rem 0.6rem;
    cursor: pointer;
    color: @sf-70;

    &:hover,
    &.is-active {
      background: @sf-6;
      color: @sf-85;
    }
  }
}

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

.line-ts { color: @sf-30; flex-shrink: 0; min-width: 0; font-variant-numeric: tabular-nums; display: inline-flex; align-items: center; align-self: flex-start; }
.line-ts-combined { font-size: 0.72rem; }
.line-ts-date { display: none; }
.line-ts-time { display: none; }
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

.log-line-error .line-ts { color: @c-danger; }
.log-line-warn .line-ts { color: @c-warning; }

@media (max-width: 640px) {
  .log-header { gap: @space-md; }
  .log-controls { gap: @space-md; flex-wrap: wrap; }
  .filter-row { display: flex; flex-direction: column; align-items: stretch; }
  .filter-group { width: 100%; flex: 0 0 auto; }
  .filter-group-wide { width: 100%; flex: 0 0 auto; }
  .filter-search-wrap { flex-wrap: wrap; }
  .filter-clear { margin-left: 0; width: 100%; justify-content: center; }
  .line-num { display: none; }
  .log-line { gap: 0.4rem; padding: 0.08rem 0.5rem; }
  .line-ts { min-width: 40px; margin-right: 0.35rem; flex-direction: column; align-items: flex-start; }
  .line-ts-combined { display: none; }
  .line-ts-date { display: inline; font-size: 0.68rem; line-height: 1.6; opacity: 0.85; }
  .line-ts-time { display: inline; font-size: 0.72rem; line-height: 1.3; }
}
</style>

<!-- Non-scoped styles for v-html injected content (scoped styles don't apply to v-html) -->
<style lang="less">
@import '@/styles/variables.less';

.log-level-badge {
  font-weight: 700;
}

.log-level-bracket {
  opacity: 0.5;
}

.log-level-error { color: @c-danger; }
.log-level-warn { color: @c-warning; }
.log-level-info { color: @c-info; }
.log-level-debug { color: #a855f7; }
.log-level-trace { color: @sf-50; opacity: 0.7; }

.log-logger { font-style: italic; opacity: 0.5; }

.log-msg { font-weight: 700; color: rgba(255, 255, 255, 0.85); }
.log-msg-error { color: @c-danger; }
.log-msg-warn { color: @c-warning; }

.log-sep { opacity: 0.5; }

.log-field-key { color: #3b82f6; }
.log-field-number { color: @c-success; }
.log-field-true { color: @c-warning; }
.log-field-false { color: @c-danger; }
.log-field-null { opacity: 0.5; }
.log-field-string { color: rgba(255, 255, 255, 0.85); }
.log-field-complex { color: @c-info; }
</style>
