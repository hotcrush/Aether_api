import { Lock } from 'lucide-react'
import { formatExpiry, formatFullDate, formatRelativeTime } from '../lib/time'
import type { Account } from '../types'
import { Dialog } from './Dialog'

interface AccountDetailDialogProps {
  account: Account | null
  onClose: () => void
}

function timestamp(value: string) {
  return `${formatFullDate(value)} · ${formatRelativeTime(value)}`
}

function DetailRow({ label, value }: { label: string; value: string }) {
  if (!value) return null
  return (
    <div className="account-detail-row">
      <dt>{label}</dt>
      <dd>{value}</dd>
    </div>
  )
}

export function AccountDetailDialog({ account, onClose }: AccountDetailDialogProps) {
  const oauth = account?.account_type === 'oauth'
  return (
    <Dialog
      open={Boolean(account)}
      title="账号详情"
      onClose={onClose}
      footer={<button className="btn" type="button" onClick={onClose}>关闭</button>}
    >
      {account && (
        <div className="account-detail">
          <div className="account-detail-head">
            <span className="account-detail-name">{account.name || '未命名'}</span>
            <span className={`badge ${oauth ? 'badge-oauth' : 'badge-key'}`}>
              {oauth ? 'OAuth' : '中转站'}
            </span>
            <span className={`badge ${account.status === 'active' ? 'badge-plan' : ''}`}>
              {account.status === 'active' ? '启用' : '停用'}
            </span>
            {account.locked && (
              <span className="account-detail-lock" data-tooltip="已锁定，置顶显示">
                <Lock size={13} />已锁定
              </span>
            )}
          </div>

          <dl className="account-detail-list">
            <DetailRow label="邮箱" value={account.email} />
            <DetailRow label="套餐" value={account.plan_type} />
            <DetailRow label="ChatGPT 账号 ID" value={account.chatgpt_account_id} />
            <DetailRow label="ChatGPT 用户 ID" value={account.chatgpt_user_id} />
            <DetailRow
              label="Base URL"
              value={account.base_url || (!oauth ? 'https://api.openai.com' : '')}
            />
            <DetailRow label="凭据" value={account.credential_masked} />
            <DetailRow
              label="模型白名单"
              value={account.models?.length ? account.models.join('、') : '全部模型'}
            />
            <DetailRow label="优先级" value={String(account.priority ?? 1)} />
            <DetailRow label="权重" value={String(account.weight ?? 1)} />
            <DetailRow label="并发容量" value={String(account.concurrency ?? 10)} />
            <DetailRow label="成本倍率" value={`×${(account.rate_multiplier ?? 1).toFixed(2)}`} />
            <DetailRow
              label="自动倍率同步"
              value={account.auto_sync_rate_multiplier ? '已开启' : '已关闭'}
            />
            <DetailRow
              label="Token 过期"
              value={account.expires_at
                ? `${formatFullDate(account.expires_at)} · ${formatExpiry(account.expires_at)}`
                : ''}
            />
            <DetailRow label="首次导入时间" value={account.created_at ? timestamp(account.created_at) : ''} />
            <DetailRow label="最近更新时间" value={account.updated_at ? timestamp(account.updated_at) : ''} />
            <DetailRow label="最近使用" value={account.last_used_at ? formatFullDate(account.last_used_at) : ''} />
            <DetailRow
              label="请求次数"
              value={account.request_count ? account.request_count.toLocaleString() : '0'}
            />
          </dl>

          {account.last_error && (
            <div className="account-detail-error" data-tooltip={account.last_error}>
              {account.last_error}
            </div>
          )}
        </div>
      )}
    </Dialog>
  )
}
