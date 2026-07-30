import { UploadCloud } from 'lucide-react'
import { useEffect, useRef, useState } from 'react'

export function PageImportDropZone({ onFiles }: { onFiles: (files: File[]) => void }) {
  const [dragging, setDragging] = useState(false)
  const dragDepth = useRef(0)
  const fileInput = useRef<HTMLInputElement>(null)

  useEffect(() => {
    const hasFiles = (event: DragEvent) =>
      Array.from(event.dataTransfer?.types ?? []).includes('Files')

    const onDragEnter = (event: DragEvent) => {
      if (!hasFiles(event)) return
      event.preventDefault()
      dragDepth.current += 1
      setDragging(true)
    }
    const onDragOver = (event: DragEvent) => {
      if (!hasFiles(event)) return
      event.preventDefault()
      if (event.dataTransfer) event.dataTransfer.dropEffect = 'copy'
    }
    const onDragLeave = (event: DragEvent) => {
      if (!dragDepth.current) return
      event.preventDefault()
      dragDepth.current = Math.max(0, dragDepth.current - 1)
      if (!dragDepth.current) setDragging(false)
    }
    const onDrop = (event: DragEvent) => {
      if (event.defaultPrevented) {
        dragDepth.current = 0
        setDragging(false)
        return
      }
      const files = Array.from(event.dataTransfer?.files ?? [])
      event.preventDefault()
      dragDepth.current = 0
      setDragging(false)
      if (!files.length) return
      onFiles(files)
    }

    window.addEventListener('dragenter', onDragEnter)
    window.addEventListener('dragover', onDragOver)
    window.addEventListener('dragleave', onDragLeave)
    window.addEventListener('drop', onDrop)
    return () => {
      window.removeEventListener('dragenter', onDragEnter)
      window.removeEventListener('dragover', onDragOver)
      window.removeEventListener('dragleave', onDragLeave)
      window.removeEventListener('drop', onDrop)
    }
  }, [onFiles])

  return (
    <div className="page-drop-slot">
      <section className={`page-drop-zone${dragging ? ' dragging' : ''}`} aria-label="导入上游文件">
        <UploadCloud size={18} />
        <div className="page-drop-copy">
          <strong>{dragging ? '松开以导入上游文件' : '拖放上游文件导入'}</strong>
          <span>Sub2API / CPA</span>
        </div>
        <button className="btn" onClick={() => fileInput.current?.click()}>选择文件</button>
        <input
          ref={fileInput}
          type="file"
          accept="application/json,.json"
          multiple
          hidden
          onChange={(event) => {
            const files = Array.from(event.target.files ?? [])
            if (files.length) onFiles(files)
            event.target.value = ''
          }}
        />
      </section>
    </div>
  )
}
