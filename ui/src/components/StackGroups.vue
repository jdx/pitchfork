<script setup lang="ts">
import { computed } from 'vue'
import DaemonTable from './DaemonTable.vue'
import { useGroupActions } from '@/composables/useApi'
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
const unresolvable = computed(() => props.stack.unresolvable_daemons)

const NO_CONFIG =
  'The supervisor has no config for this daemon, so it cannot be started or '
  + 'restarted. Register this worktree first.'
const NO_DIRECTORY =
  'This worktree directory no longer exists, so its daemons cannot be started or '
  + 'restarted.'

/** Every daemon the page renders, grouped or not. */
const renderedDaemons = computed(() => [
  ...props.stack.groups.flatMap(g => g.daemons),
  ...props.stack.ungrouped,
])

/**
 * Why each daemon cannot be started or restarted, keyed by qualified id.
 *
 * A missing directory only blocks the daemons that would run from it, which
 * are the ones in this worktree's namespace. A group inherited from a wider
 * config can name daemons of other namespaces, and those stay usable.
 */
const blockedReasons = computed(() => {
  const reasons: Record<string, string> = {}
  for (const id of unresolvable.value) reasons[id] = NO_CONFIG
  if (!props.stack.dir_exists && props.stack.namespace) {
    // Namespaces compare case-insensitively, as they do on the API side.
    const ns = props.stack.namespace.toLowerCase()
    for (const d of renderedDaemons.value) {
      if (d.id.namespace.toLowerCase() === ns) reasons[d.id.qualified] = NO_DIRECTORY
    }
  }
  return reasons
})

// Only the members the supervisor cannot resolve lose their group action; a
// group of resolvable daemons stays usable even when the stack has others.
// This gates Start and Restart only: stopping works from the daemon's tracked
// state and needs no config, so a running stack can always be taken down.
function groupBlocked(groupName: string): boolean {
  return ids(groupName).some(id => id in blockedReasons.value)
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
    <p v-if="!stack.dir_exists" class="notice">
      <code>{{ stack.dir }}</code> no longer exists. Remove the
      <code>[namespaces]</code> entry from your user config, or restore the directory.
    </p>

    <p v-else-if="stack.config_error" class="notice">
      This worktree's config could not be read, so its groups are missing:
      <code>{{ stack.config_error }}</code>
    </p>

    <p v-if="!stack.namespace" class="notice">
      No namespace could be derived for this worktree, so no daemons are attributed to
      it: {{ stack.namespace_error }}. Set <code>namespace</code> in a
      <code>pitchfork.toml</code> here to give it one.
    </p>

    <p v-else-if="stack.dir_exists && unresolvable.length" class="notice">
      The supervisor has no config for
      <code>{{ unresolvable.join(', ') }}</code>, so starting or restarting them would
      fail; stopping still works. Register <code>{{ stack.dir }}</code> as namespace
      <code>{{ stack.namespace }}</code> under <code>[namespaces]</code> in your user
      config, or run <code>pitchfork proxy add</code> from it, then reload.
    </p>

    <section v-for="group in groups" :key="group.name" class="group" :class="{ primary: group.is_default }">
      <header class="group-header">
        <div class="group-title">
          <h3 class="group-name">{{ group.name }}</h3>
          <span v-if="group.is_default" class="group-tag">stack</span>
          <span class="group-count">{{ group.running }}/{{ group.total }} running</span>
        </div>
        <div class="group-actions">
          <button class="act-btn act-start" :disabled="isActing(group.name) || groupBlocked(group.name) || group.daemons.length === 0" @click="onStart(group.name)">
            {{ group.is_default ? 'Start stack' : 'Start' }}
          </button>
          <button class="act-btn act-stop" :disabled="isActing(group.name) || group.daemons.length === 0" @click="onStop(group.name)">
            {{ group.is_default ? 'Stop stack' : 'Stop' }}
          </button>
          <button class="act-btn act-restart" :disabled="isActing(group.name) || groupBlocked(group.name) || group.daemons.length === 0" @click="onRestart(group.name)">
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
        :disabled-reasons="blockedReasons"
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
        :disabled-reasons="blockedReasons"
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

.notice {
  margin: 0;
  .font-sans(0.78rem; @c-warning; 500);

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
