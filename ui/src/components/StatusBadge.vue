<script setup lang="ts">
const props = defineProps<{
  status: string | null
  variant?: 'card' | 'table'
  showPort?: boolean
  port?: number | null
}>()

const v = props.variant ?? 'table'

function statusClass(s: string | null): string {
  if (!s) return ''
  return s
}
</script>

<template>
  <span
    v-if="status"
    class="badge"
    :class="[statusClass(status), v]"
  >
    <template v-if="showPort && port != null">
      {{ status }}&nbsp;·&nbsp;{{ port }}
    </template>
    <template v-else>
      {{ status }}
    </template>
  </span>
  <span v-else class="placeholder">—</span>
</template>

<style scoped lang="less">
@import '@/styles/mixins.less';

.badge {
  .badge-base();
  display: inline-block;

  &.card {
    font-size: 0.68rem;
    padding: 0.18rem 0.6rem;
    border-radius: @r-md;
    border: 1px solid;

    &.running { background: @sf-success-12; color: @c-success; border-color: @sf-success-20; }
    &.stopped { background: @sf-3; color: @sf-30; border-color: @sf-8; }
    &.waiting, &.stopping { background: @sf-warning-8; color: @c-warning; border-color: @sf-warning-15; }
    &.failed, &.errored { background: @sf-danger-8; color: @c-danger; border-color: @sf-danger-15; }
    &.available { background: @sf-info-8; color: @c-info; border-color: @sf-info-15; }
  }

  &.table {
    font-size: 0.72rem;
    padding: 0.14rem 0.45rem;
    border-radius: @r-sm;

    &.running { background: @sf-success-10; color: @c-success; }
    &.stopped { background: @sf-4; color: @sf-30; }
    &.waiting, &.stopping { background: @sf-warning-8; color: @c-warning; }
    &.failed, &.errored { background: @sf-danger-8; color: @c-danger; }
    &.available { background: @sf-info-8; color: @c-info; }
  }
}

.placeholder {
  .font-mono(0.82rem; @sf-15);
}
</style>
