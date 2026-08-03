import { Check, RefreshCw, RotateCw } from 'lucide-react'
import { useEffect, useState } from 'react'
import type { Account } from '../types'

interface RateMultiplierEditorProps {
  account: Account
  busy: boolean
  onSave: (multiplier: number) => void
  onAutoSync: (enabled: boolean) => void
  onSync: () => void
}

export function RateMultiplierEditor({
  account,
  busy,
  onSave,
  onAutoSync,
  onSync,
}: RateMultiplierEditorProps) {
  const [value, setValue] = useState(String(account.rate_multiplier))
  const canSync = account.account_type === 'api_key' && Boolean(account.base_url?.trim())
  const automatic = account.auto_sync_rate_multiplier

  useEffect(() => {
    if (!busy) setValue(String(account.rate_multiplier))
  }, [account.rate_multiplier, busy])

  const parsed = Number(value)
  const valid = value.trim() !== '' && Number.isFinite(parsed) && parsed >= 0 && parsed <= 100
  const changed = valid && parsed !== account.rate_multiplier
  const save = () => {
    if (changed && !busy && !automatic) onSave(parsed)
  }

  return (
    <div className="rate-multiplier-editor">
      <div className={`rate-multiplier-input${valid ? '' : ' invalid'}${automatic ? ' managed' : ''}`}>
        <input
          type="number"
          min={0}
          max={100}
          step={0.01}
          inputMode="decimal"
          value={value}
          disabled={busy || automatic}
          aria-label={`${account.name || '未命名'}的成本倍率`}
          aria-invalid={!valid}
          data-tooltip={automatic ? '已由上游自动同步' : '上游实际成本倍率，0 表示免费'}
          onChange={(event) => setValue(event.target.value)}
          onKeyDown={(event) => {
            if (event.key === 'Enter') save()
            if (event.key === 'Escape') setValue(String(account.rate_multiplier))
          }}
          onBlur={() => {
            if (changed) save()
            else if (!valid) setValue(String(account.rate_multiplier))
          }}
        />
        <span>×</span>
        <button
          className={`priority-save${changed || busy ? ' visible' : ''}`}
          onMouseDown={(event) => event.preventDefault()}
          onClick={save}
          disabled={!changed || busy || automatic}
          data-tooltip="保存成本倍率"
          aria-label="保存成本倍率"
          tabIndex={changed && !automatic ? 0 : -1}
        >
          {busy ? <RefreshCw className="spin" size={13} /> : <Check size={13} />}
        </button>
      </div>
      {canSync && (
        <div className="rate-sync-actions">
          <button
            className={`rate-auto-toggle${automatic ? ' on' : ''}`}
            type="button"
            onClick={() => onAutoSync(!automatic)}
            disabled={busy}
            aria-pressed={automatic}
            data-tooltip={automatic ? '关闭自动倍率同步' : '每 30 分钟从 Sub2API 上游同步成本倍率'}
          >
            自动
          </button>
          <button
            className="rate-sync-button"
            type="button"
            onClick={onSync}
            disabled={!automatic || busy}
            data-tooltip={automatic ? '立即从上游同步倍率' : '开启自动同步后可立即同步'}
            aria-label="立即同步成本倍率"
          >
            <RotateCw className={busy ? 'spin' : undefined} size={12} />
          </button>
        </div>
      )}
    </div>
  )
}
