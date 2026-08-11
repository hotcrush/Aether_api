import { useEffect, useMemo, useState } from 'react'
import { open } from '@tauri-apps/plugin-shell'
import {
  Check,
  FileInput,
  FolderOpen,
  MessageSquareText,
  Plus,
  RefreshCw,
  Save,
  Sparkles,
  Trash2,
} from 'lucide-react'
import {
  deleteCodexPrompt,
  getCodexPromptState,
  getCodexSkillState,
  importCurrentCodexPrompt,
  saveCodexPrompt,
  setCodexSkillEnabled,
} from '../lib/commands'
import type { CodexPromptPreset, CodexPromptState, CodexSkillState } from '../types'

type ResourceView = 'prompts' | 'skills'

interface PromptDraft {
  id: string | null
  name: string
  content: string
}

const EMPTY_PROMPT: PromptDraft = { id: null, name: '', content: '' }

export function CodexResourcesPanel() {
  const [view, setView] = useState<ResourceView>('prompts')
  const [promptState, setPromptState] = useState<CodexPromptState | null>(null)
  const [skillState, setSkillState] = useState<CodexSkillState | null>(null)
  const [draft, setDraft] = useState<PromptDraft>(EMPTY_PROMPT)
  const [promptBusy, setPromptBusy] = useState('')
  const [skillBusy, setSkillBusy] = useState('')
  const [error, setError] = useState('')
  const [notice, setNotice] = useState('')

  const selectedPrompt = useMemo(
    () => promptState?.prompts.find((item) => item.id === draft.id) ?? null,
    [draft.id, promptState?.prompts],
  )
  const promptChanged = Boolean(
    draft.name.trim()
      && (draft.id === null
        || selectedPrompt?.name !== draft.name
        || selectedPrompt?.content !== draft.content),
  )

  useEffect(() => {
    let disposed = false
    Promise.all([getCodexPromptState(), getCodexSkillState()])
      .then(([prompts, skills]) => {
        if (disposed) return
        setPromptState(prompts)
        setSkillState(skills)
        const selected = prompts.prompts.find((item) => item.id === prompts.active_id)
          ?? prompts.prompts[0]
        setDraft(selected ? promptDraft(selected) : EMPTY_PROMPT)
      })
      .catch((nextError) => {
        if (!disposed) setError(errorText(nextError))
      })
    return () => { disposed = true }
  }, [])

  const applyPromptState = (next: CodexPromptState, preferredId?: string | null) => {
    setPromptState(next)
    const selected = next.prompts.find((item) => item.id === preferredId)
      ?? next.prompts.find((item) => item.id === next.active_id)
      ?? next.prompts[0]
    setDraft(selected ? promptDraft(selected) : EMPTY_PROMPT)
  }

  const savePrompt = async (activate: boolean) => {
    if (promptBusy || !draft.name.trim()) return
    setPromptBusy(activate ? 'activate' : 'save')
    setError('')
    setNotice('')
    try {
      const next = await saveCodexPrompt(draft.id, draft.name, draft.content, activate)
      const preferredId = draft.id
        ?? next.prompts.find((item) => item.name === draft.name.trim() && item.content === draft.content)?.id
      applyPromptState(next, preferredId)
      setNotice(activate ? '提示词已写入 AGENTS.md' : '提示词预设已保存')
    } catch (nextError) {
      setError(errorText(nextError))
    } finally {
      setPromptBusy('')
    }
  }

  const importCurrent = async () => {
    if (promptBusy) return
    setPromptBusy('import')
    setError('')
    setNotice('')
    try {
      const next = await importCurrentCodexPrompt()
      applyPromptState(next, next.active_id)
      setNotice('当前 AGENTS.md 已加入预设')
    } catch (nextError) {
      setError(errorText(nextError))
    } finally {
      setPromptBusy('')
    }
  }

  const removePrompt = async () => {
    if (!draft.id || promptBusy) return
    if (!window.confirm(`删除提示词预设“${draft.name}”？AGENTS.md 文件内容会保留。`)) return
    setPromptBusy('delete')
    setError('')
    setNotice('')
    try {
      applyPromptState(await deleteCodexPrompt(draft.id))
      setNotice('提示词预设已删除')
    } catch (nextError) {
      setError(errorText(nextError))
    } finally {
      setPromptBusy('')
    }
  }

  const refreshSkills = async () => {
    if (skillBusy) return
    setSkillBusy('refresh')
    setError('')
    setNotice('')
    try {
      setSkillState(await getCodexSkillState())
      setNotice('Skills 列表已刷新')
    } catch (nextError) {
      setError(errorText(nextError))
    } finally {
      setSkillBusy('')
    }
  }

  const toggleSkill = async (directory: string, enabled: boolean) => {
    if (skillBusy) return
    setSkillBusy(directory)
    setError('')
    setNotice('')
    try {
      setSkillState(await setCodexSkillEnabled(directory, enabled))
      setNotice(enabled ? 'Skill 已启用' : 'Skill 已停用')
    } catch (nextError) {
      setError(errorText(nextError))
    } finally {
      setSkillBusy('')
    }
  }

  return (
    <section className="codex-resources" aria-label="Codex 提示词与 Skills">
      <header className="codex-resources-head">
        <div className="codex-heading">
          <div className="codex-title">
            <Sparkles size={18} />
            Codex 扩展
          </div>
          <div className="codex-subtitle">
            {view === 'prompts' ? promptState?.file_path || 'AGENTS.md' : skillState?.skills_dir || 'Skills'}
          </div>
        </div>
        <div className="codex-resource-tabs" role="tablist" aria-label="Codex 扩展类型">
          <button
            type="button"
            className={view === 'prompts' ? 'active' : ''}
            onClick={() => setView('prompts')}
            role="tab"
            aria-selected={view === 'prompts'}
          >
            <MessageSquareText size={14} />
            提示词
          </button>
          <button
            type="button"
            className={view === 'skills' ? 'active' : ''}
            onClick={() => setView('skills')}
            role="tab"
            aria-selected={view === 'skills'}
          >
            <Sparkles size={14} />
            Skills
          </button>
        </div>
      </header>

      {view === 'prompts' ? (
        <div className="codex-prompt-workspace">
          <aside className="codex-prompt-list" aria-label="提示词预设">
            <button
              type="button"
              className={`codex-prompt-item new${draft.id === null ? ' active' : ''}`}
              onClick={() => setDraft({ ...EMPTY_PROMPT })}
            >
              <Plus size={14} />
              新建预设
            </button>
            {promptState?.prompts.map((prompt) => (
              <button
                type="button"
                className={`codex-prompt-item${draft.id === prompt.id ? ' active' : ''}`}
                key={prompt.id}
                onClick={() => setDraft(promptDraft(prompt))}
              >
                <span>{prompt.name}</span>
                {promptState.active_id === prompt.id && <Check size={13} aria-label="当前启用" />}
              </button>
            ))}
            {promptState?.file_exists && (
              <button
                type="button"
                className="codex-prompt-import"
                onClick={() => { void importCurrent() }}
                disabled={Boolean(promptBusy)}
              >
                <FileInput className={promptBusy === 'import' ? 'spin' : undefined} size={14} />
                导入当前文件
              </button>
            )}
          </aside>

          <div className="codex-prompt-editor">
            <input
              className="codex-prompt-name"
              value={draft.name}
              onChange={(event) => setDraft({ ...draft, name: event.target.value })}
              placeholder="预设名称"
              maxLength={80}
              disabled={Boolean(promptBusy)}
              aria-label="提示词预设名称"
            />
            <textarea
              value={draft.content}
              onChange={(event) => setDraft({ ...draft, content: event.target.value })}
              placeholder="# AGENTS.md"
              spellCheck={false}
              disabled={Boolean(promptBusy)}
              aria-label="提示词内容"
            />
            <div className="codex-prompt-actions">
              <span>
                {draft.id && promptState?.active_id === draft.id
                  ? '当前启用'
                  : promptChanged ? '有未保存修改' : '预设未启用'}
              </span>
              {draft.id && (
                <button
                  type="button"
                  className="icon-btn codex-prompt-delete"
                  onClick={() => { void removePrompt() }}
                  disabled={Boolean(promptBusy)}
                  data-tooltip="删除预设"
                  aria-label="删除提示词预设"
                >
                  <Trash2 size={14} />
                </button>
              )}
              <button
                type="button"
                className="btn"
                onClick={() => { void savePrompt(false) }}
                disabled={Boolean(promptBusy) || !promptChanged}
              >
                <Save className={promptBusy === 'save' ? 'spin' : undefined} size={14} />
                保存
              </button>
              <button
                type="button"
                className="btn btn-primary"
                onClick={() => { void savePrompt(true) }}
                disabled={Boolean(promptBusy) || !draft.name.trim()}
              >
                <Check className={promptBusy === 'activate' ? 'spin' : undefined} size={14} />
                保存并启用
              </button>
            </div>
          </div>
        </div>
      ) : (
        <div className="codex-skills-panel">
          <div className="codex-skills-toolbar">
            <span>{skillState?.skills.length ?? 0} 个 Skills</span>
            <button
              type="button"
              className="btn"
              onClick={() => skillState && void open(skillState.skills_dir)}
              disabled={!skillState}
            >
              <FolderOpen size={14} />
              打开目录
            </button>
            <button
              type="button"
              className="icon-btn"
              onClick={() => { void refreshSkills() }}
              disabled={Boolean(skillBusy)}
              data-tooltip="刷新 Skills"
              aria-label="刷新 Skills"
            >
              <RefreshCw className={skillBusy === 'refresh' ? 'spin' : undefined} size={14} />
            </button>
          </div>
          <div className="codex-skill-list">
            {skillState?.skills.map((skill) => (
              <div className={`codex-skill-row${skill.enabled ? '' : ' disabled'}`} key={skill.directory}>
                <div className="codex-skill-copy">
                  <strong>{skill.name}</strong>
                  <span>{skill.description || skill.directory}</span>
                </div>
                <code>{skill.directory}</code>
                <button
                  type="button"
                  className={`settings-toggle${skill.enabled ? ' on' : ''}`}
                  onClick={() => { void toggleSkill(skill.directory, !skill.enabled) }}
                  disabled={Boolean(skillBusy)}
                  aria-label={`${skill.enabled ? '停用' : '启用'} ${skill.name}`}
                >
                  <span className="settings-toggle-knob" />
                </button>
              </div>
            ))}
            {skillState && skillState.skills.length === 0 && (
              <div className="codex-resource-empty">Skills 目录为空</div>
            )}
          </div>
        </div>
      )}

      {(error || notice) && (
        <div className={`codex-resource-feedback${error ? ' error' : ''}`} role={error ? 'alert' : 'status'}>
          {error || notice}
        </div>
      )}
    </section>
  )
}

function promptDraft(prompt: CodexPromptPreset): PromptDraft {
  return { id: prompt.id, name: prompt.name, content: prompt.content }
}

function errorText(error: unknown) {
  return error instanceof Error ? error.message : String(error)
}
