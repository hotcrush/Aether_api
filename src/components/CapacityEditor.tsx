import { Check, Gauge, RefreshCw } from 'lucide-react'
import { useEffect, useState } from 'react'

interface CapacityEditorProps {
  accountName: string
  current: number
  limit: number
  busy: boolean
  onSave: (limit: number) => void
}

export function CapacityEditor({
  accountName,
  current,
  limit,
  busy,
  onSave,
}: CapacityEditorProps) {
  const [value, setValue] = useState(String(limit))

  useEffect(() => {
    if (!busy) setValue(String(limit))
  }, [limit, busy])

  const parsed = Number(value)
  const valid = value.trim() !== '' && Number.isInteger(parsed) && parsed >= 1 && parsed <= 1000
  const changed = valid && parsed !== limit
  const full = current >= limit
  const save = () => {
    if (changed && !busy) onSave(parsed)
  }

  return (
    <div
      className={`capacity-editor${full ? ' full' : ''}${valid ? '' : ' invalid'}`}
      title={`当前有 ${current} 个经过 Aether 的活跃请求，最多允许 ${limit} 个并发`}
    >
      <Gauge size={12} aria-hidden="true" />
      <span className="capacity-current">{current}</span>
      <span className="capacity-divider">/</span>
      <input
        type="number"
        min={1}
        max={1000}
        step={1}
        inputMode="numeric"
        value={value}
        disabled={busy}
        aria-label={`${accountName || '未命名'}的并发容量`}
        aria-invalid={!valid}
        onChange={(event) => setValue(event.target.value)}
        onKeyDown={(event) => {
          if (event.key === 'Enter') save()
          if (event.key === 'Escape') setValue(String(limit))
        }}
        onBlur={() => {
          if (changed) save()
          else setValue(String(limit))
        }}
      />
      <button
        className={`capacity-save${changed || busy ? ' visible' : ''}`}
        onMouseDown={(event) => event.preventDefault()}
        onClick={save}
        disabled={!changed || busy}
        title="保存容量"
        aria-label="保存容量"
        tabIndex={changed ? 0 : -1}
      >
        {busy ? <RefreshCw className="spin" size={12} /> : <Check size={12} />}
      </button>
    </div>
  )
}
