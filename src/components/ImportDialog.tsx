import { RefreshCw, Upload, UploadCloud, X } from 'lucide-react'
import { useEffect, useRef, useState, type DragEvent } from 'react'
import { importAccounts } from '../lib/commands'
import { errorText } from '../lib/format'
import type { ImportResult } from '../types'
import { Dialog } from './Dialog'

interface ImportDialogProps {
  open: boolean
  initialFiles?: File[]
  onClose: () => void
  onImported: () => Promise<void>
  notify: (message: string, error?: boolean) => void
}

const EMPTY_FILES: File[] = []

export function ImportDialog({
  open,
  initialFiles = EMPTY_FILES,
  onClose,
  onImported,
  notify,
}: ImportDialogProps) {
  const [tab, setTab] = useState<'file' | 'paste'>('file')
  const [files, setFiles] = useState<File[]>([])
  const [text, setText] = useState('')
  const [defaultPriority, setDefaultPriority] = useState('1')
  const [dragging, setDragging] = useState(false)
  const [busy, setBusy] = useState(false)
  const [result, setResult] = useState<ImportResult | null>(null)
  const fileInput = useRef<HTMLInputElement>(null)

  useEffect(() => {
    if (!open) return
    const selection = selectFiles([], initialFiles)
    setFiles(selection.files)
    setText('')
    setDefaultPriority('1')
    setResult(null)
    setTab('file')
    if (selection.ignored) notify('已忽略非 JSON 文件', true)
    if (selection.overflow) notify('一次最多导入 20 个文件', true)
  }, [open, initialFiles, notify])

  const acceptFiles = (incoming: File[] | FileList) => {
    const incomingFiles = Array.from(incoming)
    const selection = selectFiles(files, incomingFiles)
    setFiles(selection.files)
    setResult(null)
    if (selection.ignored) notify('已忽略非 JSON 文件', true)
    if (selection.overflow) notify('一次最多导入 20 个文件', true)
  }

  const dropFiles = (event: DragEvent<HTMLDivElement>) => {
    event.preventDefault()
    setDragging(false)
    acceptFiles(event.dataTransfer.files)
  }

  const runImport = async () => {
    const parsedPriority = Number(defaultPriority)
    if (
      defaultPriority.trim() === ''
      || !Number.isInteger(parsedPriority)
      || parsedPriority < 0
      || parsedPriority > 1000
    ) {
      return notify('默认优先级必须是 0 到 1000 的整数', true)
    }

    if (tab === 'file') {
      if (!files.length) return notify('请选择 JSON 文件', true)
    } else {
      if (!text.trim()) return notify('请输入导入内容', true)
    }

    setResult(null)
    setBusy(true)
    try {
      const contents = tab === 'file'
        ? await Promise.all(files.map((file) => file.text()))
        : [text]
      const imported = await importAccounts(contents, parsedPriority)
      setResult(imported)
      if (imported.created || imported.updated) await onImported()
      if (!imported.failed) {
        notify('上游导入完成')
        onClose()
      } else {
        notify('导入完成，部分条目失败', true)
      }
    } catch (error) {
      notify(errorText(error), true)
    } finally {
      setBusy(false)
    }
  }

  return (
    <Dialog
      open={open}
      title="导入上游"
      onClose={onClose}
      preventClose={busy}
      footer={
        <>
          <button className="btn" onClick={onClose} disabled={busy}>取消</button>
          <button className="btn btn-primary" onClick={runImport} disabled={busy}>
            {busy ? <RefreshCw className="spin" size={16} /> : <Upload size={16} />}
            {busy ? '导入中' : '开始导入'}
          </button>
        </>
      }
    >
      <div className="import-options">
        <div className="import-formats">
          <span>自动识别</span>
          <strong>Sub2API</strong>
          <strong>CPA</strong>
        </div>
        <label className="default-priority">
          <span>默认优先级</span>
          <input
            type="number"
            min={0}
            max={1000}
            step={1}
            inputMode="numeric"
            value={defaultPriority}
            onChange={(event) => setDefaultPriority(event.target.value)}
            aria-label="导入上游默认优先级"
          />
        </label>
      </div>
      <div className="tabs">
        <button
          className={`tab${tab === 'file' ? ' active' : ''}`}
          onClick={() => { setTab('file'); setResult(null) }}
        >
          JSON 文件
        </button>
        <button
          className={`tab${tab === 'paste' ? ' active' : ''}`}
          onClick={() => { setTab('paste'); setResult(null) }}
        >
          粘贴内容
        </button>
      </div>

      {tab === 'file' ? (
        <>
          <div
            className={`drop-zone${dragging ? ' dragging' : ''}`}
            onDragEnter={(event) => { event.preventDefault(); setDragging(true) }}
            onDragOver={(event) => event.preventDefault()}
            onDragLeave={() => setDragging(false)}
            onDrop={dropFiles}
          >
            <div>
              <UploadCloud size={25} />
              <strong>拖放多个 JSON 文件</strong>
              <button className="btn" type="button" onClick={() => fileInput.current?.click()}>
                浏览文件
              </button>
            </div>
          </div>
          <input
            ref={fileInput}
            type="file"
            accept="application/json,.json"
            multiple
            hidden
            onChange={(event) => {
              if (event.target.files) acceptFiles(event.target.files)
              event.target.value = ''
            }}
          />
          <div className="file-list">
            {files.map((file, index) => (
              <div className="file-item" key={`${file.name}-${file.size}-${file.lastModified}`}>
                <span className="file-name">{file.name}</span>
                <button
                  className="icon-btn"
                  onClick={() => setFiles((current) => current.filter((_, itemIndex) => itemIndex !== index))}
                  title="移除"
                  aria-label={`移除 ${file.name}`}
                >
                  <X size={14} />
                </button>
              </div>
            ))}
          </div>
        </>
      ) : (
        <textarea
          value={text}
          onChange={(event) => setText(event.target.value)}
          placeholder="粘贴 auth.json、Sub2API 备份或 token，每行一条"
        />
      )}

      {result && (
        <div className="import-result">
          <div
            className="result-summary"
            aria-label={`新增 ${result.created}，更新 ${result.updated}，失败 ${result.failed}`}
          >
            <span><strong>{result.created}</strong>新增</span>
            <span><strong>{result.updated}</strong>更新</span>
            <span className={result.failed ? 'result-failed' : ''}>
              <strong>{result.failed}</strong>失败
            </span>
          </div>
          {result.errors.length > 0 && (
            <div className="error-detail">
              <div className="error-title">失败明细</div>
              <ul className="error-list">
                {result.errors.map((item, index) => (
                  <li key={`${item.index}-${index}`}>
                    #{item.index}{item.name ? ` ${item.name}` : ''}: {item.message}
                  </li>
                ))}
              </ul>
            </div>
          )}
        </div>
      )}
    </Dialog>
  )
}

function selectFiles(current: File[], incoming: File[]) {
  const selected = incoming.filter((file) => file.name.toLowerCase().endsWith('.json'))
  const uniqueFiles = [...current, ...selected].filter(
    (file, index, all) => all.findIndex((item) =>
      item.name === file.name && item.size === file.size && item.lastModified === file.lastModified,
    ) === index,
  )
  return {
    files: uniqueFiles.slice(0, 20),
    ignored: selected.length !== incoming.length,
    overflow: uniqueFiles.length > 20,
  }
}
