import { RefreshCw, RotateCcw } from 'lucide-react'
import { Dialog } from './Dialog'

interface ResetAccessKeyDialogProps {
  open: boolean
  busy: boolean
  onClose: () => void
  onConfirm: () => void
}

export function ResetAccessKeyDialog({
  open,
  busy,
  onClose,
  onConfirm,
}: ResetAccessKeyDialogProps) {
  return (
    <Dialog
      open={open}
      title="重置 API Key"
      onClose={onClose}
      small
      preventClose={busy}
      footer={
        <>
          <button className="btn" onClick={onClose} disabled={busy}>取消</button>
          <button className="btn btn-danger" onClick={onConfirm} disabled={busy}>
            {busy ? <RefreshCw className="spin" size={16} /> : <RotateCcw size={16} />}
            {busy ? '重置中' : '确认重置'}
          </button>
        </>
      }
    >
      <p className="confirm-copy">
        重置后当前 API Key 会立即失效，已配置的客户端需要改用新 Key。
      </p>
    </Dialog>
  )
}
