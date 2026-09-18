<script setup lang="ts">
import { computed, ref } from 'vue'
import DaemonTable from './DaemonTable.vue'
import { api, useGroupActions } from '@/composables/useApi'
import { toast } from 'vue-sonner'
import type { Stack } from '@/types/api'

const props = defineProps<{ stack: Stack; prefersCard: boolean }>()
const emit = defineEmits<{ refresh: [] }>()

const { start, stop, restart, acting } = useGroupActions()

// The `default` group is the stack's primary action, so its buttons say
// "Start stack" rather than repeating the group name.
const groups = computed(() => props.stack.groups)

// The supervisor resolves daemon configs from its own project and from the
// namespace registry. A worktree in neither is listed, but starting its
// daemons would fail, so its actions stay disabled until it is registered.
const startable = computed(() => props.stack.can_start)
const blockedReason = computed(() =>
  startable.value
    ? undefined
    : `The supervisor has no config for ${props.stack.unresolvable_daemons.join(', ')}. `
      + 'Register this worktree to start it.',
)

const registering = ref(false)
async function registerWorktree() {
  if (registering.value) return
  registering.value = true
  try {
    const res = await api<{ ok: boolean; name?: string; error?: string }>('/namespaces', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ dir: props.stack.dir }),
    })
    toast.success(`Registered ${res.name ?? props.stack.namespace}`, { duration: 2000 })
    emit('refresh')
  } catch (e: any) {
    toast.error('Register worktree failed', { duration: 4000, description: e.message })
  } finally {
    registering.value = false
  }
}

function key(groupName: string): string {
  return `${props.stack.project}/${props.stack.worktree}/${groupName}`
}

function ids(groupName: string): string[] {
  const group = props.stack.groups.find(g => g.name === groupName)
  return group ? group.daemons.map(d => d.id.qualified) : []
}

/** Members the group declares that no known daemon matches. */
function missing(groupName: string): string[] {
  return props.stack.groups.find(g => g.name === groupName)?.missing ?? []
}

function isActing(groupName: string): boolean {
  return acting.value.has(key(groupName))
}

async function onStart(groupName: string) {
  await start(key(groupName), ids(groupName), missing(groupName))
  emit('refresh')
}
async function onStop(groupName: string) {
  await stop(key(groupName), ids(groupName), missing(groupName))
  emit('refresh')
}
async function onRestart(groupName: string) {
  await restart(key(groupName), ids(groupName), missing(groupName))
  emit('refresh')
}
</script>

<template>
  <div class="stack-groups">
    <div v-if="!startable" class="unregistered">
      <p>
        The supervisor has no config for
        <code>{{ stack.unresolvable_daemons.join(', ') }}</code>, so starting them would
        fail. Register this worktree as namespace <code>{{ stack.namespace }}</code> to
        enable its actions.
      </p>
      <button class="act-btn" :disabled="registering" @click="registerWorktree">
        Register worktree
      </button>
    </div>

    <section v-for="group in groups" :key="group.name" class="group" :class="{ primary: group.is_default }">
      <header class="group-header">
        <div class="group-title">
          <h3 class="group-name">{{ group.name }}</h3>
          <span v-if="group.is_default" class="group-tag">stack</span>
          <span class="group-count">{{ group.running }}/{{ group.total }} running</span>
        </div>
        <div class="group-actions">
          <button class="act-btn act-start" :disabled="isActing(group.name) || !startable || group.daemons.length === 0" @click="onStart(group.name)">
            {{ group.is_default ? 'Start stack' : 'Start' }}
          </button>
          <button class="act-btn act-stop" :disabled="isActing(group.name) || !startable || group.daemons.length === 0" @click="onStop(group.name)">
            {{ group.is_default ? 'Stop stack' : 'Stop' }}
          </button>
          <button class="act-btn act-restart" :disabled="isActing(group.name) || !startable || group.daemons.length === 0" @click="onRestart(group.name)">
            {{ group.is_default ? 'Restart stack' : 'Restart' }}
          </button>
        </div>
      </header>

      <p v-if="group.missing.length" class="group-missing">
        Not defined in this worktree's config: {{ group.missing.join(', ') }}
      </p>

      <DaemonTable
        v-if="group.daemons.length"
        :daemons="group.daemons"
        :prefers-card="prefersCard"
        :actions-disabled-reason="blockedReason"
        @refresh="emit('refresh')"
      />
    </section>

    <section v-if="stack.ungrouped.length" class="group">
      <header class="group-header">
        <div class="group-title">
          <h3 class="group-name">ungrouped</h3>
          <span class="group-count">{{ stack.ungrouped.length }} daemons</span>
        </div>
      </header>
      <DaemonTable
        :daemons="stack.ungrouped"
        :prefers-card="prefersCard"
        :actions-disabled-reason="blockedReason"
        @refresh="emit('refresh')"
      />
    </section>

    <div v-if="groups.length === 0 && stack.ungrouped.length === 0" class="empty-state">
      <h3>No daemons in this worktree</h3>
      <p>Declare daemons in pitchfork.toml, and groups under [groups] to control them together.</p>
    </div>
  </div>
</template>

<style scoped lang="less">
@import '@/styles/mixins.less';

.stack-groups { display: flex; flex-direction: column; gap: @space-xl; }

.group {
  .ns-group-surface();

  &.primary { border-color: @sf-accent-10; }
}

.group-header {
  .flex-between();
  gap: @space-md;
  padding: 0.6rem 0.9rem;
  flex-wrap: wrap;
}

.group-title { .flex-center(); gap: 0.5rem; }

.group-name { margin: 0; .font-sans(0.9rem; @sf-65; 600); }

.group-tag {
  .font-sans(0.65rem; @c-accent-dim; 600);
  background: @sf-accent-10;
  padding: 0.1rem 0.4rem;
  border-radius: @r-sm;
  text-transform: uppercase;
  letter-spacing: 0.04em;
}

.group-count {
  .font-sans(0.72rem; @sf-40; 500);
  background: @sf-5;
  padding: 0.12rem 0.45rem;
  border-radius: @r-sm;
  font-variant-numeric: tabular-nums;
}

.group-actions { display: flex; gap: 0.35rem; flex-wrap: wrap; }

.act-btn {
  .font-sans(0.75rem; @sf-65; 600);
  background: @sf-5;
  border: 1px solid @sf-8;
  border-radius: @r-md;
  padding: 0.3rem 0.7rem;
  cursor: pointer;
  transition: @tr-base;

  &:hover:not(:disabled) { background: @sf-8; color: @c-white; }
  &:disabled { opacity: 0.5; cursor: not-allowed; }
}

.act-start:hover:not(:disabled) { color: @c-success; }
.act-stop:hover:not(:disabled) { color: @c-danger; }

.unregistered {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: @space-md;
  flex-wrap: wrap;

  p {
    margin: 0;
    .font-sans(0.78rem; @c-warning; 500);
  }

  code { .font-mono(0.75rem; @sf-45); }
}

.group-missing {
  margin: 0;
  padding: 0.4rem 0.9rem 0.6rem;
  .font-sans(0.75rem; @c-warning; 500);
}

.empty-state { text-align: center; padding: 2rem @space-xl; border: 1px dashed rgba(255, 255, 255, 0.06); border-radius: @r-2xl; background: @sf-1; }
.empty-state h3 { margin: 0 0 0.2rem 0; font-size: 1rem; font-weight: 600; color: @sf-45; }
.empty-state p { margin: 0; font-size: 0.85rem; color: @sf-25; }

.mobile({
  .group-header { align-items: flex-start; flex-direction: column; }
});
</style>
