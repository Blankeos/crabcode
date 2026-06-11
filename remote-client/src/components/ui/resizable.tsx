import type { ValidComponent } from "solid-js"
import { Show, splitProps } from "solid-js"

import type { DynamicProps, HandleProps, PanelProps, RootProps } from "@corvu/resizable"
import ResizablePrimitive from "@corvu/resizable"

import { cx } from "../../lib/cx"

type ResizableProps<T extends ValidComponent = "div"> = RootProps<T> & { class?: string }

const Resizable = <T extends ValidComponent = "div">(props: DynamicProps<T, ResizableProps<T>>) => {
  const [local, rest] = splitProps(props as ResizableProps, ["class"])
  return (
    <ResizablePrimitive
      class={cx("flex h-full w-full min-h-0 min-w-0 data-[orientation=vertical]:flex-col", local.class)}
      {...rest}
    />
  )
}

type ResizablePanelProps<T extends ValidComponent = "div"> = DynamicProps<T, PanelProps<T> & { class?: string }>

const ResizablePanel = <T extends ValidComponent = "div">(props: ResizablePanelProps<T>) => {
  const [local, rest] = splitProps(props as ResizablePanelProps<T>, ["class"])
  return (
    <ResizablePrimitive.Panel
      class={cx("min-h-0 min-w-0 overflow-hidden", local.class)}
      {...rest}
    />
  )
}

type ResizableHandleProps<T extends ValidComponent = "button"> = HandleProps<T> & {
  class?: string
  withHandle?: boolean
}

const ResizableHandle = <T extends ValidComponent = "button">(
  props: DynamicProps<T, ResizableHandleProps<T>>
) => {
  const [, rest] = splitProps(props as ResizableHandleProps, ["class", "withHandle"])
  return (
    <ResizablePrimitive.Handle
      class={cx(
        "relative flex shrink-0 basis-px items-center justify-center bg-[var(--line)] text-[var(--faint)] transition-colors after:absolute after:inset-y-0 after:left-1/2 after:w-3 after:-translate-x-1/2 hover:bg-[var(--line-strong)] focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-[var(--brand-primary)] data-[orientation=vertical]:h-px data-[orientation=vertical]:basis-px data-[orientation=vertical]:w-full data-[orientation=vertical]:after:left-0 data-[orientation=vertical]:after:h-3 data-[orientation=vertical]:after:w-full data-[orientation=vertical]:after:-translate-y-1/2 data-[orientation=vertical]:after:translate-x-0 [&[data-orientation=vertical]>div]:rotate-90",
        props.class
      )}
      {...rest}
    >
      <Show when={props.withHandle}>
        <div class="z-10 flex h-4 w-3 items-center justify-center rounded-sm border border-[var(--line-strong)] bg-[#242424]">
          <svg
            xmlns="http://www.w3.org/2000/svg"
            viewBox="0 0 24 24"
            fill="none"
            stroke="currentColor"
            stroke-width="2"
            stroke-linecap="round"
            stroke-linejoin="round"
            class="size-2.5"
          >
            <path d="M9 5m-1 0a1 1 0 1 0 2 0a1 1 0 1 0 -2 0" />
            <path d="M9 12m-1 0a1 1 0 1 0 2 0a1 1 0 1 0 -2 0" />
            <path d="M9 19m-1 0a1 1 0 1 0 2 0a1 1 0 1 0 -2 0" />
            <path d="M15 5m-1 0a1 1 0 1 0 2 0a1 1 0 1 0 -2 0" />
            <path d="M15 12m-1 0a1 1 0 1 0 2 0a1 1 0 1 0 -2 0" />
            <path d="M15 19m-1 0a1 1 0 1 0 2 0a1 1 0 1 0 -2 0" />
          </svg>
        </div>
      </Show>
    </ResizablePrimitive.Handle>
  )
}

export { Resizable, ResizablePanel, ResizableHandle }
