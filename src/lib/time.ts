/**
 * Unified time parsing and formatting utilities.
 *
 * Backend convention: all timestamps are stored as UTC ISO 8601
 *   - Second precision: `2026-08-01T12:34:56Z`
 *   - Millis precision:  `2026-08-01T12:34:56.789Z`
 *   - Unix seconds (i64) for `expires_at`
 *
 * Display convention (zh-CN, 24-hour):
 *   - formatTime:       HH:mm:ss
 *   - formatShortTime:  HH:mm
 *   - formatDateTime:   MM/DD HH:mm
 *   - formatFullDate:   YYYY/MM/DD HH:mm:ss
 *   - formatRelative:   刚刚 / X秒前 / X分钟前 / X小时前 / X天前
 */

// ── Parsing ────────────────────────────────────────────────────────

/**
 * Parse a timestamp value into a Date.
 * Accepts unix seconds (< 1e12), unix millis (>= 1e12), ISO 8601 strings,
 * and legacy `YYYY-MM-DD HH:mm:ss` local-time strings.
 * Returns null for invalid / empty input.
 */
export function parseDate(value: number | string | null | undefined): Date | null {
  if (value === null || value === undefined || value === '') return null
  const numeric = typeof value === 'number' ? value : Number(value)
  const date = Number.isFinite(numeric)
    ? new Date(numeric < 1_000_000_000_000 ? numeric * 1000 : numeric)
    : new Date(value)
  return Number.isNaN(date.getTime()) ? null : date
}

// ── Absolute formatting ────────────────────────────────────────────

/** HH:mm:ss — log entries, monitor event timestamps. */
export function formatTime(value: number | string | null | undefined): string {
  return parseDate(value)?.toLocaleTimeString('zh-CN', {
    hour: '2-digit',
    minute: '2-digit',
    second: '2-digit',
    hour12: false,
  }) ?? '未知'
}

/** HH:mm — tooltips, compact metadata. */
export function formatShortTime(value: number | string | null | undefined): string {
  return parseDate(value)?.toLocaleTimeString('zh-CN', {
    hour: '2-digit',
    minute: '2-digit',
    hour12: false,
  }) ?? '未知'
}

/** MM/DD HH:mm — market timestamps, general date-time display. */
export function formatDateTime(value: number | string | null | undefined): string {
  return parseDate(value)?.toLocaleString('zh-CN', {
    month: '2-digit',
    day: '2-digit',
    hour: '2-digit',
    minute: '2-digit',
    hour12: false,
  }) ?? '未知'
}

/** YYYY/MM/DD HH:mm:ss — full precision display. */
export function formatFullDate(value: number | string | null | undefined): string {
  return parseDate(value)?.toLocaleString('zh-CN', {
    year: 'numeric',
    month: '2-digit',
    day: '2-digit',
    hour: '2-digit',
    minute: '2-digit',
    second: '2-digit',
    hour12: false,
  }) ?? '未知'
}

/** MM/DD — chart axis labels. */
export function formatShortDate(value: number | string | null | undefined): string {
  return parseDate(value)?.toLocaleDateString('zh-CN', {
    month: '2-digit',
    day: '2-digit',
  }) ?? '-'
}

// ── Relative formatting ────────────────────────────────────────────

/** 刚刚 / X秒前 / X分钟前 / X小时前 / X天前 */
export function formatRelativeTime(value: number | string | null | undefined): string {
  const date = parseDate(value)
  if (!date) return '时间未知'
  const seconds = Math.max(0, Math.floor((Date.now() - date.getTime()) / 1000))
  if (seconds < 10) return '刚刚'
  if (seconds < 60) return `${seconds} 秒前`
  const minutes = Math.floor(seconds / 60)
  if (minutes < 60) return `${minutes} 分钟前`
  const hours = Math.floor(minutes / 60)
  if (hours < 24) return `${hours} 小时前`
  return `${Math.floor(hours / 24)} 天前`
}

// ── Domain-specific ────────────────────────────────────────────────

/**
 * Account expiry countdown from a unix-seconds timestamp.
 * Returns '' for null, '已过期' for past, 'X 小时后过期' or 'X 天后过期'.
 */
export function formatExpiry(timestamp: number | null): string {
  if (!timestamp) return ''
  const difference = timestamp * 1000 - Date.now()
  if (difference <= 0) return '已过期'
  const hours = Math.ceil(difference / 3_600_000)
  return hours < 48 ? `${hours} 小时后过期` : `${Math.ceil(hours / 24)} 天后过期`
}

/**
 * Structured log timestamp: splits into clock / date / full parts
 * for multi-column log display.
 */
export function parseLogTime(value: string): { clock: string; date: string; full: string } {
  const date = parseDate(value)
  if (!date) {
    return { clock: '—', date: value || '未知时间', full: value || '未知时间' }
  }
  const ts = date.getTime()
  return {
    clock: formatTime(ts),
    date: formatShortDate(ts),
    full: formatFullDate(ts),
  }
}
