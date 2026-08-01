import { check } from '@tauri-apps/plugin-updater'

export type UpdateStatus =
  | { state: 'idle' }
  | { state: 'checking' }
  | { state: 'available'; version: string; forced: boolean }
  | { state: 'downloading'; progress: number }
  | { state: 'installing' }
  | { state: 'up-to-date' }
  | { state: 'error'; message: string }

export type UpdateListener = (status: UpdateStatus) => void

let listeners: UpdateListener[] = []
let current: UpdateStatus = { state: 'idle' }

function emit(status: UpdateStatus) {
  current = status
  listeners.forEach((fn) => fn(status))
}

export function getUpdateStatus(): UpdateStatus {
  return current
}

export function onUpdate(fn: UpdateListener): () => void {
  listeners.push(fn)
  return () => {
    listeners = listeners.filter((l) => l !== fn)
  }
}

/** 比较语义化版本，返回 major 是否升级 */
function isMajorBump(currentVer: string, newVer: string): boolean {
  const parseMajor = (v: string) => parseInt(v.replace(/^v/, '').split('.')[0], 10) || 0
  return parseMajor(newVer) > parseMajor(currentVer)
}

/**
 * 检查更新。
 * @param currentVersion 当前版本号（用于判断大版本强制更新）
 * @param silent 静默模式：无更新时不 emit up-to-date
 */
export async function checkForUpdate(currentVersion: string, silent = true): Promise<void> {
  if (current.state === 'checking' || current.state === 'downloading' || current.state === 'installing') {
    return
  }
  emit({ state: 'checking' })
  try {
    const update = await check()
    if (!update) {
      if (!silent) emit({ state: 'up-to-date' })
      else emit({ state: 'idle' })
      return
    }
    const forced = isMajorBump(currentVersion, update.version)
    emit({ state: 'available', version: update.version, forced })
  } catch (err) {
    emit({ state: 'error', message: err instanceof Error ? err.message : String(err) })
  }
}

/**
 * 下载并安装更新，完成后自动重启。
 */
export async function downloadAndInstall(): Promise<void> {
  try {
    const update = await check()
    if (!update) return

    emit({ state: 'downloading', progress: 0 })

    let downloaded = 0
    let total = 0

    await update.downloadAndInstall((event) => {
      if (event.event === 'Started') {
        total = event.data.contentLength ?? 0
        downloaded = 0
      } else if (event.event === 'Progress') {
        downloaded += event.data.chunkLength
        const progress = total > 0 ? Math.round((downloaded / total) * 100) : 0
        emit({ state: 'downloading', progress })
      }
    })

    emit({ state: 'installing' })
    await update.install()
  } catch (err) {
    emit({ state: 'error', message: err instanceof Error ? err.message : String(err) })
  }
}
