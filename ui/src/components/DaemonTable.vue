<script setup lang="ts">
import type { DaemonEntry } from '@/types/api'
import DaemonRow from './DaemonRow.vue'
import DaemonCard from './DaemonCard.vue'

const props = defineProps<{
  daemons: DaemonEntry[]
  prefersCard: boolean
}>()

defineEmits<{ refresh: [] }>()
</script>

<template>
  <table class="daemon-table" :class="prefersCard ? 'hidden' : 'show-table'">
    <thead>
      <tr>
        <th class="col-name">Name</th>
        <th class="col-status">Status</th>
        <th class="col-uptime">Uptime</th>
        <th class="col-n">CPU</th>
        <th class="col-n">MEM</th>
        <th class="col-actions">Actions</th>
      </tr>
    </thead>
    <tbody>
      <DaemonRow
        v-for="d in daemons"
        :key="d.id.qualified"
        :daemon="d"
        @refresh="$emit('refresh')"
      />
    </tbody>
  </table>
  <div class="daemon-grid" :class="prefersCard ? 'show-grid' : 'hidden'">
    <DaemonCard
      v-for="d in daemons"
      :key="d.id.qualified"
      :daemon="d"
      @refresh="$emit('refresh')"
    />
  </div>
</template>

<style scoped lang="less">
@import '@/styles/mixins.less';

.daemon-table { .table-base(); }

.col-name    { width: 26%; text-align: left; }
.col-status  { width: 11%; text-align: center; }
.col-uptime  { width: 10%; text-align: center; }
.col-n       { width: 8%; text-align: center; }
.col-actions { width: 32%; text-align: right; }

.daemon-grid { .card-grid(); }

.show-table { display: table; }
.show-grid  { display: grid; }
.hidden     { display: none !important; }

.mobile({
  .daemon-table { display: none !important; }
  .daemon-grid  { display: grid !important; }
});
</style>
