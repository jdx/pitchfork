<script setup lang="ts">
import { ref, toRef } from 'vue'
import { useStack } from '@/composables/useApi'
import StackGroups from '@/components/StackGroups.vue'

const props = defineProps<{ project: string; worktree: string }>()
const { stack, loading, error, refresh } = useStack(
  toRef(props, 'project'),
  toRef(props, 'worktree'),
)

function safeGet(key: string): string | null {
  try { return localStorage.getItem(key) } catch { return null }
}
const prefersCard = ref(safeGet('pitchfork.viewMode') === 'card')
</script>

<template>
  <div class="stack">
    <div class="page-header">
      <div>
        <div class="crumbs">
          <router-link to="/projects" class="crumb">Projects</router-link>
          <span class="crumb-sep">/</span>
          <router-link :to="`/projects/${encodeURIComponent(project)}`" class="crumb">{{ project }}</router-link>
          <span class="crumb-sep">/</span>
        </div>
        <h1 class="page-title">{{ worktree }}</h1>
        <span class="page-subtitle">
          <span v-if="stack">
            {{ stack.branch }} &middot; {{ stack.namespace }} &middot;
            {{ stack.daemons.running }}/{{ stack.daemons.total }} daemons running
          </span>
        </span>
      </div>
      <button class="btn-ghost" :disabled="loading" @click="refresh">Refresh</button>
    </div>

    <div v-if="error" class="alert alert-error">{{ error }}</div>

    <template v-if="stack">
      <p class="dir">{{ stack.dir }}</p>
      <p class="note">Starting is always a click: opening this page does not start anything.</p>
      <StackGroups :stack="stack" :prefers-card="prefersCard" @refresh="refresh" />
    </template>
  </div>
</template>

<style scoped lang="less">
@import '@/styles/mixins.less';

.stack { width: 100%; max-width: @max-content; margin: 0 auto; }

.page-header { .flex-between(); margin-bottom: @space-xl; gap: @space-xl; padding-bottom: 0.6rem; border-bottom: 1px solid rgba(255, 255, 255, 0.04); }
.page-title { margin: 0; font-size: 1.4rem; font-weight: 700; color: @c-white; letter-spacing: -0.02em; }
.page-subtitle { font-size: 0.8rem; color: @sf-30; }
.crumbs { .font-sans(0.75rem; @sf-30; 500); display: flex; gap: 0.3rem; }
.crumb { color: @sf-40; text-decoration: none; &:hover { color: @c-accent-dim; } }
.crumb-sep { color: @sf-15; }
.btn-ghost { .ghost-btn(); }
.alert { .alert-error(); }

.dir { margin: 0 0 0.2rem 0; .font-mono(0.75rem; @sf-30); }
.note { margin: 0 0 @space-xl 0; .font-sans(0.75rem; @sf-30; 400); }
</style>
