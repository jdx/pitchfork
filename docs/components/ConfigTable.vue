<!-- docs/components/ConfigTable.vue -->
<!-- Displays daemon configuration options from config.toml -->
<script setup lang="ts">
import { data } from '../config.data'

interface FlatOption {
  name: string
  type: string
  required: boolean
  default: string
  description: string
  docsHtml: string
  example: string
}

const { options } = data as {
  options: FlatOption[]
}

// Format type names for display
function formatType(type: string): string {
  const types: Record<string, string> = {
    String: 'String',
    Path: 'Path',
    Map: 'Map',
    Array: 'Array',
    Boolean: 'Boolean',
    Integer: 'Number',
    'Integer|Boolean': 'Number | Boolean',
    URL: 'URL',
    Object: 'Object',
  }
  return types[type] || type
}
</script>

<template>
  <div class="config-reference">
    <div v-for="option in options" :key="option.name" class="config-item">
      <div class="config-header">
        <h3 :id="option.name" class="config-heading">{{ option.name }}</h3>
        <span class="type-badge">{{ formatType(option.type) }}</span>
        <span v-if="option.required" class="required-badge">Required</span>
      </div>
      
      <div class="config-meta" v-if="option.default">
        <div class="meta-row">
          <span class="meta-label">Default:</span>
          <code>{{ option.default }}</code>
        </div>
      </div>

      <p class="config-description">{{ option.description }}</p>

      <div v-if="option.docsHtml" v-html="option.docsHtml" class="config-docs"></div>
    </div>
  </div>
</template>

<style scoped>
.config-reference {
  max-width: 100%;
}

.config-item {
  margin-bottom: 2rem;
  padding-bottom: 1.5rem;
  border-bottom: 1px solid var(--vp-c-divider-light);
}

.config-item:last-child {
  border-bottom: none;
}

.config-header {
  display: flex;
  align-items: center;
  gap: 0.75rem;
  margin-bottom: 0.75rem;
  flex-wrap: wrap;
}

.config-heading {
  font-size: 1rem;
  font-weight: 600;
  margin: 0;
  padding: 0;
  border: none;
  color: var(--vp-c-brand);
  font-family: var(--vp-font-family-mono);
}

.config-heading code {
  font-size: 1.1em;
  color: var(--vp-c-brand);
}

.type-badge {
  font-size: 0.75em;
  font-weight: 500;
  background-color: var(--vp-c-brand-soft);
  color: var(--vp-c-brand-dark);
  padding: 2px 8px;
  border-radius: 4px;
}

.required-badge {
  font-size: 0.7em;
  font-weight: 600;
  background-color: var(--vp-c-danger-soft);
  color: var(--vp-c-danger-1);
  padding: 2px 8px;
  border-radius: 4px;
}

.config-meta {
  margin: 0.75rem 0;
  padding: 0.75rem;
  background-color: var(--vp-c-bg-soft);
  border-radius: 6px;
  font-size: 0.9em;
}

.meta-row {
  display: flex;
  align-items: center;
  gap: 0.5rem;
}

.meta-label {
  font-weight: 500;
  color: var(--vp-c-text-2);
  min-width: 80px;
}

.config-description {
  color: var(--vp-c-text-1);
  margin: 0.75rem 0;
}

.config-docs {
  color: var(--vp-c-text-2);
  font-size: 0.95em;
  line-height: 1.7;
}

.config-docs :deep(code) {
  background-color: var(--vp-c-bg-soft);
  padding: 2px 6px;
  border-radius: 4px;
  font-size: 0.9em;
}

.config-docs :deep(pre) {
  background-color: var(--vp-c-bg-soft);
  padding: 1rem;
  border-radius: 6px;
  margin: 0.75rem 0;
  overflow-x: auto;
}

.config-docs :deep(ul) {
  padding-left: 1.25rem;
  margin: 0.5rem 0;
}

.config-docs :deep(li) {
  margin-bottom: 0.25rem;
}

.config-docs :deep(strong) {
  color: var(--vp-c-text-1);
}

.config-docs :deep(a) {
  color: var(--vp-c-brand);
  text-decoration: none;
}

.config-docs :deep(a:hover) {
  text-decoration: underline;
}
</style>
