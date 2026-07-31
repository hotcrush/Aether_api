export type ChannelMonitorStatus = 'operational' | 'degraded' | 'failed' | 'error'
export type ChannelMonitorWindow = '24h' | '7d'

export interface ChannelMonitorEvent {
  id: number
  request_id: string
  attempt_index: number
  status: ChannelMonitorStatus
  http_status: number | null
  ttfb_ms: number | null
  duration_ms: number | null
  endpoint_family: string
  model: string
  source: string
  message: string
  estimated_cost: number | null
  created_at: number | string
}

export interface ChannelMonitorItem {
  account_id: string
  name: string
  account_type: 'oauth' | 'api_key'
  account_status: 'active' | 'disabled'
  latest_status: ChannelMonitorStatus | null
  latest_checked_at: number | string | null
  latest_ttfb_ms: number | null
  current_capacity: number
  concurrency: number
  availability_24h: number | null
  availability_7d: number | null
  avg_ttfb_24h_ms: number | null
  avg_ttfb_7d_ms: number | null
  attempts_24h: number
  attempts_7d: number
  failed_24h: number
  failed_7d: number
  estimated_cost_24h: number | null
  estimated_cost_7d: number | null
  timeline: ChannelMonitorEvent[]
}

export interface ChannelMonitorSnapshot {
  generated_at: number | string
  total_24h: number
  available_24h: number
  failed_24h: number
  availability_24h: number | null
  avg_ttfb_24h_ms: number | null
  active_channels: number
  abnormal_channels: number
  items: ChannelMonitorItem[]
}
