import { ref, shallowRef, watchEffect, type Ref } from 'vue'
import { toast } from 'vue-sonner'
import type { DaemonEntry, DaemonStats, ProcessTree } from '@/types/api'

const API_BASE = (() => {
  const base = (window as any).__PITCHFORK_BASE__ as string | undefined
  if (base && base !== '__PF_BASE_PLACEHOLDER__') {
    const prefix = base.endsWith('/') ? base.slice(0, -1) : base
    return `${prefix}/api`
  }
  return '/api'
})()
function getAuthHeaders(): Record<string, string> {
  const token = (window as any).__PITCHFORK_TOKEN__ as string | undefined
  const headers: Record<string, string> = {}
  if (token && token !== '__PF_TOKEN_PLACEHOLDER__') {
    headers['X-Pitchfork-Token'] = token
  }
  return headers
}

export async function api<T>(path: string, init?: RequestInit): Promise<T> {
  const res = await fetch(`${API_BASE}${path}`, {
    ...init,
    headers: {
      ...getAuthHeaders(),
      ...(init?.headers as Record<string, string> ?? {}),
    },
  })
  const data = await res.json().catch(() => null)
  if (!res.ok || (data && typeof data === 'object' && 'ok' in data && data.ok === false)) {
    const msg = data && typeof data === 'object' && 'error' in data
      ? String(data.error)
      : `HTTP ${res.status}`
    throw new Error(msg)
  }
  return data as T
}

export function useDaemons(pollInterval = 3000) {
  const daemons = shallowRef<DaemonEntry[]>([])
  const loading = ref(true)
  const error = ref<string | null>(null)
  let timer: ReturnType<typeof setInterval> | null = null

  async function fetchDaemons() {
    try {
      error.value = null
      const data: DaemonEntry[] = await api('/daemons')
      daemons.value = data
    } catch (e: any) {
      error.value = e.message ?? 'Unknown error'
    } finally {
      loading.value = false
    }
  }

  function startPolling() {
    if (timer) return
    fetchDaemons()
    timer = setInterval(fetchDaemons, pollInterval)
  }
  function stopPolling() {
    if (timer) {
      clearInterval(timer)
      timer = null
    }
  }

  return {
    daemons,
    loading,
    error,
    refresh: fetchDaemons,
    startPolling,
    stopPolling,
  }
}

export function useDaemon(id: Ref<string>, pollInterval = 3000) {
  const daemon = shallowRef<DaemonEntry | null>(null)
  const loading = ref(true)
  const error = ref<string | null>(null)
  let nonce = 0

  async function fetchOne() {
    const current = ++nonce
    try {
      error.value = null
      const d: DaemonEntry = await api(`/daemons/${encodeURIComponent(id.value)}`)
      if (current !== nonce) return
      daemon.value = d
    } catch (e: any) {
      if (current !== nonce) return
      error.value = e.message ?? 'Unknown error'
    } finally {
      if (current === nonce) loading.value = false
    }
  }

  watchEffect((onCleanup) => {
    if (!id.value) return
    fetchOne()
    const interval = setInterval(fetchOne, pollInterval)
    onCleanup(() => clearInterval(interval))
  })

  return { daemon, loading, error, refresh: fetchOne }
}

function daemonName(id: string): string {
  return id.split('.').pop() ?? id
}

async function toastAction(
  name: string,
  verb: string,
  action: () => Promise<unknown>,
) {
  const toastId = toast.loading(`${verb} ${name}...`)
  try {
    await action()
    toast.dismiss(toastId)
    toast.success(`${name} ${verb.toLowerCase()}ed`, { duration: 2000 })
  } catch (e: any) {
    toast.dismiss(toastId)
    toast.error(`${verb} ${name} failed`, { duration: 4000, description: e.message ?? 'unknown error' })
    throw e
  }
}

export function useDaemonActions() {
  const acting = ref<Set<string>>(new Set())

  function wrap(name: string, action: () => Promise<unknown>): () => Promise<void> {
    return async () => {
      if (acting.value.has(name)) return
      acting.value = new Set(acting.value).add(name)
      try {
        await action()
      } finally {
        const next = new Set(acting.value)
        next.delete(name)
        acting.value = next
      }
    }
  }

  function start(id: string) {
    return toastAction(daemonName(id), 'Start', wrap(id, () =>
      api(`/daemons/${encodeURIComponent(id)}/start`, { method: 'POST' }),
    ))
  }
  function stop(id: string) {
    return toastAction(daemonName(id), 'Stop', wrap(id, () =>
      api(`/daemons/${encodeURIComponent(id)}/stop`, { method: 'POST' }),
    ))
  }
  function restart(id: string) {
    return toastAction(daemonName(id), 'Restart', wrap(id, () =>
      api(`/daemons/${encodeURIComponent(id)}/restart`, { method: 'POST' }),
    ))
  }
  function enable(id: string) {
    return toastAction(daemonName(id), 'Enable', wrap(id, () =>
      api(`/daemons/${encodeURIComponent(id)}/enable`, { method: 'POST' }),
    ))
  }
  function disable(id: string) {
    return toastAction(daemonName(id), 'Disable', wrap(id, () =>
      api(`/daemons/${encodeURIComponent(id)}/disable`, { method: 'POST' }),
    ))
  }
  return { start, stop, restart, enable, disable, acting }
}

export function useLogStream(id: Ref<string>) {
  const lines = ref<string[]>([])
  const error = ref<string | null>(null)
  const connected = ref(false)
  let abort: AbortController | null = null

  async function connect() {
    lines.value = []
    error.value = null
    abort = new AbortController()

    try {
      const res = await fetch(
        `${API_BASE}/logs/${encodeURIComponent(id.value)}/tail`,
        { signal: abort.signal, headers: getAuthHeaders() },
      )
      if (!res.ok || !res.body) {
        error.value = `HTTP ${res.status}`
        connected.value = false
        return
      }
      connected.value = true
      const reader = res.body.getReader()
      const decoder = new TextDecoder()
      let buf = ''

      while (true) {
        const { done, value } = await reader.read()
        if (done) break
        buf += decoder.decode(value, { stream: true })
        const parts = buf.split('\n')
        buf = parts.pop() ?? ''
        lines.value.push(...parts)
      }

      if (buf) {
        lines.value.push(buf)
      }
    } catch (e: any) {
      if (e.name !== 'AbortError') {
        error.value = e.message ?? 'Stream error'
      }
    } finally {
      connected.value = false
    }
  }

  watchEffect((onCleanup) => {
    connect()
    onCleanup(() => {
      abort?.abort()
    })
  })

  return { lines, error, connected }
}

export function useStats(pollInterval = 3000) {
  const stats = shallowRef<DaemonStats | null>(null)
  let timer: ReturnType<typeof setInterval> | null = null

  async function fetchStats() {
    try {
      stats.value = await api('/stats')
    } catch {
      // ignore
    }
  }

  function startPolling() {
    if (timer) return
    fetchStats()
    timer = setInterval(fetchStats, pollInterval)
  }
  function stopPolling() {
    if (timer) {
      clearInterval(timer)
      timer = null
    }
  }

  return { stats, startPolling, stopPolling }
}

export interface NamespaceEntry {
  name: string
  dir: string
}

export function useNamespaces() {
  const namespaces = shallowRef<NamespaceEntry[]>([])
  const loading = ref(false)
  const error = ref<string | null>(null)

  async function fetchNamespaces() {
    try {
      error.value = null
      namespaces.value = await api('/namespaces')
    } catch (e: any) {
      error.value = e.message ?? 'Unknown error'
    }
  }

  async function register(dir: string) {
    const data = await api<{ ok: boolean; name?: string; error?: string }>('/namespaces', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ dir }),
    })
    if (!data.ok) {
      throw new Error(data.error || 'Failed to register namespace')
    }
    await fetchNamespaces()
    return data.name as string
  }

  async function remove(name: string) {
    await api(`/namespaces/${encodeURIComponent(name)}`, { method: 'DELETE' })
    await fetchNamespaces()
  }

  fetchNamespaces()

  return { namespaces, loading, error, refresh: fetchNamespaces, register, remove }
}

export function useProcessTree(id: Ref<string>, pollInterval = 3000) {
  const tree = shallowRef<ProcessTree[]>([])
  const loading = ref(false)
  const error = ref<string | null>(null)

  async function fetchTree() {
    try {
      loading.value = true
      error.value = null
      tree.value = await api<ProcessTree[]>(`/processes/${encodeURIComponent(id.value)}/tree`)
    } catch (e: any) {
      error.value = e.message ?? 'Unknown error'
    } finally {
      loading.value = false
    }
  }

  watchEffect((onCleanup) => {
    if (!id.value) return
    fetchTree()
    const interval = setInterval(fetchTree, pollInterval)
    onCleanup(() => clearInterval(interval))
  })

  return { tree, loading, error, refresh: fetchTree }
}
