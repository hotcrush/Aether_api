import { useEffect, useState } from 'react'
import { Settings, Monitor, Info, MessageSquarePlus, Network, ShieldCheck, Trash2 } from 'lucide-react'
import { enable, disable, isEnabled } from '@tauri-apps/plugin-autostart'
import { open } from '@tauri-apps/plugin-shell'
import { getAppVersion, getCostGuardSettings, getOutboundProxySettings, updateCostGuardSettings, updateOutboundProxySettings } from '../lib/commands'
import type { AppVersion, CostGuardSettings, OutboundProxySettings } from '../types'

export function SettingsPage({ onOpenTrash }: { onOpenTrash?: () => void }) {
  const [autostart, setAutostart] = useState<boolean | null>(null)
  const [autostartBusy, setAutostartBusy] = useState(false)
  const [appVersion, setAppVersion] = useState<AppVersion | null>(null)
  const [costGuard, setCostGuard] = useState<CostGuardSettings | null>(null)
  const [costGuardBusy, setCostGuardBusy] = useState(false)
  const [costGuardError, setCostGuardError] = useState('')
  const [outboundProxy, setOutboundProxy] = useState<OutboundProxySettings | null>(null)
  const [outboundProxyBusy, setOutboundProxyBusy] = useState(false)
  const [outboundProxyError, setOutboundProxyError] = useState('')

  useEffect(() => {
    isEnabled().then(setAutostart).catch(() => setAutostart(false))
    getAppVersion().then(setAppVersion).catch(() => undefined)
    getCostGuardSettings().then(setCostGuard).catch(() => setCostGuardError('无法读取成本保护设置'))
    getOutboundProxySettings().then(setOutboundProxy).catch(() => setOutboundProxyError('无法读取出站代理设置'))
  }, [])

  const toggleAutostart = async () => {
    if (autostartBusy || autostart === null) return
    setAutostartBusy(true)
    try {
      if (autostart) {
        await disable()
        setAutostart(false)
      } else {
        await enable()
        setAutostart(true)
      }
    } catch {
      // revert on failure
      setAutostart(await isEnabled().catch(() => false))
    } finally {
      setAutostartBusy(false)
    }
  }

  const saveCostGuard = async (next: CostGuardSettings) => {
    if (costGuardBusy) return
    setCostGuardBusy(true)
    setCostGuardError('')
    try {
      setCostGuard(await updateCostGuardSettings(next))
    } catch (error) {
      setCostGuardError(error instanceof Error ? error.message : '保存成本保护设置失败')
    } finally {
      setCostGuardBusy(false)
    }
  }

  const saveOutboundProxy = async (next: OutboundProxySettings) => {
    if (outboundProxyBusy) return
    setOutboundProxyBusy(true)
    setOutboundProxyError('')
    try {
      setOutboundProxy(await updateOutboundProxySettings(next))
    } catch (error) {
      setOutboundProxyError(error instanceof Error ? error.message : '保存出站代理设置失败')
    } finally {
      setOutboundProxyBusy(false)
    }
  }

  const sendFeedback = async (version: AppVersion | null) => {
    const ver = version ? `v${version.version} (${version.commit})` : '未知版本'
    const subject = encodeURIComponent('[Aether 反馈] ')
    const body = encodeURIComponent(
      `应用版本：${ver}\n操作系统：Windows\n\n` +
      `问题描述：\n\n\n` +
      `复现步骤：\n1. \n\n` +
      `期望行为：\n\n\n` +
      `实际行为：\n\n`,
    )
    try {
      await open(`mailto:siriushhfyy@gmail.com?subject=${subject}&body=${body}`)
    } catch {
      // fallback: open in default browser
      window.open(`mailto:siriushhfyy@gmail.com?subject=${subject}&body=${body}`)
    }
  }

  return (
    <main className="settings-page">
      <section className="settings-section" aria-label="通用设置">
        <header className="settings-section-head">
          <Settings size={18} />
          <h2>设置</h2>
        </header>

        <div className="settings-group">
          <h3>启动</h3>
          <div className="settings-row">
            <div className="settings-row-info">
              <Monitor size={16} />
              <div>
                <span className="settings-row-label">开机自启动</span>
                <span className="settings-row-desc">登录 Windows 后自动启动 Aether；关闭主窗口后应用继续驻留托盘</span>
              </div>
            </div>
            <button
              type="button"
              className={`settings-toggle${autostart ? ' on' : ''}`}
              onClick={() => { void toggleAutostart() }}
              disabled={autostartBusy || autostart === null}
              aria-label="切换开机自启动"
            >
              <span className="settings-toggle-knob" />
            </button>
          </div>
        </div>

        <div className="settings-group">
          <h3>成本保护</h3>
          <div className="settings-row settings-row-top">
            <div className="settings-row-info">
              <ShieldCheck size={16} />
              <div>
                <span className="settings-row-label">仅路由可接受成本的上游</span>
                <span className="settings-row-desc">关闭时不影响现有调度；开启后会跳过超过成本上限的渠道</span>
              </div>
            </div>
            <button
              type="button"
              className={`settings-toggle${costGuard?.enabled ? ' on' : ''}`}
              onClick={() => costGuard && void saveCostGuard({ ...costGuard, enabled: !costGuard.enabled })}
              disabled={costGuardBusy || !costGuard}
              aria-label="切换成本保护"
            >
              <span className="settings-toggle-knob" />
            </button>
          </div>
          {costGuard && (
            <div className="cost-guard-fields">
              <label>
                <span>最高成本倍率</span>
                <input
                  type="number"
                  min={0}
                  max={100}
                  step={0.01}
                  value={costGuard.max_cost_multiplier}
                  disabled={costGuardBusy}
                  onChange={(event) => setCostGuard({ ...costGuard, max_cost_multiplier: Number(event.target.value) })}
                  onBlur={() => void saveCostGuard(costGuard)}
                />
                <em>×</em>
              </label>
              <label>
                <span>安全缓冲</span>
                <input
                  type="number"
                  min={0}
                  max={95}
                  step={1}
                  value={Math.round(costGuard.safety_buffer * 100)}
                  disabled={costGuardBusy}
                  onChange={(event) => setCostGuard({ ...costGuard, safety_buffer: Number(event.target.value) / 100 })}
                  onBlur={() => void saveCostGuard(costGuard)}
                />
                <em>%</em>
              </label>
            </div>
          )}
          <span className="settings-row-desc">实际准入上限 = 最高成本倍率 × (1 − 安全缓冲)。</span>
          {costGuardError && <span className="settings-error">{costGuardError}</span>}
        </div>

        <div className="settings-group">
          <h3>出站代理</h3>
          <div className="settings-row settings-row-top">
            <div className="settings-row-info">
              <Network size={16} />
              <div>
                <span className="settings-row-label">让 Aether 通过代理访问 OpenAI</span>
                <span className="settings-row-desc">即时用于 OAuth 登录、令牌刷新、额度查询、账号检测及本地中转的上游请求</span>
              </div>
            </div>
            <button
              type="button"
              className={`settings-toggle${outboundProxy?.enabled ? ' on' : ''}`}
              onClick={() => outboundProxy && void saveOutboundProxy({ ...outboundProxy, enabled: !outboundProxy.enabled })}
              disabled={outboundProxyBusy || !outboundProxy}
              aria-label="切换出站代理"
            >
              <span className="settings-toggle-knob" />
            </button>
          </div>
          {outboundProxy && (
            <label className="outbound-proxy-url">
              <span>代理地址</span>
              <input
                value={outboundProxy.url}
                disabled={outboundProxyBusy}
                onChange={(event) => setOutboundProxy({ ...outboundProxy, url: event.target.value })}
                onBlur={() => void saveOutboundProxy(outboundProxy)}
                placeholder="http://127.0.0.1:7890"
                spellCheck={false}
              />
            </label>
          )}
          <span className="settings-row-desc">默认 <code>http://127.0.0.1:7890</code>。内置授权页会继承 HTTP、SOCKS5/SOCKS5H 代理；HTTPS 代理用于后端请求。</span>
          {outboundProxyError && <span className="settings-error">{outboundProxyError}</span>}
        </div>

        <div className="settings-group">
          <h3>关于</h3>
          <div className="settings-about">
            <div className="settings-about-row">
              <Info size={16} />
              <div>
                <span className="settings-row-label">Aether</span>
                <span className="settings-row-desc">
                  {appVersion
                    ? `v${appVersion.version} · ${appVersion.commit} · ${appVersion.build_time}`
                    : '本地 OpenAI/Codex 多上游网关'}
                </span>
              </div>
            </div>
          </div>
        </div>

        <div className="settings-group">
          <h3>数据</h3>
          <div className="settings-row">
            <div className="settings-row-info">
              <Trash2 size={16} />
              <div>
                <span className="settings-row-label">回收站</span>
                <span className="settings-row-desc">查看、恢复或永久删除 OAuth 账号和 API Key 中转站</span>
              </div>
            </div>
            <button type="button" className="btn settings-feedback-btn" onClick={onOpenTrash}>
              打开回收站
            </button>
          </div>
        </div>

        <div className="settings-group">
          <h3>反馈</h3>
          <div className="settings-row">
            <div className="settings-row-info">
              <MessageSquarePlus size={16} />
              <div>
                <span className="settings-row-label">意见反馈 / Bug 报告</span>
                <span className="settings-row-desc">一键生成邮件模板，通过系统邮件客户端发送</span>
              </div>
            </div>
            <button
              type="button"
              className="btn btn-primary settings-feedback-btn"
              onClick={() => { void sendFeedback(appVersion) }}
            >
              发送反馈
            </button>
          </div>
        </div>
      </section>
    </main>
  )
}
