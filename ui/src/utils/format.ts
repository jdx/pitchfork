export function formatBytes(bytes: number): string {
  if (bytes === 0) return '0 B'
  const units = ['B', 'KB', 'MB', 'GB', 'TB']
  const i = Math.min(Math.floor(Math.log(bytes) / Math.log(1024)), units.length - 1)
  return `${(bytes / Math.pow(1024, i)).toFixed(1)} ${units[i]}`
}

export function formatUptime(secs: number | null): string {
  if (secs == null) return '—'
  if (secs < 60) return `${secs}s`
  if (secs < 3600) return `${Math.floor(secs / 60)}m`
  const h = Math.floor(secs / 3600)
  const m = Math.floor((secs % 3600) / 60)
  return `${h}h${m}m`
}

/** "3m ago" for an RFC 3339 timestamp; an em dash when unknown. */
export function formatSince(timestamp: string | null | undefined): string {
  if (!timestamp) return '—'
  const then = Date.parse(timestamp)
  if (Number.isNaN(then)) return '—'
  const secs = Math.max(0, Math.round((Date.now() - then) / 1000))
  return `${formatUptime(secs)} ago`
}
