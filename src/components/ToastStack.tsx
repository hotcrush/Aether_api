import { Check, X } from 'lucide-react'
import { useEffect, useRef, useState } from 'react'
import type { ToastItem } from '../types'

interface DisplayToast extends ToastItem {
  leaving?: boolean
}

export function ToastStack({ items }: { items: ToastItem[] }) {
  const [display, setDisplay] = useState<DisplayToast[]>([])
  const prevIds = useRef<Set<number>>(new Set())

  useEffect(() => {
    const currentIds = new Set(items.map((item) => item.id))
    // newly added
    const added = items.filter((item) => !prevIds.current.has(item.id))
    // removed -> mark leaving
    const removedIds = [...prevIds.current].filter((id) => !currentIds.has(id))
    prevIds.current = currentIds

    if (added.length || removedIds.length) {
      setDisplay((current) => {
        let next = current.filter((item) => currentIds.has(item.id))
        if (removedIds.length) {
          next = next.map((item) =>
            removedIds.includes(item.id) ? { ...item, leaving: true } : item,
          )
        }
        return [...next, ...added]
      })
      if (removedIds.length) {
        window.setTimeout(() => {
          setDisplay((current) => current.filter((item) => !item.leaving))
        }, 260)
      }
    }
  }, [items])

  return (
    <div className="toast-stack" aria-live="polite">
      {display.map((item) => (
        <div
          className={`toast${item.error ? ' error' : ''}${item.leaving ? ' leaving' : ''}`}
          key={item.id}
        >
          {item.error ? <X size={17} /> : <Check size={17} />}
          <div>{item.message}</div>
        </div>
      ))}
    </div>
  )
}
