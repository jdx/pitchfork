<script setup lang="ts">
const props = defineProps<{
  name: string
  count: number
  expanded: boolean
}>()

const emit = defineEmits<{
  toggle: []
}>()
</script>

<template>
  <div class="ns-group">
    <button class="ns-header" @click="emit('toggle')">
      <div class="ns-name">
        <svg
          class="ns-chevron"
          :class="{ collapsed: !expanded }"
          viewBox="0 0 24 24"
          fill="none"
          stroke="currentColor"
          stroke-width="2.5"
          stroke-linecap="round"
          stroke-linejoin="round"
        >
          <polyline points="6 9 12 15 18 9" />
        </svg>
        <svg
          class="ns-icon"
          viewBox="0 0 24 24"
          fill="none"
          stroke="currentColor"
          stroke-width="2"
          stroke-linecap="round"
          stroke-linejoin="round"
        >
          <path d="M22 19a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h5l2 3h9a2 2 0 0 1 2 2z" />
        </svg>
        {{ name }}
      </div>
      <span class="ns-count">{{ count }}</span>
    </button>
    <div v-show="expanded" class="ns-content">
      <slot />
    </div>
  </div>
</template>

<style scoped lang="less">
@import '@/styles/mixins.less';

.ns-group { .ns-group-surface(); }

.ns-header { .ns-header-surface(); }

.ns-name {
  .flex-center();
  gap: 0.45rem;
  .font-sans(0.9rem; @sf-65; 600);
}

.ns-chevron {
  width: 16px;
  height: 16px;
  opacity: 0.6;
  transition: transform 0.15s ease;
  flex-shrink: 0;

  &.collapsed { transform: rotate(-90deg); }
}

.ns-icon { width: 16px; height: 16px; opacity: 0.65; }

.ns-count {
  .font-sans(0.72rem; @sf-40; 500);
  background: @sf-5;
  padding: 0.12rem 0.45rem;
  border-radius: @r-sm;
  font-variant-numeric: tabular-nums;
}

.ns-content {
  padding: 0;

  :deep(.daemon-table),
  :deep(.proxy-table) {
    border-radius: 0;
    border-top: none;
  }
}
</style>
