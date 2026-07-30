export function errorText(error: unknown): string {
  if (typeof error === 'string') return error
  if (error instanceof Error) return error.message
  return String(error || '操作失败')
}

export function formatExpiry(timestamp: number | null): string {
  if (!timestamp) return ''
  const difference = timestamp * 1000 - Date.now()
  if (difference <= 0) return '已过期'
  const hours = Math.ceil(difference / 3_600_000)
  return hours < 48 ? `${hours} 小时后过期` : `${Math.ceil(hours / 24)} 天后过期`
}
