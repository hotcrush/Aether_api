import { useEffect, useId, useLayoutEffect, useRef, useState, type CSSProperties } from 'react'
import { createPortal } from 'react-dom'

const TOOLTIP_SELECTOR = '[data-tooltip]'
const OPEN_DELAY_MS = 280
const FOCUS_DELAY_MS = 80
const CLOSE_DELAY_MS = 80
const VIEWPORT_GAP = 10
const ANCHOR_GAP = 8

interface ActiveTooltip {
  anchor: HTMLElement
  content: string
}

interface TooltipPosition {
  top: number
  left: number
  arrowLeft: number
  placement: 'top' | 'bottom'
}

interface TooltipStyle extends CSSProperties {
  '--tooltip-arrow-left': string
}

export function TooltipProvider() {
  const tooltipId = useId()
  const tooltipRef = useRef<HTMLDivElement>(null)
  const activeAnchorRef = useRef<HTMLElement | null>(null)
  const pendingAnchorRef = useRef<HTMLElement | null>(null)
  const pointerDownAtRef = useRef(0)
  const openTimerRef = useRef<number | null>(null)
  const closeTimerRef = useRef<number | null>(null)
  const [active, setActive] = useState<ActiveTooltip | null>(null)
  const [position, setPosition] = useState<TooltipPosition | null>(null)

  useEffect(() => {
    const clearTimer = (timerRef: { current: number | null }) => {
      if (timerRef.current !== null) window.clearTimeout(timerRef.current)
      timerRef.current = null
    }

    const hideNow = () => {
      clearTimer(openTimerRef)
      clearTimer(closeTimerRef)
      pendingAnchorRef.current = null
      activeAnchorRef.current = null
      setActive(null)
      setPosition(null)
    }

    const show = (anchor: HTMLElement, delay: number) => {
      const content = anchor.dataset.tooltip?.trim()
      if (!content) return
      clearTimer(closeTimerRef)
      if (activeAnchorRef.current === anchor) {
        setActive((current) => current?.content === content ? current : { anchor, content })
        return
      }
      if (activeAnchorRef.current) {
        clearTimer(openTimerRef)
        pendingAnchorRef.current = anchor
        activeAnchorRef.current = anchor
        setPosition(null)
        setActive({ anchor, content })
        return
      }
      clearTimer(openTimerRef)
      pendingAnchorRef.current = anchor
      openTimerRef.current = window.setTimeout(() => {
        if (pendingAnchorRef.current !== anchor || !anchor.isConnected) return
        activeAnchorRef.current = anchor
        setPosition(null)
        setActive({ anchor, content })
      }, delay)
    }

    const scheduleHide = (anchor: HTMLElement) => {
      clearTimer(openTimerRef)
      pendingAnchorRef.current = null
      clearTimer(closeTimerRef)
      closeTimerRef.current = window.setTimeout(() => {
        const focusInside = anchor === document.activeElement || anchor.contains(document.activeElement)
        if (anchor.matches(':hover') || focusInside) return
        if (activeAnchorRef.current === anchor) hideNow()
      }, CLOSE_DELAY_MS)
    }

    const tooltipTarget = (target: EventTarget | null) => target instanceof Element
      ? target.closest<HTMLElement>(TOOLTIP_SELECTOR)
      : null

    const onPointerOver = (event: PointerEvent) => {
      if (event.pointerType === 'touch') return
      const anchor = tooltipTarget(event.target)
      if (anchor) show(anchor, OPEN_DELAY_MS)
    }
    const onPointerOut = (event: PointerEvent) => {
      const anchor = tooltipTarget(event.target)
      if (!anchor) return
      const related = event.relatedTarget
      if (related instanceof Node && anchor.contains(related)) return
      scheduleHide(anchor)
    }
    const onFocusIn = (event: FocusEvent) => {
      if (Date.now() - pointerDownAtRef.current < 500) return
      const anchor = tooltipTarget(event.target)
      if (anchor) show(anchor, FOCUS_DELAY_MS)
    }
    const onFocusOut = (event: FocusEvent) => {
      const anchor = tooltipTarget(event.target)
      if (anchor) scheduleHide(anchor)
    }
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === 'Escape') hideNow()
    }
    const onPointerDown = () => {
      pointerDownAtRef.current = Date.now()
      hideNow()
    }

    document.addEventListener('pointerover', onPointerOver, true)
    document.addEventListener('pointerout', onPointerOut, true)
    document.addEventListener('focusin', onFocusIn, true)
    document.addEventListener('focusout', onFocusOut, true)
    document.addEventListener('keydown', onKeyDown, true)
    document.addEventListener('pointerdown', onPointerDown, true)
    window.addEventListener('blur', hideNow)
    return () => {
      clearTimer(openTimerRef)
      clearTimer(closeTimerRef)
      document.removeEventListener('pointerover', onPointerOver, true)
      document.removeEventListener('pointerout', onPointerOut, true)
      document.removeEventListener('focusin', onFocusIn, true)
      document.removeEventListener('focusout', onFocusOut, true)
      document.removeEventListener('keydown', onKeyDown, true)
      document.removeEventListener('pointerdown', onPointerDown, true)
      window.removeEventListener('blur', hideNow)
    }
  }, [])

  useLayoutEffect(() => {
    if (!active) return
    const anchor = active.anchor
    const previousDescription = anchor.getAttribute('aria-describedby')
    const descriptions = new Set(previousDescription?.split(/\s+/).filter(Boolean) ?? [])
    descriptions.add(tooltipId)
    anchor.setAttribute('aria-describedby', [...descriptions].join(' '))

    const updatePosition = () => {
      const tooltip = tooltipRef.current
      if (!tooltip || !anchor.isConnected) return
      const anchorRect = anchor.getBoundingClientRect()
      const tooltipRect = tooltip.getBoundingClientRect()
      const roomAbove = anchorRect.top
      const roomBelow = window.innerHeight - anchorRect.bottom
      const placement = roomAbove >= tooltipRect.height + ANCHOR_GAP + VIEWPORT_GAP
        || roomAbove >= roomBelow
        ? 'top'
        : 'bottom'
      const idealTop = placement === 'top'
        ? anchorRect.top - tooltipRect.height - ANCHOR_GAP
        : anchorRect.bottom + ANCHOR_GAP
      const top = clamp(
        idealTop,
        VIEWPORT_GAP,
        Math.max(VIEWPORT_GAP, window.innerHeight - tooltipRect.height - VIEWPORT_GAP),
      )
      const left = clamp(
        anchorRect.left + anchorRect.width / 2 - tooltipRect.width / 2,
        VIEWPORT_GAP,
        Math.max(VIEWPORT_GAP, window.innerWidth - tooltipRect.width - VIEWPORT_GAP),
      )
      const arrowLeft = clamp(
        anchorRect.left + anchorRect.width / 2 - left,
        12,
        Math.max(12, tooltipRect.width - 12),
      )
      setPosition({ top, left, arrowLeft, placement })
    }

    updatePosition()
    window.addEventListener('resize', updatePosition)
    document.addEventListener('scroll', updatePosition, true)
    return () => {
      window.removeEventListener('resize', updatePosition)
      document.removeEventListener('scroll', updatePosition, true)
      if (previousDescription) anchor.setAttribute('aria-describedby', previousDescription)
      else anchor.removeAttribute('aria-describedby')
    }
  }, [active, tooltipId])

  if (!active || typeof document === 'undefined') return null
  const style = {
    top: position?.top ?? -10_000,
    left: position?.left ?? -10_000,
    '--tooltip-arrow-left': `${position?.arrowLeft ?? 16}px`,
  } as TooltipStyle

  return createPortal(
    <div
      ref={tooltipRef}
      id={tooltipId}
      className={`app-tooltip ${position?.placement ?? 'top'}${position ? ' visible' : ''}`}
      style={style}
      role="tooltip"
    >
      {active.content}
    </div>,
    document.body,
  )
}

function clamp(value: number, min: number, max: number) {
  return Math.min(max, Math.max(min, value))
}
