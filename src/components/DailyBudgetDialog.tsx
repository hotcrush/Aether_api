import { DollarSign, Save, Trash2 } from 'lucide-react'
import { useEffect, useState } from 'react'
import { errorText } from '../lib/format'
import { Dialog } from './Dialog'

interface DailyBudgetDialogProps {
  open: boolean
  limitUsd: number | null
  todayCost: number
  onClose: () => void
  onSave: (limitUsd: number | null) => Promise<void>
}

export function DailyBudgetDialog({
  open,
  limitUsd,
  todayCost,
  onClose,
  onSave,
}: DailyBudgetDialogProps) {
  const [value, setValue] = useState('')
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState('')

  useEffect(() => {
    if (!open) return
    setValue(limitUsd === null ? '' : String(limitUsd))
    setError('')
  }, [limitUsd, open])

  const persist = async (next: number | null) => {
    setBusy(true)
    setError('')
    try {
      await onSave(next)
      onClose()
    } catch (saveError) {
      setError(errorText(saveError))
    } finally {
      setBusy(false)
    }
  }

  const submit = () => {
    const parsed = Number(value)
    if (!Number.isFinite(parsed) || parsed <= 0) {
      setError('请输入大于 0 的每日美元预算')
      return
    }
    void persist(Math.round(parsed * 100) / 100)
  }

  return (
    <Dialog
      open={open}
      title="每日 USD 预算"
      onClose={onClose}
      small
      preventClose={busy}
      footer={
        <>
          {limitUsd !== null && (
            <button className="btn btn-danger" onClick={() => void persist(null)} disabled={busy}>
              <Trash2 size={15} />清除预算
            </button>
          )}
          <button className="btn" onClick={onClose} disabled={busy}>取消</button>
          <button className="btn btn-primary" onClick={submit} disabled={busy}>
            <Save size={15} />保存
          </button>
        </>
      }
    >
      <p className="confirm-copy">
        按本地时间每天重新统计。今日估算费用 <strong>${todayCost.toFixed(4)}</strong>。
        此预算仅用于本地展示，超出后剩余显示为 $0，不会停止代理、自动停用上游或参与路由。
      </p>
      <div className="field">
        <label htmlFor="dailyBudgetUsd">每日预算（USD）</label>
        <div className="budget-input-wrap">
          <DollarSign size={15} />
          <input
            id="dailyBudgetUsd"
            type="number"
            min="0.01"
            step="0.01"
            value={value}
            onChange={(event) => setValue(event.target.value)}
            placeholder="例如 10.00"
            autoFocus
          />
        </div>
      </div>
      {error && <p className="field-error">{error}</p>}
    </Dialog>
  )
}
