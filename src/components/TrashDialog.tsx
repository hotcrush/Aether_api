import { ArchiveRestore, Trash2, X } from 'lucide-react'
import { useCallback, useEffect, useState } from 'react'
import { listTrashedAccounts, purgeAccount, purgeAllTrashed, restoreAccount } from '../lib/commands'
import type { Account } from '../types'

interface TrashDialogProps {
  open: boolean
  onClose: () => void
  onRestored: () => void
  notify: (message: string, isError?: boolean) => void
}

export function TrashDialog({ open, onClose, onRestored, notify }: TrashDialogProps) {
  const [items, setItems] = useState<Account[]>([])
  const [loading, setLoading] = useState(false)
  const [busyId, setBusyId] = useState<string | null>(null)
  const [purgeAllBusy, setPurgeAllBusy] = useState(false)

  const load = useCallback(async () => {
    setLoading(true)
    try {
      const list = await listTrashedAccounts()
      setItems(list)
    } catch {
      setItems([])
    } finally {
      setLoading(false)
    }
  }, [])

  useEffect(() => {
    if (open) load()
  }, [open, load])

  if (!open) return null

  const handleRestore = async (account: Account) => {
    setBusyId(account.id)
    try {
      await restoreAccount(account.id)
      setItems((prev) => prev.filter((a) => a.id !== account.id))
      notify(`已恢复「${account.name}」`)
      onRestored()
    } catch (error) {
      notify(`恢复失败: ${error}`, true)
    } finally {
      setBusyId(null)
    }
  }

  const handlePurge = async (account: Account) => {
    setBusyId(account.id)
    try {
      await purgeAccount(account.id)
      setItems((prev) => prev.filter((a) => a.id !== account.id))
      notify(`已永久删除「${account.name}」`)
    } catch (error) {
      notify(`删除失败: ${error}`, true)
    } finally {
      setBusyId(null)
    }
  }

  const handlePurgeAll = async () => {
    setPurgeAllBusy(true)
    try {
      const count = await purgeAllTrashed()
      setItems([])
      notify(`已清空回收站（${count} 条）`)
    } catch (error) {
      notify(`清空失败: ${error}`, true)
    } finally {
      setPurgeAllBusy(false)
    }
  }

  return (
    <div className="dialog-backdrop" onClick={onClose}>
      <div className="dialog trash-dialog" onClick={(e) => e.stopPropagation()}>
        <div className="dialog-head">
          <h3>回收站</h3>
          <button className="icon-btn" onClick={onClose} aria-label="关闭">
            <X size={16} />
          </button>
        </div>
        <div className="trash-body">
          {loading ? (
            <p className="trash-empty">正在加载…</p>
          ) : items.length === 0 ? (
            <p className="trash-empty">回收站为空</p>
          ) : (
            <ul className="trash-list">
              {items.map((account) => (
                <li key={account.id} className="trash-item">
                  <div className="trash-item-info">
                    <span className="trash-item-name">{account.name}</span>
                    <span className="trash-item-meta">
                      {account.account_type === 'oauth' ? '账号池' : '中转站'}
                      {account.email ? ` · ${account.email}` : ''}
                    </span>
                  </div>
                  <div className="trash-item-actions">
                    <button
                      className="icon-btn"
                      title="恢复"
                      disabled={busyId === account.id}
                      onClick={() => handleRestore(account)}
                    >
                      <ArchiveRestore size={15} />
                    </button>
                    <button
                      className="icon-btn trash-purge-btn"
                      title="永久删除"
                      disabled={busyId === account.id}
                      onClick={() => handlePurge(account)}
                    >
                      <Trash2 size={15} />
                    </button>
                  </div>
                </li>
              ))}
            </ul>
          )}
        </div>
        {items.length > 0 && (
          <div className="dialog-foot">
            <button
              className="btn btn-danger"
              onClick={handlePurgeAll}
              disabled={purgeAllBusy}
            >
              <Trash2 size={14} />清空回收站
            </button>
          </div>
        )}
      </div>
    </div>
  )
}
