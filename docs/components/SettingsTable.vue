<!-- docs/components/SettingsTable.vue -->
<!-- Displays settings from settings.toml with automatic documentation -->
<script setup lang="ts">
import { data } from '../settings.data'

interface FlatSetting {
  name: string
  section: string
  type: string
  default: string
  env: string
  description: string
  docsHtml: string
}

const { grouped, sections } = data as {
  grouped: Record<string, FlatSetting[]>
  sections: string[]
}

// Format section names for display
function formatSection(section: string): string {
  const names: Record<string, string> = {
    general: 'General Settings',
    ipc: 'IPC Settings',
    web: 'Web UI Settings',
    tui: 'TUI Settings',
    supervisor: 'Supervisor Settings',
  }
  return names[section] || section.charAt(0).toUpperCase() + section.slice(1)
}

// Format type names for display
function formatType(type: string): string {
  const names: Record<string, string> = {
    Bool: 'Boolean',
    Integer: 'Number',
    String: 'String',
    Duration: 'Duration',
    Path: 'Path',
  }
  return names[type] || type
}

// Format default values for display
function formatDefault(value: string, type: string): string {
  if (!value) return 'none'
  // Remove outer quotes for string types
  if (type === 'String' || type === 'Duration') {
    return value.replace(/^"|"$/g, '')
  }
  return value
}
</script>

<template>
  <div class="settings-reference">
    <div v-for="section in sections" :key="section" class="settings-section">
      <h3 :id="section">{{ formatSection(section) }}</h3>
      
      <div v-for="setting in grouped[section]" :key="setting.name" class="setting-item">
        <div class="setting-header">
          <h4 :id="`${section}-${setting.name}`" class="setting-heading">{{ section }}.{{ setting.name }}</h4>
          <span class="type-badge">{{ formatType(setting.type) }}</span>
        </div>
        
        <div class="setting-meta" v-if="setting.env || setting.default">
          <div class="meta-row" v-if="setting.env">
            <span class="meta-label">Environment Variable:</span>
            <code>{{ setting.env }}</code>
          </div>
          <div class="meta-row" v-if="setting.default">
            <span class="meta-label">Default:</span>
            <code>{{ formatDefault(setting.default, setting.type) }}</code>
          </div>
        </div>

        <p class="setting-description">{{ setting.description }}</p>

        <div v-if="setting.docsHtml" v-html="setting.docsHtml" class="setting-docs"></div>
      </div>
    </div>
  </div>
</template>

<style scoped>
.settings-reference {
  max-width: 100%;
}

.settings-section {
  margin-bottom: 3rem;
}

.settings-section h3 {
  border-bottom: 1px solid var(--vp-c-divider);
  padding-bottom: 0.5rem;
  margin-bottom: 1.5rem;
}

.setting-item {
  margin-bottom: 2rem;
  padding-bottom: 1.5rem;
  border-bottom: 1px solid var(--vp-c-divider-light);
}

.setting-item:last-child {
  border-bottom: none;
}

.setting-header {
  display: flex;
  align-items: center;
  gap: 0.75rem;
  margin-bottom: 0.75rem;
  flex-wrap: wrap;
}

.setting-heading {
  font-size: 1rem;
  font-weight: 600;
  margin: 0;
  padding: 0;
  border: none;
  color: var(--vp-c-brand);
  font-family: var(--vp-font-family-mono);
}

.type-badge {
  font-size: 0.75em;
  font-weight: 500;
  background-color: var(--vp-c-brand-soft);
  color: var(--vp-c-brand-dark);
  padding: 2px 8px;
  border-radius: 4px;
}

.setting-meta {
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
  margin-bottom: 0.25rem;
}

.meta-row:last-child {
  margin-bottom: 0;
}

.meta-label {
  font-weight: 500;
  color: var(--vp-c-text-2);
  min-width: 160px;
}

.setting-description {
  color: var(--vp-c-text-1);
  margin: 0.75rem 0;
}

.setting-docs {
  color: var(--vp-c-text-2);
  font-size: 0.95em;
  line-height: 1.7;
}

.setting-docs :deep(code) {
  background-color: var(--vp-c-bg-soft);
  padding: 2px 6px;
  border-radius: 4px;
  font-size: 0.9em;
}

.setting-docs :deep(pre) {
  background-color: var(--vp-c-bg-soft);
  padding: 1rem;
  border-radius: 6px;
  margin: 0.75rem 0;
  overflow-x: auto;
}

.setting-docs :deep(ul) {
  padding-left: 1.25rem;
  margin: 0.5rem 0;
}

.setting-docs :deep(li) {
  margin-bottom: 0.25rem;
}

.setting-docs :deep(strong) {
  color: var(--vp-c-text-1);
}
</style>
