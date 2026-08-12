import { Info, RefreshCw, Save } from 'lucide-react'
import { useEffect, useState } from 'react'
import { syncOAuthAccountModels, updateAccount } from '../lib/commands'
import { errorText } from '../lib/format'
import type { Account } from '../types'
import { Dialog } from './Dialog'

interface EditAccountDialogProps {
  account: Account | null
  onClose: () => void
  onSaved: () => Promise<void>
  notify: (message: string, error?: boolean) => void
}

export function EditAccountDialog({ account, onClose, onSaved, notify }: EditAccountDialogProps) {
  const [name, setName] = useState('')
  const [apiKey, setApiKey] = useState('')
  const [baseUrl, setBaseUrl] = useState('')
  const [models, setModels] = useState('')
  const [priority, setPriority] = useState('1')
  const [weight, setWeight] = useState('1')
  const [concurrency, setConcurrency] = useState('10')
  const [rateMultiplier, setRateMultiplier] = useState('1')
  const [busy, setBusy] = useState(false)

  useEffect(() => {
    if (!account) return
    setName(account.name || '')
    setApiKey('')
    setBaseUrl(account.base_url || '')
    setModels((account.models ?? []).join(', '))
    setPriority(String(account.priority ?? 1))
    setWeight(String(account.weight ?? 1))
    setConcurrency(String(account.concurrency ?? 10))
    setRateMultiplier(String(account.rate_multiplier ?? 1))
  }, [account])

  const save = async () => {
    if (!account) return
    if (!name.trim()) return notify('请输入账号名称', true)
    const parsedPriority = integerInRange(priority, 0, 1000)
    if (parsedPriority === null) return notify('优先级必须是 0 到 1000 的整数', true)
    const parsedWeight = integerInRange(weight, 1, 1000)
    if (parsedWeight === null) return notify('权重必须是 1 到 1000 的整数', true)
    const parsedConcurrency = integerInRange(concurrency, 1, 1000)
    if (parsedConcurrency === null) return notify('容量必须是 1 到 1000 的整数', true)
    const parsedRate = Number(rateMultiplier)
    if (!Number.isFinite(parsedRate) || parsedRate < 0 || parsedRate > 100) {
      return notify('成本倍率必须在 0 到 100 之间', true)
    }
    setBusy(true)
    try {
      await updateAccount(account.id, {
        name: name.trim(),
        api_key: apiKey.trim() || null,
        base_url: baseUrl.trim(),
        models: [...new Set(models.split(',').map((model) => model.trim()).filter(Boolean))],
        priority: parsedPriority,
        weight: parsedWeight,
        concurrency: parsedConcurrency,
        rate_multiplier: parsedRate,
      })
      await onSaved()
      notify('账号已更新')
      onClose()
    } catch (error) {
      notify(errorText(error), true)
    } finally {
      setBusy(false)
    }
  }

  const syncModels = async () => {
    if (!account) return
    setBusy(true)
    try {
      const updated = await syncOAuthAccountModels(account.id)
      setModels(updated.models.join(', '))
      await onSaved()
      notify(`已同步 ${updated.models.length} 个 Codex 模型`)
    } catch (error) {
      notify(errorText(error), true)
    } finally {
      setBusy(false)
    }
  }

  const relay = account?.account_type === 'api_key'
  return (
    <Dialog
      open={Boolean(account)}
      title="编辑账号"
      onClose={onClose}
      small
      preventClose={busy}
      footer={
        <>
          <button className="btn" onClick={onClose} disabled={busy}>取消</button>
          <button className="btn btn-primary" onClick={() => void save()} disabled={busy}>
            {busy ? <RefreshCw className="spin" size={16} /> : <Save size={16} />}
            {busy ? '保存中' : '保存'}
          </button>
        </>
      }
    >
      <div className="field">
        <label htmlFor="editAccountName">名称</label>
        <input id="editAccountName" value={name} onChange={(event) => setName(event.target.value)} />
      </div>
      {relay && (
        <>
          <div className="field">
            <label htmlFor="editAccountKey">更换 API Key</label>
            <input
              id="editAccountKey"
              type="password"
              value={apiKey}
              onChange={(event) => setApiKey(event.target.value)}
              placeholder="留空保持原 Key"
            />
          </div>
          <div className="field">
            <label htmlFor="editAccountBaseUrl">Base URL</label>
            <input id="editAccountBaseUrl" value={baseUrl} onChange={(event) => setBaseUrl(event.target.value)} />
          </div>
          <div className="field">
            <label htmlFor="editAccountModels">模型白名单</label>
            <input
              id="editAccountModels"
              value={models}
              onChange={(event) => setModels(event.target.value)}
              placeholder="留空表示全部模型"
            />
          </div>
        </>
      )}
      {account?.account_type === 'oauth' && (
        <div className="field">
          <label htmlFor="editOauthModels">Codex 模型</label>
          <div className="field-row">
            <input id="editOauthModels" value={models} readOnly placeholder="尚未同步" />
            <button className="btn" type="button" onClick={() => void syncModels()} disabled={busy}>
              <RefreshCw className={busy ? 'spin' : undefined} size={16} />
              同步
            </button>
          </div>
        </div>
      )}
      <div className="field-row">
        <div className="field">
          <label htmlFor="editAccountPriority">优先级</label>
          <input id="editAccountPriority" type="number" min="0" max="1000" value={priority} onChange={(event) => setPriority(event.target.value)} />
        </div>
        <div className="field">
          <label htmlFor="editAccountWeight">权重</label>
          <input id="editAccountWeight" type="number" min="1" max="1000" value={weight} onChange={(event) => setWeight(event.target.value)} />
        </div>
      </div>
      <div className="field-row">
        <div className="field">
          <label htmlFor="editAccountConcurrency">并发容量</label>
          <input id="editAccountConcurrency" type="number" min="1" max="1000" value={concurrency} onChange={(event) => setConcurrency(event.target.value)} />
        </div>
        <div className="field">
          <label htmlFor="editAccountRate">成本倍率</label>
          <input
            id="editAccountRate"
            type="number"
            min="0"
            max="100"
            step="0.01"
            value={rateMultiplier}
            onChange={(event) => setRateMultiplier(event.target.value)}
            disabled={Boolean(account?.auto_sync_rate_multiplier)}
          />
        </div>
      </div>
      {account?.auto_sync_rate_multiplier && (
        <div className="oauth-dialog-intro">
          <Info size={15} />
          <span>该中转站已开启自动成本倍率，保存时会保留当前自动同步值。</span>
        </div>
      )}
    </Dialog>
  )
}

function integerInRange(value: string, min: number, max: number) {
  const parsed = Number(value)
  return Number.isInteger(parsed) && parsed >= min && parsed <= max ? parsed : null
}
