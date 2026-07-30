import { RefreshCw, Trash2 } from 'lucide-react'
import type { Account } from '../types'
import { Dialog } from './Dialog'

interface DeleteAccountDialogProps {
  account: Account | null
  busy: boolean
  onClose: () => void
  onConfirm: () => void
}

export function DeleteAccountDialog({
  account,
  busy,
  onClose,
  onConfirm,
}: DeleteAccountDialogProps) {
  return (
    <Dialog
      open={Boolean(account)}
      title="删除上游"
      onClose={onClose}
      small
      preventClose={busy}
      footer={
        <>
          <button className="btn" onClick={onClose} disabled={busy}>取消</button>
          <button className="btn btn-danger" onClick={onConfirm} disabled={busy}>
            {busy ? <RefreshCw className="spin" size={16} /> : <Trash2 size={16} />}
            删除
          </button>
        </>
      }
    >
      <p className="confirm-copy">
        确定删除“{account?.name || '未命名'}”？本机保存的凭据也会一并移除。
      </p>
    </Dialog>
  )
}
