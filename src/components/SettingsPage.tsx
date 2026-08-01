import { useEffect, useState } from 'react'
import { Settings, Monitor, Info, MessageSquarePlus, Trash2 } from 'lucide-react'
import { enable, disable, isEnabled } from '@tauri-apps/plugin-autostart'
import { open } from '@tauri-apps/plugin-shell'
import { getAppVersion } from '../lib/commands'
import type { AppVersion } from '../types'

export function SettingsPage({ onOpenTrash }: { onOpenTrash?: () => void }) {
  const [autostart, setAutostart] = useState<boolean | null>(null)
  const [autostartBusy, setAutostartBusy] = useState(false)
  const [appVersion, setAppVersion] = useState<AppVersion | null>(null)

  useEffect(() => {
    isEnabled().then(setAutostart).catch(() => setAutostart(false))
    getAppVersion().then(setAppVersion).catch(() => undefined)
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
