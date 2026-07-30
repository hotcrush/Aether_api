import { Check, RefreshCw } from 'lucide-react'
import { useEffect, useState } from 'react'

interface PriorityEditorProps {
  accountName: string
  priority: number
  busy: boolean
  onSave: (priority: number) => void
}

export function PriorityEditor({
  accountName,
  priority,
  busy,
  onSave,
}: PriorityEditorProps) {
  const [value, setValue] = useState(String(priority))

  useEffect(() => {
    if (!busy) setValue(String(priority))
  }, [priority, busy])

  const parsed = Number(value)
  const valid = value.trim() !== '' && Number.isInteger(parsed) && parsed >= 0 && parsed <= 1000
  const changed = valid && parsed !== priority

  const save = () => {
    if (changed && !busy) onSave(parsed)
  }

  return (
    <div className={`priority-editor${valid ? '' : ' invalid'}`}>
      <input
        type="number"
        min={0}
        max={1000}
        step={1}
        inputMode="numeric"
        value={value}
        disabled={busy}
        aria-label={`${accountName || '未命名'}的优先级`}
        aria-invalid={!valid}
        onChange={(event) => setValue(event.target.value)}
        onKeyDown={(event) => {
          if (event.key === 'Enter') save()
          if (event.key === 'Escape') setValue(String(priority))
        }}
        onBlur={() => {
          if (changed) save()
          else if (!valid) setValue(String(priority))
        }}
      />
      <button
        className={`priority-save${changed || busy ? ' visible' : ''}`}
        onMouseDown={(event) => event.preventDefault()}
        onClick={save}
        disabled={!changed || busy}
        title="保存优先级"
        aria-label="保存优先级"
        tabIndex={changed ? 0 : -1}
      >
        {busy ? <RefreshCw className="spin" size={13} /> : <Check size={13} />}
      </button>
    </div>
  )
}
