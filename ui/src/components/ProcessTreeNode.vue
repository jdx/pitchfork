<script setup lang="ts">
import { ref } from 'vue'
import type { ProcessTree } from '@/types/api'

const props = defineProps<{ node: ProcessTree; depth: number }>()
const childrenExpanded = ref(true)
const detailsExpanded = ref(false)

function toggleChildren() {
  childrenExpanded.value = !childrenExpanded.value
}

function toggleDetails() {
  detailsExpanded.value = !detailsExpanded.value
}

function formatBytes(n: number): string {
  if (n === 0) return '0 B'
  const units = ['B', 'KB', 'MB', 'GB', 'TB']
  const i = Math.floor(Math.log(n) / Math.log(1024))
  return `${(n / Math.pow(1024, i)).toFixed(1)} ${units[i]}`
}
</script>

<template>
  <div class="tree-node">
    <div class="tree-row" :style="{ paddingLeft: `${depth * 1.2}rem` }" @click="toggleDetails">
      <div class="tree-main">
        <span class="tree-toggle" v-if="node.children.length" @click.stop="toggleChildren">
          <svg v-if="childrenExpanded" width="10" height="10" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="3"><polyline points="6 9 12 15 18 9"/></svg>
          <svg v-else width="10" height="10" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="3"><polyline points="9 6 15 12 9 18"/></svg>
        </span>
        <span v-else class="tree-spacer" />
        <span class="tree-pid">{{ node.pid }}</span>
        <span class="tree-name">{{ node.name }}</span>
        <span class="tree-exe" v-if="node.exe">{{ node.exe }}</span>
      </div>
      <div class="tree-stats">
        <span class="tree-stat" :class="node.cpu_percent > 50 ? 'high' : ''">{{ node.cpu_percent.toFixed(1) }}%</span>
        <span class="tree-stat">{{ formatBytes(node.memory_bytes) }}</span>
        <span class="tree-stat">{{ node.thread_count }}t</span>
        <span class="tree-stat tree-status" :class="node.status.toLowerCase()">{{ node.status }}</span>
      </div>
    </div>
    <!-- Mobile: expand metrics inline when tapped -->
    <div v-if="detailsExpanded" class="tree-details" :style="{ marginLeft: `${depth * 1.2 + 0.5}rem` }">
      <div class="detail-row">
        <span class="detail-label">CPU</span>
        <span class="detail-value" :class="node.cpu_percent > 50 ? 'high' : ''">{{ node.cpu_percent.toFixed(1) }}%</span>
      </div>
      <div class="detail-row">
        <span class="detail-label">Memory</span>
        <span class="detail-value">{{ formatBytes(node.memory_bytes) }}</span>
      </div>
      <div class="detail-row">
        <span class="detail-label">Threads</span>
        <span class="detail-value">{{ node.thread_count }}</span>
      </div>
      <div class="detail-row">
        <span class="detail-label">Status</span>
        <span class="detail-value" :class="node.status.toLowerCase()">{{ node.status }}</span>
      </div>
      <div class="detail-row" v-if="node.exe">
        <span class="detail-label">Exe</span>
        <span class="detail-value detail-exe">{{ node.exe }}</span>
      </div>
    </div>
    <div v-if="childrenExpanded" class="tree-children">
      <ProcessTreeNode
        v-for="child in node.children"
        :key="child.pid"
        :node="child"
        :depth="depth + 1"
      />
    </div>
  </div>
</template>

<style scoped lang="less">
@import '@/styles/mixins.less';

.tree-node { user-select: none; }

.tree-row {
  .flex-between();
  padding: 0.35rem 0;
  border-bottom: 1px solid rgba(255, 255, 255, 0.03);
  cursor: pointer;
  transition: background 0.1s;

  &:hover { background: rgba(255, 255, 255, 0.02); }
  &:last-child { border-bottom: none; }
}

.tree-main { .flex-center(); gap: 0.4rem; min-width: 0; }

.tree-toggle { .flex-center(); width: 14px; height: 14px; cursor: pointer; color: @sf-30; transition: color 0.15s; flex-shrink: 0; }
.tree-toggle:hover { color: @sf-60; }
.tree-spacer { width: 14px; flex-shrink: 0; }

.tree-pid { color: @sf-30; font-size: 0.72rem; min-width: 3.5ch; text-align: right; }
.tree-name { color: @sf-75; font-weight: 600; .truncate(); }
.tree-exe { color: @sf-30; font-size: 0.72rem; .truncate(); max-width: 200px; }

.tree-stats { .flex-center(); gap: 0.75rem; flex-shrink: 0; }

.tree-stat { color: @sf-35; font-size: 0.72rem; min-width: 5ch; text-align: right; }
.tree-stat.high { color: @c-warning; }
.tree-status { text-transform: lowercase; min-width: 6ch; }
.tree-status.running, .tree-status.run { color: @c-success; }
.tree-status.sleeping, .tree-status.sleep { color: @sf-30; }
.tree-status.zombie { color: @c-danger; }

.tree-children { border-left: 1px solid rgba(255, 255, 255, 0.04); margin-left: 0.5rem; }

/* Inline details panel (mobile) */
.tree-details { display: none; }

.detail-row { .flex-between(); padding: 0.2rem 0; font-size: 0.72rem; border-bottom: 1px solid rgba(255, 255, 255, 0.02); }
.detail-row:last-child { border-bottom: none; }

.detail-label { color: @sf-30; text-transform: uppercase; letter-spacing: 0.04em; }
.detail-value { color: @sf-55; font-weight: 500; }
.detail-value.high { color: @c-warning; }
.detail-value.running, .detail-value.run { color: @c-success; }
.detail-value.sleeping, .detail-value.sleep { color: @sf-30; }
.detail-value.zombie { color: @c-danger; }
.detail-exe { max-width: 200px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }

.mobile-sm({
  .tree-stats { display: none; }
  .tree-exe { display: none; }
  .tree-details {
    display: block;
    padding: 0.4rem 0.6rem;
    margin-bottom: 0.25rem;
    background: rgba(255, 255, 255, 0.015);
    border-radius: @r-md;
    border: 1px solid rgba(255, 255, 255, 0.04);
  }
  .tree-row { padding: 0.5rem 0; }
});
</style>
