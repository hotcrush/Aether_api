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
        确定将“{account?.name || '未命名'}”移入回收站？移入后不再参与路由，恢复后会重新启用；凭据仅在永久删除时移除。
      </p>
    </Dialog>
  )
}
