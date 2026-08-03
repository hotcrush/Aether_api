import { Info, Plus, RefreshCw } from 'lucide-react'
import { useEffect, useState } from 'react'
import { importAccounts } from '../lib/commands'
import { errorText } from '../lib/format'
import { Dialog } from './Dialog'

interface ApiKeyDialogProps {
  open: boolean
  onClose: () => void
  onSaved: () => Promise<void>
  notify: (message: string, error?: boolean) => void
}

export function ApiKeyDialog({ open, onClose, onSaved, notify }: ApiKeyDialogProps) {
  const [name, setName] = useState('')
  const [apiKey, setApiKey] = useState('')
  const [baseUrl, setBaseUrl] = useState('')
  const [models, setModels] = useState('')
  const [priority, setPriority] = useState('1')
  const [weight, setWeight] = useState('1')
  const [busy, setBusy] = useState(false)

  useEffect(() => {
    if (!open) return
    setName('')
    setApiKey('')
    setBaseUrl('')
    setModels('')
    setPriority('1')
    setWeight('1')
  }, [open])

  const save = async () => {
    if (!apiKey.trim()) return notify('请输入 API Key', true)
    const parsedPriority = Number(priority)
    if (!Number.isInteger(parsedPriority) || parsedPriority < 0 || parsedPriority > 1000) {
      return notify('优先级必须是 0 到 1000 的整数', true)
    }
    const parsedWeight = Number(weight)
    if (!Number.isInteger(parsedWeight) || parsedWeight < 1 || parsedWeight > 1000) {
      return notify('权重必须是 1 到 1000 的整数', true)
    }
    const normalizedModels = [...new Set(
      models.split(',').map((model) => model.trim()).filter(Boolean),
    )]
    setBusy(true)
    try {
      const payload = {
        name: name.trim(),
        platform: 'openai',
        type: 'apikey',
        models: normalizedModels,
        weight: parsedWeight,
        priority: parsedPriority,
        credentials: { api_key: apiKey.trim(), base_url: baseUrl.trim() },
      }
      const result = await importAccounts([JSON.stringify(payload)], parsedPriority)
      if (result.failed) throw new Error(result.errors[0]?.message || '添加失败')
      await onSaved()
      notify(result.updated ? '中转站已更新' : '中转站已添加')
      onClose()
    } catch (error) {
      notify(errorText(error), true)
    } finally {
      setBusy(false)
    }
  }

  return (
    <Dialog
      open={open}
      title="添加中转站"
      onClose={onClose}
      small
      preventClose={busy}
      footer={
        <>
          <button className="btn" onClick={onClose} disabled={busy}>取消</button>
          <button className="btn btn-primary" onClick={save} disabled={busy}>
            {busy ? <RefreshCw className="spin" size={16} /> : <Plus size={16} />}
            {busy ? '添加中' : '添加'}
          </button>
        </>
      }
    >
      <div className="oauth-dialog-intro">
        <Info size={15} />
        <span>兼容 OpenAI API 与 QuantumNous/new-api。new-api 请填写站点根地址（不要带 /v1），并使用中转 Token（通常以 sk- 开头）。</span>
      </div>
      <div className="field">
        <label htmlFor="keyName">名称</label>
        <input
          id="keyName"
          value={name}
          onChange={(event) => setName(event.target.value)}
          placeholder="例如：团队中转站"
        />
      </div>
      <div className="field">
        <label htmlFor="keyValue">API Key</label>
        <input
          id="keyValue"
          type="password"
          value={apiKey}
          onChange={(event) => setApiKey(event.target.value)}
          placeholder="sk-..."
        />
      </div>
      <div className="field">
        <label htmlFor="keyBaseUrl">Base URL</label>
        <input
          id="keyBaseUrl"
          value={baseUrl}
          onChange={(event) => setBaseUrl(event.target.value)}
          placeholder="https://new-api.example.com"
        />
      </div>
      <div className="field">
        <label htmlFor="keyModels">模型白名单</label>
        <input
          id="keyModels"
          value={models}
          onChange={(event) => setModels(event.target.value)}
          placeholder="gpt-5, gpt-5-mini（留空表示全部）"
        />
      </div>
      <div className="field-row">
        <div className="field">
          <label htmlFor="keyPriority">优先级</label>
          <input
            id="keyPriority"
            type="number"
            min="0"
            max="1000"
            step="1"
            value={priority}
            onChange={(event) => setPriority(event.target.value)}
          />
        </div>
        <div className="field">
          <label htmlFor="keyWeight">权重</label>
          <input
            id="keyWeight"
            type="number"
            min="1"
            max="1000"
            step="1"
            value={weight}
            onChange={(event) => setWeight(event.target.value)}
          />
        </div>
      </div>
    </Dialog>
  )
}
