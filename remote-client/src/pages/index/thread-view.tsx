import { type Accessor, createEffect, createMemo, createSignal, For, Index, onCleanup, Show } from "solid-js"
import { StreamMarkdown } from "solid-streamdown"
import "solid-streamdown/styles.css"
import {
  Attachment,
  AttachmentInfo,
  AttachmentPreview,
  Attachments,
  type AttachmentData,
} from "../../components/ai-elements/attachments"
import {
  Message,
  MessageAction,
  MessageActions,
  MessageContent,
  MessageResponse,
  MessageToolbar,
  remoteMarkdownComponents,
} from "../../components/ai-elements/message"
import { Shimmer } from "../../components/ai-elements/shimmer"
import { CollapsiblePanel } from "../../components/remote/collapsible-panel"
import { IconBrainGlyph, IconSubagentHierarchy } from "../../assets/icons"
import {
  IconCaretDown,
  IconCheck,
  IconCopy,
  IconFileText,
  IconGlobe,
  IconPencilSimple,
  IconSearch,
  IconTerminal,
  IconWarningCircle,
  IconX,
} from "../../icons"
import { cx } from "../../lib/cx"
import type { RemoteMessage, RemoteStatus } from "../../remote-api"
import type { DiffLine, DiffSection, ImagePreviewTarget, SubagentActivityItem, ThreadItem, ToolActivityStep, ToolIconKind, ToolMessage, ToolStepDetail, ToolVisualState } from "./page-types"
import { handleImagePreviewKeyDown, messageImageAttachmentData, promptTextPartClass, promptTextParts, promptTextPartStyle } from "./prompt-utils"
import { actionDescriptor, assistantMetrics, buildActivitySteps, messageModelLabel, trimPreview } from "./thread-model"
import { agentAccentClass, displayAgentMode, fallbackCopyText } from "./shared-utils"

export function ThreadItemView(props: {
  item: Accessor<ThreadItem>
  status: Accessor<RemoteStatus | null>
  streaming: Accessor<boolean>
  token: Accessor<string>
  onPreviewImage: (attachment: AttachmentData) => void
  onOpenSubagentSession?: (sessionId: string) => void | Promise<void>
}) {
  const message = createMemo(() => {
    const item = props.item()
    return item.type === "message" ? item.message : null
  })
  const messageActivityTools = createMemo(() => {
    const item = props.item()
    return item.type === "message" ? item.activityTools : []
  })
  const activity = createMemo(() => {
    const item = props.item()
    return item.type === "activity" ? item.tools : null
  })
  const action = createMemo(() => {
    const item = props.item()
    return item.type === "action" ? item.tool : null
  })

  return (
    <>
      <Show when={message()}>
        {(current) => (
          <MessageView
            message={current}
            activityTools={messageActivityTools}
            status={props.status}
            streaming={props.streaming}
            token={props.token}
            onPreviewImage={props.onPreviewImage}
            onOpenSubagentSession={props.onOpenSubagentSession}
          />
        )}
      </Show>
      <Show when={activity()}>{(tools) => <ToolActivityGroup tools={tools} onOpenSubagentSession={props.onOpenSubagentSession} />}</Show>
      <Show when={action()}>{(tool) => <ToolActionMessage tool={tool} />}</Show>
    </>
  )
}

function SubagentActivityDetails(props: {
  items: SubagentActivityItem[]
  preview?: string
  onOpenSession?: (sessionId: string) => void | Promise<void>
}) {
  return (
    <div class="flex min-w-0 flex-col gap-2 pt-2 pb-1">
      <div class="grid min-w-0 gap-2">
        <For each={props.items}>
          {(item, index) => {
            const openable = () => Boolean(item.sessionId && props.onOpenSession)
            const open = () => {
              if (!item.sessionId || !props.onOpenSession) return
              void props.onOpenSession(item.sessionId)
            }
            return (
            <div
              role={openable() ? "button" : undefined}
              tabIndex={openable() ? 0 : undefined}
              class={cx(
                "min-w-0 rounded-lg border border-[var(--line)] bg-white/[0.026] px-2.5 py-2 transition",
                item.state === "active" && "border-[rgba(108,142,216,0.26)] bg-[rgba(108,142,216,0.052)]",
                item.state === "error" && "border-[rgba(200,108,116,0.3)] bg-[rgba(200,108,116,0.065)]",
                openable() && "cursor-pointer hover:border-[rgba(108,142,216,0.38)] hover:bg-white/[0.04] focus-visible:outline focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-[var(--brand-primary)]"
              )}
              onClick={() => open()}
              onKeyDown={(event) => {
                if (!openable()) return
                if (event.key === "Enter" || event.key === " ") {
                  event.preventDefault()
                  open()
                }
              }}
            >
              <div class="grid min-w-0 grid-cols-[auto_minmax(0,1fr)_auto] items-start gap-2">
                <span class={cx("mt-0.5 grid h-5 w-5 place-items-center border text-[0.64rem] font-bold leading-none", agentAccentClass(item.agent))}>
                  {index() + 1}
                </span>
                <span class="flex min-w-0 flex-col gap-1">
                  <span class="flex min-w-0 flex-wrap items-center gap-x-2 gap-y-1">
                    <strong class={cx("font-mono text-[0.72rem] font-bold leading-none", agentAccentClass(item.agent))}>
                      @{item.agent}
                    </strong>
                    <span class="min-w-0 text-[0.8rem] font-medium leading-snug text-[#ddd9d0] [overflow-wrap:anywhere]">
                      {item.description}
                    </span>
                  </span>
                  <span class="flex min-w-0 flex-wrap items-center gap-x-2 gap-y-1 font-mono text-[0.68rem] leading-snug text-[var(--faint)]">
                    <SubagentStatus state={item.state} />
                    <Show when={item.durationMs !== undefined}>
                      <span>{formatDuration(item.durationMs!)} </span>
                    </Show>
                    <Show when={item.toolCallCount !== undefined}>
                      <span>{item.toolCallCount} tool{item.toolCallCount === 1 ? "" : "s"}</span>
                    </Show>
                    <Show when={item.sessionId}>
                      {(sessionId) => <span title={sessionId()}>session {sessionId().slice(0, 7)}</span>}
                    </Show>
                  </span>
                </span>
                <span
                  class={cx(
                    "mt-1 h-2 w-2 rounded-full border border-[var(--line-strong)] bg-white/10",
                    item.state === "active" && "border-[rgba(108,142,216,0.65)] bg-[var(--brand-primary)] animate-toolPulse",
                    item.state === "error" && "border-[rgba(200,108,116,0.65)] bg-[var(--red)]",
                    item.state === "complete" && "border-[rgba(92,168,134,0.5)] bg-[var(--green)]"
                  )}
                  aria-hidden="true"
                />
              </div>
            </div>
            )
          }}
        </For>
      </div>
      <Show when={props.items.some((item) => item.sessionId)}>
        <p class="m-0 pl-1 font-mono text-[0.68rem] leading-snug text-[var(--faint)]">
          {props.onOpenSession ? "Open a subagent card to view its full transcript." : "Subagent transcript opens when session browsing is available."}
        </p>
      </Show>
      <Show when={props.preview}>
        {(preview) => (
          <pre class="m-0 max-w-full overflow-x-auto whitespace-pre rounded-[7px] border border-[var(--line)] bg-black/20 p-3 font-mono text-[0.73rem] leading-normal text-[#bebbb4]">
            {trimPreview(preview())}
          </pre>
        )}
      </Show>
    </div>
  )
}

function SubagentStatus(props: { state: ToolVisualState }) {
  if (props.state === "active") return <span class="text-[#9db1ef]">running</span>
  if (props.state === "error") return <span class="text-[var(--red)]">failed</span>
  return <span class="text-[var(--green)]">complete</span>
}

function formatDuration(ms: number) {
  if (ms < 1000) return `${Math.max(0, Math.round(ms))}ms`
  return `${(ms / 1000).toFixed(1)}s`
}

function ToolActivityGroup(props: {
  tools: Accessor<ToolMessage[]>
  onOpenSubagentSession?: (sessionId: string) => void | Promise<void>
}) {
  const steps = createMemo(() => buildActivitySteps(props.tools()))
  const state = createMemo<ToolVisualState>(() => {
    if (steps().some((step) => step.state === "error")) return "error"
    if (steps().some((step) => step.state === "active")) return "active"
    return "complete"
  })

  return (
    <article class="grid grid-cols-[2rem_minmax(0,1fr)] gap-3 py-1">
      <div class="w-7" />
      <div class="w-[min(100%,44rem)] min-w-0">
        <ToolActivityTimeline steps={steps} state={state} onOpenSubagentSession={props.onOpenSubagentSession} />
      </div>
    </article>
  )
}

function ToolActivityTimeline(props: {
  steps: Accessor<ToolActivityStep[]>
  state: Accessor<ToolVisualState>
  steady?: Accessor<boolean>
  onOpenSubagentSession?: (sessionId: string) => void | Promise<void>
}) {
  return (
    <section class="flex w-[min(100%,30rem)] flex-col text-[#d8d6d1]" aria-label="Tool activity">
      <Index each={props.steps()}>{(step) => <ToolTimelineStep step={step} steady={props.steady} onOpenSubagentSession={props.onOpenSubagentSession} />}</Index>
      <Show when={props.state() === "complete"}>
        <ToolTimelineStep
          steady={props.steady}
          step={() => ({
            key: "done",
            label: "Done",
            icon: "check",
            state: "complete",
            details: [],
          })}
        />
      </Show>
    </section>
  )
}

function ToolTimelineStep(props: {
  step: Accessor<ToolActivityStep>
  steady?: Accessor<boolean>
  onOpenSubagentSession?: (sessionId: string) => void | Promise<void>
}) {
  const [open, setOpen] = createSignal(props.step().defaultOpen ?? false)
  const [userToggled, setUserToggled] = createSignal(false)
  const hasSubagents = () => Boolean(props.step().subagents?.length)
  const hasDetails = () => props.step().details.length > 0 || hasSubagents() || Boolean(props.step().preview)

  createEffect(() => {
    if (!userToggled() && props.step().defaultOpen) setOpen(true)
  })

  return (
    <div class="relative grid min-w-0 grid-cols-[1.7rem_minmax(0,1fr)] gap-3 py-1 before:absolute before:top-[1.35rem] before:-bottom-1 before:left-[0.85rem] before:w-px before:-translate-x-1/2 before:rounded-full before:bg-[var(--line-strong)] before:content-[''] last:before:hidden">
      <div class="relative grid h-[1.7rem] w-[1.7rem] place-items-center text-[var(--muted)]">
        <ToolIcon
          kind={props.step().icon}
          class={cx("relative z-[1] h-[1.08rem] w-[1.08rem]", toolStateClass(props.step().state))}
        />
      </div>
      <div class="min-w-0 pb-1">
        <button
          type="button"
          class="inline-flex min-w-0 max-w-full items-center gap-1.5 py-0.5 text-left text-[14px] font-medium leading-snug text-[#dedbd4] disabled:cursor-default [&[aria-expanded=true]_.tool-chevron]:rotate-180"
          disabled={!hasDetails()}
          aria-expanded={open()}
          onClick={() => {
            if (!hasDetails()) return
            setUserToggled(true)
            setOpen((value) => !value)
          }}
        >
          <span class="min-w-0 [overflow-wrap:anywhere]">{props.step().label}</span>
          <Show when={hasDetails()}>
            <IconCaretDown class="tool-chevron h-3 w-3 shrink-0 text-[var(--faint)] transition-transform duration-150" />
          </Show>
        </button>
        <Show when={hasDetails()}>
          <CollapsiblePanel open={open()} steady={props.steady?.()} class="w-full">
            <Show when={hasSubagents()}>
              <SubagentActivityDetails
                items={props.step().subagents || []}
                preview={props.step().preview}
                onOpenSession={props.onOpenSubagentSession}
              />
            </Show>
            <Show when={!hasSubagents()}>
              <ToolDetails details={props.step().details} preview={props.step().preview} compact />
            </Show>
          </CollapsiblePanel>
        </Show>
      </div>
    </div>
  )
}

function ToolActionMessage(props: { tool: Accessor<ToolMessage> }) {
  const descriptor = createMemo(() => actionDescriptor(props.tool()))
  const [open, setOpen] = createSignal(descriptor().state !== "complete")

  createEffect(() => {
    if (descriptor().state === "active" || descriptor().state === "error") setOpen(true)
  })

  return (
    <article class="py-1">
      <div class="w-[min(100%,44rem)] min-w-0">
        <section
          class={cx(
            "w-[min(100%,44rem)] overflow-hidden rounded-lg border border-[var(--line)] bg-white/[0.028]",
            descriptor().state === "active" && "border-[rgba(108,142,216,0.28)] bg-[rgba(108,142,216,0.055)]",
            descriptor().state === "error" && "border-[rgba(200,108,116,0.3)] bg-[rgba(200,108,116,0.07)]"
          )}
        >
          <button
            type="button"
            class="grid w-full min-w-0 grid-cols-[1.85rem_minmax(0,1fr)_auto_auto] items-center gap-3 px-3 py-3 text-left hover:bg-white/[0.035] [&[aria-expanded=true]_.tool-chevron]:rotate-180"
            aria-expanded={open()}
            onClick={() => setOpen((value) => !value)}
          >
            <span class="grid h-[1.85rem] w-[1.85rem] place-items-center rounded-md bg-white/[0.04] text-[var(--muted)]">
              <ToolIcon
                kind={descriptor().icon}
                class={cx("h-[1.05rem] w-[1.05rem]", toolStateClass(descriptor().state))}
              />
            </span>
            <span class="flex min-w-0 flex-col gap-0.5">
              <strong class="min-w-0 overflow-hidden text-ellipsis whitespace-nowrap text-[14px] font-semibold leading-tight text-[var(--text)]">
                {descriptor().label}
              </strong>
              <small class="min-w-0 overflow-hidden text-ellipsis whitespace-nowrap font-mono text-[0.72rem] leading-snug text-[var(--muted)]">
                {descriptor().description}
              </small>
            </span>
            <Show when={descriptor().stats}>
              {(stats) => (
                <span class="inline-flex items-center gap-1.5 whitespace-nowrap font-mono text-[0.73rem] font-semibold text-[var(--muted)]" aria-label="Diff summary">
                  <span class="text-[#7fc99e]">+{stats().added}</span>
                  <span class="text-[#da8b92]">-{stats().removed}</span>
                </span>
              )}
            </Show>
            <IconCaretDown class="tool-chevron h-3.5 w-3.5 text-[var(--faint)] transition-transform duration-150" />
          </button>
          <CollapsiblePanel open={open()} class="border-t border-[var(--line)]">
            <ToolDetails
              details={descriptor().details}
              preview={descriptor().preview}
              diffLines={descriptor().diffLines}
              diffSections={descriptor().diffSections}
            />
          </CollapsiblePanel>
        </section>
      </div>
    </article>
  )
}

function ToolDetails(props: {
  details: ToolStepDetail[]
  preview?: string
  diffLines?: DiffLine[]
  diffSections?: DiffSection[]
  compact?: boolean
}) {
  return (
    <div class={cx("flex min-w-0 flex-col gap-3 px-3 py-3", props.compact && "gap-2 px-0 pt-2 pb-1")}>
      <Show when={props.details.length > 0}>
        <div class="flex min-w-0 flex-col gap-2">
          <For each={props.details}>
            {(detail) => (
              <div class="grid min-w-0 grid-cols-[0.8rem_minmax(0,1fr)] items-start gap-2">
                <span
                  class={cx(
                    "mt-1.5 h-[0.42rem] w-[0.42rem] rounded-full border border-[var(--line-strong)] bg-white/10",
                    detail.status === "active" && "border-[rgba(108,142,216,0.65)] bg-[var(--brand-primary)] animate-toolPulse",
                    detail.status === "error" && "border-[rgba(200,108,116,0.65)] bg-[var(--red)]",
                    (detail.status === "complete" || !detail.status) && "border-[rgba(92,168,134,0.5)] bg-[var(--green)]"
                  )}
                  aria-hidden="true"
                />
                <span class="flex min-w-0 flex-col gap-0.5">
                  <strong class="min-w-0 text-[0.79rem] font-medium leading-snug text-[#d7d5d0] [overflow-wrap:anywhere]">
                    {detail.label}
                  </strong>
                  <Show when={detail.detail}>
                    {(detailText) => (
                      <small class="min-w-0 text-[0.73rem] leading-snug text-[var(--faint)] [overflow-wrap:anywhere]">
                        {detailText()}
                      </small>
                    )}
                  </Show>
                </span>
              </div>
            )}
          </For>
        </div>
      </Show>
      <Show when={props.diffLines && props.diffLines.length > 0}>
        <DiffPreview lines={props.diffLines || []} />
      </Show>
      <Show when={props.diffSections && props.diffSections.length > 0}>
        <div class="grid max-w-full gap-3 overflow-x-auto rounded-[7px] border border-[var(--line)] bg-black/20 p-3 font-mono text-[0.73rem] leading-normal text-[#bebbb4]" aria-label="Diff preview">
          <For each={props.diffSections}>
            {(section) => (
              <section class="min-w-max">
                <div class="mb-1.5 flex min-w-max items-center gap-2 text-[0.7rem] font-semibold text-[#d0a94f]">
                  <span class="h-px w-6 bg-[#d0a94f]/45" />
                  <span>{section.path}</span>
                  <span class="h-px flex-1 bg-[#d0a94f]/25" />
                </div>
                <DiffRows lines={section.lines} language={section.language} />
              </section>
            )}
          </For>
        </div>
      </Show>
      <Show when={props.preview}>
        {(preview) => (
          <pre class="m-0 max-w-full overflow-x-auto whitespace-pre rounded-[7px] border border-[var(--line)] bg-black/20 p-3 font-mono text-[0.73rem] leading-normal text-[#bebbb4]">
            {trimPreview(preview())}
          </pre>
        )}
      </Show>
    </div>
  )
}

function DiffPreview(props: { lines: DiffLine[]; language?: string }) {
  return (
    <div class="grid max-w-full gap-0.5 overflow-x-auto rounded-[7px] border border-[var(--line)] bg-black/20 p-3 font-mono text-[0.73rem] leading-normal text-[#bebbb4]" aria-label="Diff preview">
      <DiffRows lines={props.lines} language={props.language} />
    </div>
  )
}

function DiffRows(props: { lines: DiffLine[]; language?: string }) {
  return (
    <For each={props.lines}>
      {(line) => {
        const language = line.language || props.language
        return (
          <div
            class={cx(
              "grid min-w-max grid-cols-[2.4rem_1rem_minmax(0,1fr)] gap-2",
              line.kind === "add" && "bg-[#0c2613] text-[#8fd8aa]",
              line.kind === "remove" && "bg-[#2b1012] text-[#e09299]",
              line.kind === "context" && "text-[#8d8981]"
            )}
          >
            <span class="select-none text-right text-[var(--faint)]">{line.lineNumber ?? ""}</span>
            <span class="select-none text-[var(--faint)]">{line.kind === "add" ? "+" : line.kind === "remove" ? "-" : " "}</span>
            <code class="[font:inherit] text-inherit">
              <SyntaxText text={line.text} language={language} />
            </code>
          </div>
        )
      }}
    </For>
  )
}

function SyntaxText(props: { text: string; language?: string }) {
  const tokens = () => syntaxTokens(props.text, props.language)
  return (
    <>
      <For each={tokens()}>
        {(token) => <span class={syntaxTokenClass(token.kind)}>{token.text}</span>}
      </For>
    </>
  )
}

type SyntaxToken = { kind: "plain" | "keyword" | "string" | "comment" | "number" | "type"; text: string }

function syntaxTokens(text: string, language?: string): SyntaxToken[] {
  if (!language) return [{ kind: "plain", text }]
  const keywordPattern = language === "rust"
    ? /\b(?:async|await|break|const|continue|crate|else|enum|false|fn|for|if|impl|let|loop|match|mod|mut|pub|ref|return|self|Self|static|struct|super|trait|true|type|use|where|while)\b/g
    : /\b(?:as|async|await|break|case|catch|class|const|continue|default|else|export|extends|false|finally|for|from|function|if|import|in|interface|let|new|null|return|satisfies|switch|throw|true|try|type|typeof|undefined|var|while)\b/g
  const regex = new RegExp(`(//.*$|/\\*[\\s\\S]*?\\*/|"(?:\\\\.|[^"\\\\])*"|'(?:\\\\.|[^'\\\\])*'|\`(?:\\\\.|[^\`\\\\])*\`|\\b\\d+(?:\\.\\d+)?\\b|${keywordPattern.source}|\\b[A-Z][A-Za-z0-9_]*\\b)`, "g")
  const tokens: SyntaxToken[] = []
  let lastIndex = 0
  for (const match of text.matchAll(regex)) {
    const index = match.index ?? 0
    if (index > lastIndex) tokens.push({ kind: "plain", text: text.slice(lastIndex, index) })
    const value = match[0]
    const kind: SyntaxToken["kind"] = value.startsWith("//") || value.startsWith("/*")
      ? "comment"
      : value.startsWith("\"") || value.startsWith("'") || value.startsWith("`")
        ? "string"
        : /^\d/.test(value)
          ? "number"
          : /^[A-Z]/.test(value)
            ? "type"
            : "keyword"
    tokens.push({ kind, text: value })
    lastIndex = index + value.length
  }
  if (lastIndex < text.length) tokens.push({ kind: "plain", text: text.slice(lastIndex) })
  return tokens
}

function syntaxTokenClass(kind: SyntaxToken["kind"]) {
  if (kind === "keyword") return "text-[#8fb7ff]"
  if (kind === "string") return "text-[#d8bf7f]"
  if (kind === "comment") return "text-[#7f8a77]"
  if (kind === "number") return "text-[#c7a7e8]"
  if (kind === "type") return "text-[#8fd3d8]"
  return undefined
}

function ToolIcon(props: { kind: ToolIconKind; class?: string }) {
  if (props.kind === "agent") return <IconSubagentHierarchy class={props.class} />
  if (props.kind === "check") return <IconCheck class={props.class} />
  if (props.kind === "file") return <IconFileText class={props.class} />
  if (props.kind === "globe") return <IconGlobe class={props.class} />
  if (props.kind === "pencil") return <IconPencilSimple class={props.class} />
  if (props.kind === "search") return <IconSearch class={props.class} />
  if (props.kind === "terminal") return <IconTerminal class={props.class} />
  if (props.kind === "warning") return <IconWarningCircle class={props.class} />
  return <IconBrainGlyph class={props.class} />
}

function toolStateClass(state: ToolVisualState) {
  if (state === "active") return "text-[var(--brand-primary)] animate-toolPulse"
  if (state === "error") return "text-[var(--red)]"
  return "text-[#bcb9b1]"
}

function MessageView(props: {
  message: Accessor<RemoteMessage>
  activityTools: Accessor<ToolMessage[]>
  status: Accessor<RemoteStatus | null>
  streaming: Accessor<boolean>
  token: Accessor<string>
  onPreviewImage: (attachment: AttachmentData) => void
  onOpenSubagentSession?: (sessionId: string) => void | Promise<void>
}) {
  const isUser = () => props.message().role === "user"
  const userAttachments = createMemo(() => messageImageAttachmentData(props.message(), props.token()))
  const hasThoughtProcess = () =>
    Boolean(props.message().reasoning?.trim()) || props.activityTools().length > 0
  const visibleAssistantContent = () => assistantVisibleContent(props.message())
  const showAssistantBubble = () =>
    !isUser() &&
    (visibleAssistantContent().trim().length > 0 || showStreamingPlaceholder())
  const showStreamingPlaceholder = () =>
    !isUser() &&
    !props.message().is_complete &&
    !visibleAssistantContent().trim() &&
    props.activityTools().length === 0
  const showStreamingMetadataIndicator = () =>
    !isUser() &&
    props.streaming() &&
    !props.message().is_complete &&
    visibleAssistantContent().trim().length > 0
  const copyContent = () => (isUser() ? props.message().content : visibleAssistantContent()) || ""
  return (
    <Message from={props.message().role} class={cx(!isUser() && "w-full items-stretch")}>
      <MessageContent class={cx("w-full", isUser() && "flex flex-col items-end")}>
        <Show when={hasThoughtProcess()}>
          <ThinkingAccordion
            text={props.message().reasoning || ""}
            activityTools={props.activityTools}
            streaming={!props.message().is_complete}
            onOpenSubagentSession={props.onOpenSubagentSession}
          />
        </Show>
        <Show
          when={isUser()}
          fallback={
            <>
              <Show when={showAssistantBubble()}>
                <div class="mt-1 w-full whitespace-normal break-words pl-2 text-[0.95rem] leading-relaxed text-[#d7d5d0]">
                  <Show
                    when={showStreamingPlaceholder()}
                    fallback={<MessageResponse content={visibleAssistantContent()} />}
                  >
                    <Shimmer class="text-[0.76rem] font-medium leading-snug" duration={1.6}>
                      Working...
                    </Shimmer>
                  </Show>
                </div>
              </Show>
              <Show when={showAssistantBubble()}>
                <Show when={showStreamingMetadataIndicator()}>
                  <div class="mt-2 pl-2" aria-live="polite" aria-label="Assistant is working">
                    <Shimmer class="text-[0.76rem] font-medium leading-snug text-[#a4a8cc]" duration={1.6}>
                      Working...
                    </Shimmer>
                  </div>
                </Show>
                <MessageToolbar class="mt-2 w-full justify-start pl-2">
                  <AssistantMetadata message={props.message} status={props.status} />
                </MessageToolbar>
                <MessageActions class="mt-1">
                  <CopyMessageAction content={copyContent} />
                </MessageActions>
              </Show>
            </>
          }
        >
          <Show when={userAttachments().length > 0}>
            <Attachments
              variant="grid"
              class="ml-auto mt-1 !flex max-w-[min(100%,42rem)] flex-wrap justify-end gap-2"
            >
              <For each={userAttachments()}>
                {(attachment) => (
                  <Attachment
                    data={attachment}
                    class="w-[min(14rem,calc(100vw-2rem))] cursor-zoom-in transition hover:border-[rgba(255,255,255,0.16)] hover:bg-[#242424] focus-visible:border-[rgba(157,177,239,0.55)] focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-[rgba(157,177,239,0.18)]"
                    role="button"
                    tabIndex={0}
                    onClick={() => props.onPreviewImage(attachment)}
                    onKeyDown={(event) => handleImagePreviewKeyDown(event, () => props.onPreviewImage(attachment))}
                  >
                    <AttachmentPreview />
                    <div class="px-2 py-1.5">
                      <AttachmentInfo />
                    </div>
                  </Attachment>
                )}
              </For>
            </Attachments>
          </Show>
          <div class="ml-auto mt-1 w-fit max-w-[min(100%,42rem)] whitespace-pre-wrap break-words rounded-[12px_12px_4px_12px] border border-[var(--line)] bg-[#232323] px-3 py-2 text-[0.95rem] leading-relaxed text-[var(--text)]">
            <For each={promptTextParts(props.message().content || "Working...", userAttachments().length)}>
              {(part) => (
                <span
                  class={promptTextPartClass(part)}
                  style={promptTextPartStyle(part)}
                >
                  {part.text}
                </span>
              )}
            </For>
          </div>
          <MessageActions class="mt-1 justify-end">
            <CopyMessageAction content={copyContent} />
          </MessageActions>
        </Show>
      </MessageContent>
    </Message>
  )
}

export function ImagePreviewDialog(props: {
  image: Accessor<ImagePreviewTarget | null>
  onClose: () => void
}) {
  createEffect(() => {
    if (!props.image()) return

    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key !== "Escape") return
      event.preventDefault()
      props.onClose()
    }

    window.addEventListener("keydown", onKeyDown)
    onCleanup(() => window.removeEventListener("keydown", onKeyDown))
  })

  return (
    <Show when={props.image()}>
      {(image) => (
        <div
          class="fixed inset-0 z-[140] grid place-items-center bg-black/80 p-2 animate-fadeIn"
          role="dialog"
          aria-modal="true"
          aria-label={image().label}
          onMouseDown={(event) => event.currentTarget === event.target && props.onClose()}
        >
          <button
            class="absolute top-3 right-3 z-[1] grid h-8 w-8 place-items-center rounded-full bg-black/40 text-[#d9d7d0] backdrop-blur transition hover:bg-black/60 hover:text-white focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-white/30"
            type="button"
            aria-label="Close image preview"
            onClick={props.onClose}
          >
            <IconX class="h-4 w-4" />
          </button>
          <img
            src={image().url}
            alt={image().label}
            class="block max-h-[calc(100dvh-1rem)] max-w-[calc(100vw-1rem)] rounded-[6px] bg-[#101010] object-contain shadow-[0_1rem_4rem_rgba(0,0,0,0.55)]"
          />
        </div>
      )}
    </Show>
  )
}

function AssistantMetadata(props: {
  message: Accessor<RemoteMessage>
  status: Accessor<RemoteStatus | null>
}) {
  const agent = () => displayAgentMode(props.message().agent_mode || props.status()?.agent || "Build")
  const model = () => messageModelLabel(props.message(), props.status())
  const metrics = () => assistantMetrics(props.message())
  const accent = () => agentAccentClass(agent())

  return (
    <div class="flex min-w-0 flex-wrap items-center gap-x-2 gap-y-1 font-mono text-[0.76rem] leading-snug text-[#8d93bd]">
      <span class={cx("h-2.5 w-2.5 shrink-0 border", accent())} aria-hidden="true" />
      <span class={cx("font-bold", accent())}>{agent()}</span>
      <span class="text-[#686b86]">•</span>
      <span class="min-w-0 max-w-full overflow-hidden text-ellipsis whitespace-nowrap text-[#a4a8cc]">
        {model()}
      </span>
      <For each={metrics()}>
        {(metric) => (
          <>
            <span class="text-[#686b86]">•</span>
            <span class="text-[#8d93bd]">{metric}</span>
          </>
        )}
      </For>
    </div>
  )
}

function CopyMessageAction(props: { content: Accessor<string> }) {
  const [copied, setCopied] = createSignal(false)
  let timer: number | undefined

  const copy = async () => {
    const text = props.content()
    if (!text.trim()) return
    try {
      await navigator.clipboard.writeText(text)
    } catch {
      try {
        fallbackCopyText(text)
      } catch {
        // The visual acknowledgement still matters in restricted browser contexts.
      }
    }
    setCopied(true)
    if (timer) window.clearTimeout(timer)
    timer = window.setTimeout(() => setCopied(false), 1800)
  }

  onCleanup(() => {
    if (timer) window.clearTimeout(timer)
  })

  return (
    <MessageAction
      label={copied() ? "Copied" : "Copy"}
      disabled={!props.content().trim()}
      onClick={copy}
    >
      <Show
        when={copied()}
        fallback={<IconCopy class="h-3.5 w-3.5 animate-scaleIn" />}
      >
        <IconCheck class="h-3.5 w-3.5 animate-scaleIn text-[var(--green)]" />
      </Show>
    </MessageAction>
  )
}

function assistantVisibleContent(message: RemoteMessage) {
  const parts = Array.isArray(message.parts) ? message.parts : []
  if (parts.some((part) => part.type === "tool_call" || part.type === "tool_result")) {
    return parts
      .filter((part) => part.type === "text")
      .map((part) => (typeof part.text === "string" ? part.text.trim() : ""))
      .filter((text) => text && !text.trimStart().startsWith("[tool result:"))
      .join("\n\n")
  }

  return message.content || ""
}

function ThinkingAccordion(props: {
  text: string
  activityTools: Accessor<ToolMessage[]>
  streaming: boolean
  onOpenSubagentSession?: (sessionId: string) => void | Promise<void>
}) {
  const steps = createMemo(() => buildActivitySteps(props.activityTools()))
  const state = createMemo<ToolVisualState>(() => {
    if (steps().some((step) => step.state === "error")) return "error"
    if (steps().some((step) => step.state === "active")) return "active"
    return "complete"
  })
  const hasActivity = () => props.activityTools().length > 0
  const [open, setOpen] = createSignal(false)
  const [userToggled, setUserToggled] = createSignal(false)
  const steadyActivity = createMemo(() => props.streaming || (open() && hasActivity()))

  createEffect(() => {
    if (userToggled()) return
    if (props.streaming) {
      setOpen(true)
      return
    }
    if (state() === "active" || state() === "error") {
      setOpen(true)
    }
  })

  return (
    <div class="mt-2 w-[min(100%,42rem)] text-[#c9c6bf]">
      <button
        class="inline-flex min-h-[1.9rem] items-center gap-2 rounded-full px-2 py-1 text-[14px] font-medium text-[var(--muted)] transition hover:bg-white/[0.045] hover:text-[var(--text)] [&[aria-expanded=true]_.thinking-chevron]:rotate-180"
        type="button"
        aria-expanded={open()}
        onClick={() => {
          setUserToggled(true)
          setOpen((value) => !value)
        }}
      >
        <IconBrainGlyph class="h-4 w-4 text-[var(--faint)]" />
        <Show when={props.streaming} fallback={<span>Thought process</span>}>
          <Shimmer class="font-medium">Thinking...</Shimmer>
        </Show>
        <IconCaretDown class="thinking-chevron h-3 w-3 text-[var(--faint)] transition-transform duration-150" />
      </button>
      <CollapsiblePanel open={open()} steady={steadyActivity()} class="w-full">
        <div class="w-full overflow-x-auto pt-1 text-[14px] leading-relaxed text-[var(--muted)]">
          <Show when={props.text.trim()}>
            <StreamMarkdown
              content={props.text}
              class="streamdown remote-markdown text-[var(--muted)]"
              components={remoteMarkdownComponents}
            />
          </Show>
          <Show when={hasActivity()}>
            <div class="mt-1 [&_.tool-activity]:w-[min(100%,34rem)]">
              <ToolActivityTimeline
                steps={steps}
                state={state}
                steady={steadyActivity}
                onOpenSubagentSession={props.onOpenSubagentSession}
              />
            </div>
          </Show>
        </div>
      </CollapsiblePanel>
    </div>
  )
}
