import { KeyRound, Link, RefreshCw } from 'lucide-react'
import { useEffect, useState } from 'react'
import { beginOpenAIOAuth } from '../lib/commands'
import { errorText } from '../lib/format'
import type { OpenAIAuthorization } from '../types'
import { Dialog } from './Dialog'

interface OpenAIOAuthDialogProps {
  open: boolean
  onClose: () => void
  onAuthorizationReady: (authorization: OpenAIAuthorization) => void | Promise<void>
  notify: (message: string, error?: boolean) => void
}

export function OpenAIOAuthDialog({
  open: isOpen,
  onClose,
  onAuthorizationReady,
  notify,
}: OpenAIOAuthDialogProps) {
  const [name, setName] = useState('')
  const [priority, setPriority] = useState('1')
  const [busy, setBusy] = useState(false)

  useEffect(() => {
    if (!isOpen) return
    setName('')
    setPriority('1')
    setBusy(false)
  }, [isOpen])

  const generate = async () => {
    if (!name.trim()) return notify('请先输入账号名称', true)
    const parsedPriority = Number(priority)
    if (!Number.isInteger(parsedPriority) || parsedPriority < 0 || parsedPriority > 1000) {
      return notify('优先级必须是 0 到 1000 的整数', true)
    }
    setBusy(true)
    try {
      const authorization = await beginOpenAIOAuth(name.trim(), parsedPriority)
      const authorizationUrl = authorization.authUrl?.trim()
      if (!authorizationUrl) throw new Error('后端未返回 OpenAI 授权链接')
      const oauthState = authorization.state?.trim()
      if (!oauthState) throw new Error('后端未返回 OpenAI 授权状态')
      await onAuthorizationReady({
        ...authorization,
        authUrl: authorizationUrl,
        state: oauthState,
      })
    } catch (error) {
      notify(errorText(error), true)
    } finally {
      setBusy(false)
    }
  }

  return (
    <Dialog
      open={isOpen}
      title="添加 OpenAI 账号"
      onClose={onClose}
      preventClose={busy}
      footer={
        <button className="btn" onClick={onClose} disabled={busy}>取消</button>
      }
    >
      <div className="oauth-dialog-intro">
        <KeyRound size={17} />
        <span>OpenAI 授权将在 Aether 内置 Tab 中完成，回调由本机服务自动接收。</span>
      </div>

      <div className="oauth-step">
        <span className="oauth-step-number">1</span>
        <div className="oauth-step-content">
          <strong>输入账号信息并打开授权 Tab</strong>
          <div className="field-row">
            <div className="field">
              <label htmlFor="openaiAccountName">账号名称</label>
              <input
                id="openaiAccountName"
                value={name}
                disabled={busy}
                onChange={(event) => setName(event.target.value)}
                placeholder="例如：我的 OpenAI Pro"
              />
            </div>
            <div className="field oauth-priority-field">
              <label htmlFor="openaiAccountPriority">优先级</label>
              <input
                id="openaiAccountPriority"
                type="number"
                min="0"
                max="1000"
                step="1"
                value={priority}
                disabled={busy}
                onChange={(event) => setPriority(event.target.value)}
              />
            </div>
          </div>
          <button className="btn btn-primary oauth-generate" onClick={generate} disabled={busy}>
            {busy ? <RefreshCw className="spin" size={15} /> : <Link size={15} />}
            在内置 Tab 中授权
          </button>
        </div>
      </div>

      <div className="oauth-step">
        <span className="oauth-step-number">2</span>
        <div className="oauth-step-content">
          <strong>在内置 Tab 完成 OpenAI 账户授权</strong>
          <p>Aether 会切换到内置授权 Tab。登录并确认后，授权页将回到本机回调地址。</p>
        </div>
      </div>

      <div className="oauth-step">
        <span className="oauth-step-number">3</span>
        <div className="oauth-step-content">
          <strong>监听回调并自动导入</strong>
          <p>OpenAI 跳转到 <code>http://localhost:1455/auth/callback</code> 后，Aether 会校验回调、兑换令牌、导入账号并关闭授权 Tab。</p>
        </div>
      </div>
    </Dialog>
  )
}
