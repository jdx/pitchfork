export interface DaemonId {
  namespace: string
  name: string
  qualified: string
}

export type DaemonStatus =
  | { type: 'failed'; message: string }
  | { type: 'waiting' }
  | { type: 'running' }
  | { type: 'stopping' }
  | { type: 'errored'; code: number }
  | { type: 'stopped' }
  | { type: 'available' }

export interface DaemonEntry {
  id: DaemonId
  status: DaemonStatus
  is_available: boolean
  pid: number | null
  shell_pid: number | null
  uptime_secs: number | null
  active_port: number | null
  resolved_port: number[]
  slug: string | null
  autostop: boolean | null
  retry_count: number
  is_disabled: boolean | null
  cpu_percent: number | null
  memory_bytes: number | null
  memory_limit: string | null
  cpu_limit: string | null
  stop_signal: string | null
  stop_timeout: number | null
  restart_policy: string | null
  restart_count: number
  port_config: string | null
  watch: string[]
  watch_mode: string | null
  ready_delay: number | null
  ready_output: string | null
  ready_http_url: string | null
  ready_port: number | null
  ready_cmd: string | null
  proxy_url: string | null
  pty: boolean | null
  proxy: boolean | null
  depends: string[]
  env: string[] | null
  cron_schedule: string | null
  command: string | null
  dir: string | null
  mise: boolean | null
  user: string | null
}

export interface NamespaceEntry {
  name: string
  daemon_count: number
  is_active: boolean
}

export interface NamespaceRegistration {
  name: string
  dir: string
}

export interface DaemonLogLine {
  line: string
  timestamp: string | null
}

export interface DaemonStats {
  cpu: number | null
  memory: number | null
  pid: number | null
  port: number | null
}

export interface ProxyWorktreeEntry {
  slug: string
  daemon_name: string
  branch: string
  sanitized_branch: string
  namespace: string | null
  path: string
  port: number | null
  status: string | null
  pid: number | null
  proxy_url: string | null
  daemon_qualified: string
  uptime_secs: number | null
}

export interface ProcessTree {
  pid: number
  name: string
  exe: string | null
  cpu_percent: number
  memory_bytes: number
  rss_bytes: number
  thread_count: number
  status: string
  children: ProcessTree[]
}
