import { createEffect, createSignal, type JSX, onCleanup, onMount } from "solid-js"
import { cx } from "../../lib/cx"

export function CollapsiblePanel(props: { open: boolean; class?: string; children: JSX.Element }) {
  const [height, setHeight] = createSignal(0)
  const [animateHeight, setAnimateHeight] = createSignal(true)
  let innerRef: HTMLDivElement | undefined
  let previousOpen = props.open
  let restoreFrame: number | undefined

  const measure = (animate: boolean) => {
    if (restoreFrame) window.cancelAnimationFrame(restoreFrame)
    setAnimateHeight(animate)
    setHeight(innerRef?.scrollHeight ?? 0)
    if (!animate) {
      restoreFrame = window.requestAnimationFrame(() => {
        setAnimateHeight(true)
        restoreFrame = undefined
      })
    }
  }

  onMount(() => {
    measure(false)
    const resizeObserver = new ResizeObserver(() => {
      if (props.open) measure(false)
    })
    if (innerRef) resizeObserver.observe(innerRef)
    onCleanup(() => {
      resizeObserver.disconnect()
      if (restoreFrame) window.cancelAnimationFrame(restoreFrame)
    })
  })

  createEffect(() => {
    const open = props.open
    const changed = open !== previousOpen
    previousOpen = open
    queueMicrotask(() => measure(changed))
  })

  return (
    <div
      class={cx(
        "overflow-hidden duration-[210ms] ease-out",
        animateHeight() && "transition-[height,opacity,visibility]",
        props.open ? "visible opacity-100" : "invisible opacity-0 delay-[0ms,0ms,210ms]"
      )}
      style={{ height: props.open ? `${height()}px` : "0px" }}
      aria-hidden={!props.open}
    >
      <div ref={innerRef} class={props.class}>
        {props.children}
      </div>
    </div>
  )
}
