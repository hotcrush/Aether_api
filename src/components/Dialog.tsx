import { X } from 'lucide-react'
import { useEffect, type ReactNode } from 'react'

interface DialogProps {
  open: boolean
  title: string
  onClose: () => void
  children: ReactNode
  footer: ReactNode
  small?: boolean
  preventClose?: boolean
}

export function Dialog({
  open,
  title,
  onClose,
  children,
  footer,
  small = false,
  preventClose = false,
}: DialogProps) {
  useEffect(() => {
    if (!open) return
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === 'Escape' && !preventClose) onClose()
    }
    window.addEventListener('keydown', onKeyDown)
    return () => window.removeEventListener('keydown', onKeyDown)
  }, [open, onClose, preventClose])

  if (!open) return null

  return (
    <div
      className="overlay"
      onMouseDown={(event) => event.target === event.currentTarget && !preventClose && onClose()}
    >
      <div
        className={`dialog${small ? ' dialog-small' : ''}`}
        role="dialog"
        aria-modal="true"
        aria-label={title}
      >
        <div className="dialog-head">
          <h3 className="dialog-title">{title}</h3>
          <button
            className="icon-btn"
            onClick={onClose}
            disabled={preventClose}
            title="关闭"
            aria-label="关闭"
          >
            <X size={17} />
          </button>
        </div>
        <div className="dialog-body">{children}</div>
        <div className="dialog-foot">{footer}</div>
      </div>
    </div>
  )
}
