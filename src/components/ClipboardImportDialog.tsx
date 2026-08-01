import { ClipboardCheck, FileDown, RefreshCw, Upload } from 'lucide-react'
import type { ClipboardImportCandidate } from '../types'
import { Dialog } from './Dialog'

interface ClipboardImportDialogProps {
  candidate: ClipboardImportCandidate | null
  busy: boolean
  onClose: () => void
  onConfirm: () => void
}

export function ClipboardImportDialog({
  candidate,
  busy,
  onClose,
  onConfirm,
}: ClipboardImportDialogProps) {
  const sourceName = candidate?.source === 'cpa' ? 'CPA' : 'Sub2API'
  const fromDownload = candidate?.detected_from === 'download'
  const SourceIcon = fromDownload ? FileDown : ClipboardCheck

  return (
    <Dialog
      open={Boolean(candidate)}
      title={fromDownload ? '从下载文件导入' : '从剪贴板导入'}
      onClose={onClose}
      preventClose={busy}
      small
      footer={
        <>
          <button className="btn" onClick={onClose} disabled={busy}>取消</button>
          <button className="btn btn-primary" onClick={onConfirm} disabled={busy}>
            {busy ? <RefreshCw className="spin" size={16} /> : <Upload size={16} />}
            {busy ? '导入中' : '确认导入'}
          </button>
        </>
      }
    >
      {candidate && (
        <div className="clipboard-import">
          <div className="clipboard-import-intro">
            <SourceIcon size={22} aria-hidden="true" />
            <div>
              <div className="clipboard-import-heading">
                检测到 {sourceName} 账号数据
              </div>
              {fromDownload && candidate.file_name && (
                <p title={candidate.file_name}>文件：{candidate.file_name}</p>
              )}
              <p>
                {candidate.source === 'cpa'
                  ? '确认后将转换为 Sub2API 格式并加入账号池。'
                  : '确认后将按 Sub2API 格式直接加入账号池。'}
              </p>
            </div>
          </div>

          <div className="clipboard-import-meta">
            <span>来源<strong>{sourceName}</strong></span>
            <span>账号数<strong>{candidate.account_count}</strong></span>
          </div>

          <div className="clipboard-account-list" aria-label="待导入账号">
            {candidate.accounts.map((account, index) => {
              const label = account.email || account.name || `账号 ${index + 1}`
              return (
                <div className="clipboard-account" key={`${account.account_type}-${label}-${index}`}>
                  <div>
                    <strong>{label}</strong>
                    {account.email && account.name && account.email !== account.name && (
                      <span>{account.name}</span>
                    )}
                  </div>
                  <span className="clipboard-account-type">
                    {account.account_type === 'oauth' ? 'OAuth' : '中转站'}
                  </span>
                </div>
              )
            })}
            {candidate.account_count > candidate.accounts.length && (
              <div className="clipboard-account-more">
                还有 {candidate.account_count - candidate.accounts.length} 个账号
              </div>
            )}
          </div>

          <p className="clipboard-import-note">已存在的同一账号将更新凭据。</p>
        </div>
      )}
    </Dialog>
  )
}
