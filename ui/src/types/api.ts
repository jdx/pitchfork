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
  | { type: 'completed' }
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
  health_cmd: string | null
  health_http_url: string | null
  health_port: number | null
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
  /** Project directory the namespace resolves daemons from. */
  dir: string
}

export interface NamespaceRegistration {
  name: string
  dir: string
}

export interface DaemonLogLine {
  line: string
  timestamp: string | null
}

export interface StructuredLogEntry {
  id?: number
  timestamp: string
  daemon_id: string
  message: string
  level?: string
  msg?: string
  logger?: string
  fields?: Record<string, unknown>
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

export interface DaemonCounts {
  total: number
  running: number
  stopped: number
  /** Oneshot daemons that ran and exited successfully. */
  completed: number
  /** On the way up or down: waiting or stopping. */
  transitioning: number
  failed: number
  available: number
}

export interface ProjectSummary {
  name: string
  dir: string
  worktree_count: number
  /** False when the registered directory no longer exists. */
  dir_exists: boolean
  daemons: DaemonCounts
  last_activity: string | null
  url: string
  api_url: string
}

export interface WorktreeSummary {
  name: string
  branch: string
  path: string
  /** Null when no namespace can be derived for this worktree. */
  namespace: string | null
  /** Why no namespace could be derived, when none could. */
  namespace_error?: string
  is_primary: boolean
  /** False when the supervisor cannot resolve config for some of its daemons. */
  can_start: boolean
  /** False when the worktree directory no longer exists. */
  dir_exists: boolean
  group_count: number
  daemons: DaemonCounts
  last_activity: string | null
  url: string
  api_url: string
  /** Absent when pitchfork does not track data directories for the daemons. */
  disk_usage_bytes?: number
}

export interface StackGroup {
  name: string
  is_default: boolean
  daemons: DaemonEntry[]
  missing: string[]
  running: number
  total: number
}

export interface Stack {
  project: string
  worktree: string
  branch: string
  /** Null when no namespace can be derived for this worktree. */
  namespace: string | null
  /** Why no namespace could be derived, when none could. */
  namespace_error?: string
  dir: string
  is_primary: boolean
  /** False when `unresolvable_daemons` is non-empty. */
  can_start: boolean
  /** Listed daemons the supervisor has no config for. */
  unresolvable_daemons: string[]
  /** False when the worktree directory no longer exists. */
  dir_exists: boolean
  /** Why the worktree's config could not be read, when it could not be. */
  config_error?: string
  groups: StackGroup[]
  ungrouped: DaemonEntry[]
  daemons: DaemonCounts
  url: string
}

export interface Project {
  name: string
  dir: string
  dir_exists: boolean
  daemons: DaemonCounts
  last_activity: string | null
  worktrees: WorktreeSummary[]
  stack?: Stack
}
