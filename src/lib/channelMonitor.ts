import { invoke } from '@tauri-apps/api/core'
import type {
  ChannelMonitorEvent,
  ChannelMonitorItem,
  ChannelMonitorSnapshot,
  ChannelMonitorStatus,
  ModelIntegrityResult,
} from '../monitorTypes'

const previewMode = import.meta.env.DEV
  && typeof window !== 'undefined'
  && new URLSearchParams(window.location.search).has('preview')

export async function getChannelMonitorSnapshot(): Promise<ChannelMonitorSnapshot> {
  if (previewMode) {
    await previewWait(240)
    return previewSnapshot()
  }
  return invoke<ChannelMonitorSnapshot>('get_channel_monitor_snapshot')
}

export async function probeChannel(accountId: string): Promise<string> {
  if (previewMode) {
    await previewWait(420)
    return `连接正常：${accountId}`
  }
  return invoke<string>('probe_channel', { accountId })
}

export async function probeModelIntegrity(
  accountId: string,
  model: string,
): Promise<ModelIntegrityResult> {
  if (previewMode) {
    await previewWait(760)
    return previewIntegrity(accountId, model)
  }
  return invoke<ModelIntegrityResult>('probe_model_integrity', { accountId, model })
}

export async function listModelIntegrityHistory(
  accountId: string,
  limit = 10,
): Promise<ModelIntegrityResult[]> {
  if (previewMode) return [previewIntegrity(accountId, 'gpt-5')]
  return invoke<ModelIntegrityResult[]>('list_model_integrity_history', { accountId, limit })
}

function previewWait(duration: number) {
  return new Promise((resolve) => window.setTimeout(resolve, duration))
}

function previewSnapshot(): ChannelMonitorSnapshot {
  const now = Date.now()
  const items: ChannelMonitorItem[] = [
    previewItem({
      accountId: 'oauth-pro',
      name: '日常开发',
      type: 'oauth',
      status: 'operational',
      availability24h: 99.84,
      availability7d: 99.47,
      avg24h: 842,
      avg7d: 931,
      attempts24h: 628,
      attempts7d: 3812,
      failures24h: 1,
      failures7d: 20,
      capacity: 2,
      concurrency: 10,
      cost24h: 0.014283,
      cost7d: 0.092174,
      now,
    }),
    previewItem({
      accountId: 'oauth-team',
      name: '团队备用',
      type: 'oauth',
      status: 'degraded',
      availability24h: 97.73,
      availability7d: 98.91,
      avg24h: 6420,
      avg7d: 2860,
      attempts24h: 88,
      attempts7d: 642,
      failures24h: 2,
      failures7d: 7,
      capacity: 0,
      concurrency: 6,
      cost24h: 0.003182,
      cost7d: 0.026541,
      now,
    }),
    previewItem({
      accountId: 'api-key',
      name: '团队中转站',
      type: 'api_key',
      status: 'error',
      availability24h: 91.67,
      availability7d: 96.38,
      avg24h: 1920,
      avg7d: 1460,
      attempts24h: 24,
      attempts7d: 221,
      failures24h: 2,
      failures7d: 8,
      capacity: 4,
      concurrency: 20,
      cost24h: 0.008421,
      cost7d: 0.043217,
      now,
    }),
    {
      account_id: 'oauth-disabled',
      name: '归档账号池',
      account_type: 'oauth',
      account_status: 'disabled',
      models: [],
      integrity: null,
      latest_status: null,
      latest_checked_at: null,
      latest_ttfb_ms: null,
      current_capacity: 0,
      concurrency: 4,
      availability_24h: null,
      availability_7d: null,
      avg_ttfb_24h_ms: null,
      avg_ttfb_7d_ms: null,
      attempts_24h: 0,
      attempts_7d: 0,
      failed_24h: 0,
      failed_7d: 0,
      estimated_cost_24h: null,
      estimated_cost_7d: null,
      timeline: [],
    },
  ]
  const active = items.filter((item) => item.account_status === 'active')
  const total24h = active.reduce((sum, item) => sum + item.attempts_24h, 0)
  const failed24h = active.reduce((sum, item) => sum + item.failed_24h, 0)
  return {
    generated_at: now,
    total_24h: total24h,
    available_24h: total24h - failed24h,
    failed_24h: failed24h,
    availability_24h: total24h ? (total24h - failed24h) / total24h * 100 : null,
    avg_ttfb_24h_ms: 1168,
    active_channels: active.length,
    abnormal_channels: active.filter((item) => item.latest_status === 'failed' || item.latest_status === 'error').length,
    items,
  }
}

function previewItem({
  accountId,
  name,
  type,
  status,
  availability24h,
  availability7d,
  avg24h,
  avg7d,
  attempts24h,
  attempts7d,
  failures24h,
  failures7d,
  capacity,
  concurrency,
  cost24h,
  cost7d,
  now,
}: {
  accountId: string
  name: string
  type: 'oauth' | 'api_key'
  status: ChannelMonitorStatus
  availability24h: number
  availability7d: number
  avg24h: number
  avg7d: number
  attempts24h: number
  attempts7d: number
  failures24h: number
  failures7d: number
  capacity: number
  concurrency: number
  cost24h: number
  cost7d: number
  now: number
}): ChannelMonitorItem {
  const timeline = Array.from({ length: 30 }, (_, index) => {
    let pointStatus: ChannelMonitorStatus = 'operational'
    if (status === 'degraded' && index >= 27) pointStatus = 'degraded'
    if (status === 'error' && index === 29) pointStatus = 'error'
    if (index === 17 && failures7d > 0) pointStatus = 'failed'
    return previewEvent({
      id: Number(`${accountId === 'oauth-pro' ? 1 : accountId === 'oauth-team' ? 2 : 3}${index + 10}`),
      status: pointStatus,
      accountId,
      attempt: index === 29 && status === 'error' ? 1 : 0,
      createdAt: now - (29 - index) * 18 * 60_000,
      model: type === 'oauth' ? 'gpt-5.6-sol' : 'gpt-5',
    })
  }).reverse()
  return {
    account_id: accountId,
    name,
    account_type: type,
    account_status: 'active',
    models: type === 'api_key' ? ['gpt-5', 'gpt-5-mini'] : [],
    integrity: type === 'api_key' ? previewIntegrity(accountId, 'gpt-5') : null,
    latest_status: status,
    latest_checked_at: timeline[0]?.created_at ?? now,
    latest_ttfb_ms: status === 'degraded' ? 6420 : status === 'error' ? 1812 : 842,
    current_capacity: capacity,
    concurrency,
    availability_24h: availability24h,
    availability_7d: availability7d,
    avg_ttfb_24h_ms: avg24h,
    avg_ttfb_7d_ms: avg7d,
    attempts_24h: attempts24h,
    attempts_7d: attempts7d,
    failed_24h: failures24h,
    failed_7d: failures7d,
    estimated_cost_24h: cost24h,
    estimated_cost_7d: cost7d,
    timeline,
  }
}

function previewIntegrity(accountId: string, model: string): ModelIntegrityResult {
  return {
    id: 1,
    account_id: accountId,
    requested_model: model,
    declared: true,
    observed_models: [`${model}-2026-07-31`],
    risk: 'normal',
    score: 100,
    summary: '三组主动探针与标称模型一致，暂未发现明显掺水信号',
    checks: [
      { key: 'model_declaration', label: '模型声明', status: 'pass', message: `模型列表已声明 ${model}` },
      { key: 'structured_output', label: '结构化输出', status: 'pass', message: '动态挑战结果正确' },
      { key: 'tool_call', label: '工具调用', status: 'pass', message: '工具参数通过校验' },
      { key: 'context_recall', label: '多轮指令保持', status: 'pass', message: '上下文挑战结果正确' },
    ],
    probe_count: 3,
    successful_probes: 3,
    total_tokens: 184,
    reasoning_tokens: 24,
    duration_ms: 2860,
    created_at: Date.now(),
  }
}

function previewEvent({
  id,
  status,
  accountId,
  attempt,
  createdAt,
  model,
}: {
  id: number
  status: ChannelMonitorStatus
  accountId: string
  attempt: number
  createdAt: number
  model: string
}): ChannelMonitorEvent {
  return {
    id,
    request_id: `preview-${accountId}-${id}`,
    attempt_index: attempt,
    status,
    http_status: status === 'error' ? 429 : 200,
    ttfb_ms: status === 'degraded' ? 6420 : status === 'error' ? 1812 : 760 + id % 310,
    duration_ms: status === 'error' ? 1920 : 8420 + id % 1200,
    endpoint_family: 'responses',
    model,
    source: id % 13 === 0 ? 'probe' : 'traffic',
    message: status === 'error'
      ? '429 Too Many Requests，已切换到下一渠道'
      : status === 'failed'
        ? '上游流在终止事件前结束'
        : status === 'degraded'
          ? '上游响应较慢'
          : '',
    estimated_cost: status === 'error' ? 0 : 0.00012 + id % 8 * 0.00001,
    created_at: createdAt,
  }
}
