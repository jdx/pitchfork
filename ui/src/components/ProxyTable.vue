<script setup lang="ts">
import type { ProxyWorktreeEntry } from '@/types/api'
import ProxyRow from './ProxyRow.vue'
import ProxyCard from './ProxyCard.vue'

const props = defineProps<{
  proxies: ProxyWorktreeEntry[]
  prefersCard: boolean
}>()

defineEmits<{ refresh: [] }>()
</script>

<template>
  <table class="proxy-table" :class="prefersCard ? 'hidden' : 'show-table'">
    <thead>
      <tr>
        <th class="col-branch">Branch</th>
        <th class="col-url">URL</th>
        <th class="col-status">Status</th>
        <th class="col-uptime">Uptime</th>
        <th class="col-n">Port</th>
        <th class="col-actions">Actions</th>
      </tr>
    </thead>
    <tbody>
      <ProxyRow
        v-for="p in proxies"
        :key="`${p.slug}:${p.branch}`"
        :proxy="p"
        @refresh="$emit('refresh')"
      />
    </tbody>
  </table>
  <div class="proxy-grid" :class="prefersCard ? 'show-grid' : 'hidden'">
    <ProxyCard
      v-for="p in proxies"
      :key="`${p.slug}:${p.branch}`"
      :proxy="p"
      @refresh="$emit('refresh')"
    />
  </div>
</template>

<style scoped lang="less">
@import '@/styles/mixins.less';

.proxy-table { .table-base(); }

.col-branch   { width: 18%; text-align: left; }
.col-url      { width: 20%; text-align: left; }
.col-status   { width: 11%; text-align: center; }
.col-uptime   { width: 10%; text-align: center; }
.col-n        { width: 8%; text-align: center; }
.col-actions  { width: 33%; text-align: right; }

.proxy-grid { .card-grid(); }

.show-table { display: table; }
.show-grid  { display: grid; }
.hidden     { display: none !important; }

.mobile({
  .proxy-table { display: none !important; }
  .proxy-grid  { display: grid !important; }
});
</style>
