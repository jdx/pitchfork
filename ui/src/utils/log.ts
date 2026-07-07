export interface ParsedLogLine {
  timestamp: string | null
  raw: string
  html: string
}

// Strip the fixed `<ts> ` prefix prepended by the web log API
// (src/web/routes/api/logs.rs: `{ts} {msg}`); keep the message verbatim.
const LOG_PREFIX_RE = /^(\d{4}-\d{2}-\d{2} \d{2}:\d{2}:\d{2}) (.*)$/

// Match all CSI sequences (ESC[...X)
const CSI_PATTERN = /\x1b\[[\d;?]*[A-Za-z]/g

// Match color CSI sequences only (ESC[...m)
const COLOR_CSI = /\x1b\[([\d;]*)m/g

// Standard 16 colors
const COLORS_16: Record<number, string> = {
  0: '#000000',
  1: '#ef4444',
  2: '#30a46c',
  3: '#eab308',
  4: '#3b82f6',
  5: '#a855f7',
  6: '#06b6d4',
  7: '#f3f4f6',
  8: '#374151',
  9: '#fca5a5',
  10: '#86efac',
  11: '#fde047',
  12: '#93c5fd',
  13: '#d8b4fe',
  14: '#67e8f9',
  15: '#ffffff',
}

const ANSI_FG: Record<number, string> = {
  30: '#6b7280',
  31: '#ef4444',
  32: '#30a46c',
  33: '#eab308',
  34: '#3b82f6',
  35: '#a855f7',
  36: '#06b6d4',
  37: '#f3f4f6',
  90: '#4b5563',
  91: '#fca5a5',
  92: '#86efac',
  93: '#fde047',
  94: '#93c5fd',
  95: '#d8b4fe',
  96: '#67e8f9',
  97: '#ffffff',
}

const ANSI_BG: Record<number, string> = {
  40: '#1f2937',
  41: '#7f1d1d',
  42: '#064e3b',
  43: '#713f12',
  44: '#1e3a8a',
  45: '#581c87',
  46: '#164e63',
  47: '#374151',
  100: '#111827',
  101: '#450a0a',
  102: '#022c22',
  103: '#422006',
  104: '#172554',
  105: '#3b0764',
  106: '#083344',
  107: '#1f2937',
}

function color256(n: number): string {
  if (n < 16) return COLORS_16[n] ?? '#ffffff'
  if (n < 232) {
    const idx = n - 16
    const r = Math.floor(idx / 36)
    const g = Math.floor((idx % 36) / 6)
    const b = idx % 6
    const values = [0, 95, 135, 175, 215, 255]
    return `rgb(${values[r]},${values[g]},${values[b]})`
  }
  const gray = 8 + (n - 232) * 10
  return `rgb(${gray},${gray},${gray})`
}

function escapeHtml(text: string): string {
  return text
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;')
}

function preprocess(text: string): string {
  // Remove CR so lines don't overlap in HTML
  text = text.replace(/\r/g, '')
  // Remove BEL
  text = text.replace(/\x07/g, '')
  // Remove backspace
  text = text.replace(/\x08/g, '')
  // Strip non-color CSI sequences (cursor move, clear screen, etc.)
  text = text.replace(CSI_PATTERN, (match) => {
    if (match.endsWith('m')) return match
    return ''
  })
  return text
}

function parseColorParams(params: number[]): {
  reset?: boolean
  fg?: string
  bg?: string
  bold?: boolean
  dim?: boolean
} {
  const result: {
    reset?: boolean
    fg?: string
    bg?: string
    bold?: boolean
    dim?: boolean
  } = {}

  for (let i = 0; i < params.length; i++) {
    const p = params[i]
    if (p === 0) {
      result.reset = true
    } else if (p === 1) {
      result.bold = true
    } else if (p === 2) {
      result.dim = true
    } else if (p === 22) {
      result.bold = false
      result.dim = false
    } else if (p === 39) {
      // Default foreground — clear explicit color so it inherits from parent
      result.fg = ''
    } else if (p === 49) {
      result.bg = ''
    } else if (ANSI_FG[p]) {
      result.fg = ANSI_FG[p]
    } else if (ANSI_BG[p]) {
      result.bg = ANSI_BG[p]
    } else if (p === 38 && i + 2 < params.length && params[i + 1] === 5) {
      result.fg = color256(params[i + 2])
      i += 2
    } else if (p === 38 && i + 4 < params.length && params[i + 1] === 2) {
      result.fg = `rgb(${params[i + 2]},${params[i + 3]},${params[i + 4]})`
      i += 4
    } else if (p === 48 && i + 2 < params.length && params[i + 1] === 5) {
      result.bg = color256(params[i + 2])
      i += 2
    } else if (p === 48 && i + 4 < params.length && params[i + 1] === 2) {
      result.bg = `rgb(${params[i + 2]},${params[i + 3]},${params[i + 4]})`
      i += 4
    }
  }
  return result
}

interface ColorState {
  fg: string
  bg: string
  bold: boolean
  dim: boolean
}

function ansiToHtml(text: string): string {
  let html = ''
  let lastIndex = 0
  let state: ColorState = { fg: '', bg: '', bold: false, dim: false }

  let match: RegExpExecArray | null
  while ((match = COLOR_CSI.exec(text)) !== null) {
    const segment = text.slice(lastIndex, match.index)
    if (segment) html += renderSegment(segment, state)

    const params = match[1].split(';').map((s) => {
      const n = Number(s)
      return Number.isNaN(n) ? 0 : n
    })
    const changes = parseColorParams(params)

    if (changes.reset) {
      state = { fg: '', bg: '', bold: false, dim: false }
    } else {
      if (changes.fg !== undefined) state.fg = changes.fg
      if (changes.bg !== undefined) state.bg = changes.bg
      if (changes.bold !== undefined) state.bold = changes.bold
      if (changes.dim !== undefined) state.dim = changes.dim
    }

    lastIndex = COLOR_CSI.lastIndex
  }

  const remaining = text.slice(lastIndex)
  if (remaining) html += renderSegment(remaining, state)

  return html
}

function renderSegment(text: string, state: ColorState): string {
  const styles: string[] = []
  if (state.fg) styles.push(`color:${state.fg}`)
  else styles.push('color:rgba(255,255,255,0.85)')
  if (state.bg) styles.push(`background-color:${state.bg}`)
  if (state.bold) styles.push('font-weight:700')
  if (state.dim) styles.push('opacity:0.5')
  return `<span style="${styles.join(';')}">${escapeHtml(text)}</span>`
}

export function parseLogLines(lines: string[]): ParsedLogLine[] {
  return lines.map((line) => parseLogLine(line))
}

export function parseLogLine(line: string): ParsedLogLine {
  const match = line.match(LOG_PREFIX_RE)
  if (match) {
    const [, timestamp, content] = match
    const cleaned = preprocess(content)
    return {
      timestamp,
      raw: cleaned.replace(COLOR_CSI, ''),
      html: ansiToHtml(cleaned),
    }
  }
  const cleaned = preprocess(line)
  return {
    timestamp: null,
    raw: cleaned.replace(COLOR_CSI, ''),
    html: ansiToHtml(cleaned),
  }
}
