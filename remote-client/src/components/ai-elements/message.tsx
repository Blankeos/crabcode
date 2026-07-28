import type { ComponentProps, JSX } from "solid-js"
import { splitProps } from "solid-js"
import { StreamMarkdown } from "solid-streamdown"
import { cx } from "../../lib/cx"

type MessageRole = "user" | "assistant" | "system" | "tool" | string

type MessageProps = ComponentProps<"article"> & {
  from?: MessageRole
  children: JSX.Element
}

type MessageContentProps = ComponentProps<"div"> & {
  children: JSX.Element
}

type MessageResponseProps = ComponentProps<"div"> & {
  content: string
}

type MessageActionProps = ComponentProps<"button"> & {
  label?: string
}

export function Message(props: MessageProps) {
  const [local, others] = splitProps(props, ["from", "class", "children"])
  const isUser = () => local.from === "user"

  return (
    <article
      class={cx(
        "flex min-w-0 flex-col gap-2 py-3",
        isUser() ? "items-end" : "items-stretch",
        local.class
      )}
      data-role={local.from}
      {...others}
    >
      {local.children}
    </article>
  )
}

export function MessageContent(props: MessageContentProps) {
  const [local, others] = splitProps(props, ["class", "children"])
  return (
    <div class={cx("min-w-0", local.class)} {...others}>
      {local.children}
    </div>
  )
}

/** Horizontal scroll shell so wide GFM tables don't expand the page. */
export function MarkdownTable(props: ComponentProps<"table">) {
  return (
    <div class="remote-md-table">
      <table {...props} />
    </div>
  )
}

export const remoteMarkdownComponents = {
  table: MarkdownTable,
}

export function MessageResponse(props: MessageResponseProps) {
  const [local, others] = splitProps(props, ["class", "content"])
  return (
    <StreamMarkdown
      content={local.content}
      class={cx("streamdown remote-markdown", local.class)}
      components={remoteMarkdownComponents}
      {...others}
    />
  )
}

export function MessageActions(props: MessageContentProps) {
  const [local, others] = splitProps(props, ["class", "children"])
  return (
    <div class={cx("flex min-w-0 items-center gap-1", local.class)} {...others}>
      {local.children}
    </div>
  )
}

export function MessageAction(props: MessageActionProps) {
  const [local, others] = splitProps(props, ["class", "children", "label"])
  return (
    <button
      type="button"
      aria-label={local.label}
      title={local.label}
      class={cx(
        "grid h-7 w-7 place-items-center rounded-md text-[var(--faint)] transition hover:bg-[#282828] hover:text-[var(--text)] focus-visible:bg-[#282828] focus-visible:text-[var(--text)]",
        local.class
      )}
      {...others}
    >
      {local.children}
    </button>
  )
}

export function MessageToolbar(props: MessageContentProps) {
  const [local, others] = splitProps(props, ["class", "children"])
  return (
    <div class={cx("flex min-w-0 items-center justify-between gap-2", local.class)} {...others}>
      {local.children}
    </div>
  )
}
