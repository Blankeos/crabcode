import { createEffect, createMemo, createSignal, For, Show } from "solid-js"
import { IconBrainGlyph } from "../../assets/icons"
import { IconCheck, IconWarningCircle, IconX } from "../../icons"
import { cx } from "../../lib/cx"
import type { RemotePendingPermission, RemotePendingQuestion, RemoteQuestionItem } from "../../remote-api"
import { INPUT_BASE } from "./page-constants"
import type { RemotePermissionResponse } from "./page-types"

export function PermissionRequestPanel(props: {
  permission: RemotePendingPermission
  busy: boolean
  onAnswer: (response: RemotePermissionResponse) => void
}) {
  const command = () => props.permission.command || props.permission.target || ""
  const queuedText = () => (props.permission.queued_count > 0 ? `+${props.permission.queued_count} queued` : "")

  return (
    <section class="pointer-events-auto mx-auto grid max-h-[min(40vh,18rem)] w-[min(100%,67rem)] overflow-hidden rounded-[16px] border border-[#6f5128] bg-[#211c15]/95 shadow-[0_1rem_3rem_rgba(0,0,0,0.45)] backdrop-blur">
      <div class="grid gap-3 p-4">
        <div class="flex min-w-0 items-start justify-between gap-3">
          <div class="grid min-w-0 gap-1">
            <div class="flex min-w-0 items-center gap-2 text-[#e2b16f]">
              <IconWarningCircle class="h-4 w-4 shrink-0" />
              <h2 class="m-0 min-w-0 overflow-hidden text-ellipsis whitespace-nowrap text-[0.95rem] font-bold">
                Permission required
              </h2>
              <Show when={queuedText()}>
                {(text) => <span class="rounded-md bg-[#2e261b] px-1.5 py-0.5 text-[0.68rem] font-bold text-[#be9b70]">{text()}</span>}
              </Show>
            </div>
            <div class="min-w-0 overflow-hidden text-ellipsis whitespace-nowrap text-[0.78rem] text-[var(--muted)]">
              {props.permission.tool_id} / {props.permission.action}
            </div>
          </div>
          <div class="flex shrink-0 gap-2 max-[560px]:hidden">
            <PermissionActionButtons busy={props.busy} onAnswer={props.onAnswer} compact={false} />
          </div>
        </div>

        <div class="grid gap-2 rounded-[10px] border border-[rgba(255,255,255,0.06)] bg-[#171717] p-3">
          <div class="text-[0.82rem] leading-relaxed text-[var(--muted)]">{props.permission.reason}</div>
          <Show when={command()}>
            {(value) => (
              <div class="min-w-0 overflow-hidden text-ellipsis whitespace-nowrap font-mono text-[0.78rem] text-[var(--text)]">
                {value()}
              </div>
            )}
          </Show>
          <Show when={props.permission.workdir}>
            {(workdir) => (
              <div class="min-w-0 overflow-hidden text-ellipsis whitespace-nowrap font-mono text-[0.7rem] text-[var(--faint)]">
                {workdir()}
              </div>
            )}
          </Show>
        </div>

        <div class="hidden gap-2 max-[560px]:grid">
          <PermissionActionButtons busy={props.busy} onAnswer={props.onAnswer} compact />
        </div>
      </div>
    </section>
  )
}

function PermissionActionButtons(props: {
  busy: boolean
  compact: boolean
  onAnswer: (response: RemotePermissionResponse) => void
}) {
  return (
    <>
      <button
        class={cx(
          "inline-flex h-9 items-center justify-center gap-1.5 rounded-lg border border-[#334d36] bg-[#1d2a1f] px-3 text-[0.82rem] font-bold text-[#a9d6ac] transition hover:bg-[#243326] disabled:cursor-not-allowed disabled:opacity-55",
          props.compact && "w-full"
        )}
        type="button"
        disabled={props.busy}
        onClick={() => props.onAnswer("allow_once")}
      >
        <IconCheck class="h-4 w-4" />
        <span>Allow once</span>
      </button>
      <button
        class={cx(
          "inline-flex h-9 items-center justify-center gap-1.5 rounded-lg border border-[#2d455f] bg-[#1a2530] px-3 text-[0.82rem] font-bold text-[#9ec8ef] transition hover:bg-[#202d3a] disabled:cursor-not-allowed disabled:opacity-55",
          props.compact && "w-full"
        )}
        type="button"
        disabled={props.busy}
        onClick={() => props.onAnswer("allow_always")}
      >
        <IconCheck class="h-4 w-4" />
        <span>Always</span>
      </button>
      <button
        class={cx(
          "inline-flex h-9 items-center justify-center gap-1.5 rounded-lg border border-[#553238] bg-[#2a1c1f] px-3 text-[0.82rem] font-bold text-[#dc9aa2] transition hover:bg-[#332226] disabled:cursor-not-allowed disabled:opacity-55",
          props.compact && "w-full"
        )}
        type="button"
        disabled={props.busy}
        onClick={() => props.onAnswer("deny")}
      >
        <IconX class="h-4 w-4" />
        <span>Reject</span>
      </button>
    </>
  )
}

export function QuestionRequestPanel(props: {
  prompt: RemotePendingQuestion
  busy: boolean
  onSubmit: (answers: string[][]) => void
  onCancel: () => void
}) {
  const [selected, setSelected] = createSignal<string[][]>([])
  const [customAnswers, setCustomAnswers] = createSignal<string[]>([])
  let lastPromptKey = ""

  const promptKey = () =>
    props.prompt.questions
      .map((question) =>
        [
          question.header,
          question.question,
          question.multiple ? "multiple" : "single",
          question.custom ? "custom" : "fixed",
          question.options.map((option) => `${option.label}:${option.description}`).join("|"),
        ].join("\u0000")
      )
      .join("\u0001")

  createEffect(() => {
    const key = promptKey()
    if (key === lastPromptKey) return
    lastPromptKey = key
    setSelected(props.prompt.questions.map(() => []))
    setCustomAnswers(props.prompt.questions.map(() => ""))
  })

  const toggleOption = (questionIndex: number, question: RemoteQuestionItem, label: string) => {
    setSelected((current) => {
      const next = current.map((items) => [...items])
      const values = next[questionIndex] ?? []
      if (question.multiple) {
        next[questionIndex] = values.includes(label)
          ? values.filter((item) => item !== label)
          : [...values, label]
      } else {
        next[questionIndex] = values.includes(label) ? [] : [label]
      }
      return next
    })
    if (!question.multiple) {
      setCustomAnswers((current) => current.map((value, index) => (index === questionIndex ? "" : value)))
    }
  }

  const updateCustomAnswer = (questionIndex: number, value: string, question: RemoteQuestionItem) => {
    setCustomAnswers((current) => current.map((item, index) => (index === questionIndex ? value : item)))
    if (!question.multiple && value.trim()) {
      setSelected((current) => current.map((items, index) => (index === questionIndex ? [] : items)))
    }
  }

  const answers = createMemo(() =>
    props.prompt.questions.map((question, index) => {
      const custom = (customAnswers()[index] ?? "").trim()
      if (!question.multiple && custom) return [custom]
      const values = [...(selected()[index] ?? [])]
      if (custom) values.push(custom)
      return values
    })
  )

  const canSubmit = createMemo(() => answers().every((answer) => answer.length > 0))
  const queuedText = () => (props.prompt.queued_count > 0 ? `+${props.prompt.queued_count} queued` : "")

  const submit = (event: SubmitEvent) => {
    event.preventDefault()
    if (!canSubmit() || props.busy) return
    props.onSubmit(answers())
  }

  return (
    <form
      class="pointer-events-auto mx-auto grid max-h-[min(48vh,26rem)] w-[min(100%,67rem)] overflow-hidden rounded-[16px] border border-[#33475f] bg-[#171d24]/95 shadow-[0_1rem_3rem_rgba(0,0,0,0.45)] backdrop-blur"
      onSubmit={submit}
    >
      <div class="flex min-w-0 items-center justify-between gap-3 border-b border-[rgba(255,255,255,0.07)] px-4 py-3">
        <div class="flex min-w-0 items-center gap-2 text-[#9ec8ef]">
          <IconBrainGlyph class="h-4 w-4 shrink-0" />
          <h2 class="m-0 min-w-0 overflow-hidden text-ellipsis whitespace-nowrap text-[0.95rem] font-bold">
            Agent needs input
          </h2>
          <Show when={queuedText()}>
            {(text) => <span class="rounded-md bg-[#202a34] px-1.5 py-0.5 text-[0.68rem] font-bold text-[#94b8d8]">{text()}</span>}
          </Show>
        </div>
        <button
          class="inline-flex h-8 items-center justify-center gap-1.5 rounded-lg border border-[#553238] bg-[#261c1f] px-2.5 text-[0.78rem] font-bold text-[#dc9aa2] transition hover:bg-[#302125] disabled:cursor-not-allowed disabled:opacity-55"
          type="button"
          disabled={props.busy}
          onClick={props.onCancel}
        >
          <IconX class="h-3.5 w-3.5" />
          <span class="max-[560px]:hidden">Cancel run</span>
        </button>
      </div>

      <div class="min-h-0 overflow-y-auto px-4 py-3">
        <div class="grid gap-4">
          <For each={props.prompt.questions}>
            {(question, questionIndex) => (
              <fieldset class="grid min-w-0 gap-2 rounded-[10px] border border-[rgba(255,255,255,0.06)] bg-[#151515] p-3">
                <legend class="px-1 text-[0.72rem] font-bold uppercase tracking-[0.07em] text-[var(--faint)]">
                  {question.header || `Question ${questionIndex() + 1}`}
                </legend>
                <div class="text-[0.92rem] leading-relaxed text-[var(--text)]">{question.question}</div>
                <Show when={question.multiple}>
                  <div class="text-[0.74rem] text-[var(--faint)]">Choose one or more.</div>
                </Show>

                <Show when={question.options.length > 0}>
                  <div class="grid gap-1.5">
                    <For each={question.options}>
                      {(option) => {
                        const checked = createMemo(() => (selected()[questionIndex()] ?? []).includes(option.label))
                        return (
                          <label class="grid min-w-0 cursor-pointer grid-cols-[auto_minmax(0,1fr)] items-start gap-2 rounded-[8px] px-2 py-1.5 text-[var(--muted)] hover:bg-white/[0.045]">
                            <input
                              class="mt-1 accent-[#9ec8ef]"
                              type={question.multiple ? "checkbox" : "radio"}
                              name={`remote-question-${questionIndex()}`}
                              checked={checked()}
                              onChange={() => toggleOption(questionIndex(), question, option.label)}
                            />
                            <span class="grid min-w-0 gap-0.5">
                              <span class="min-w-0 overflow-hidden text-ellipsis whitespace-nowrap text-[0.86rem] font-semibold text-[var(--text)]">
                                {option.label}
                              </span>
                              <Show when={option.description}>
                                <span class="text-[0.76rem] leading-snug text-[var(--faint)]">{option.description}</span>
                              </Show>
                            </span>
                          </label>
                        )
                      }}
                    </For>
                  </div>
                </Show>

                <Show when={question.custom}>
                  <input
                    class={cx(INPUT_BASE, "h-10 text-[0.86rem]")}
                    value={customAnswers()[questionIndex()] ?? ""}
                    onInput={(event) => updateCustomAnswer(questionIndex(), event.currentTarget.value, question)}
                    placeholder={question.options.length > 0 ? "Or type your own answer" : "Type your answer"}
                  />
                </Show>
              </fieldset>
            )}
          </For>
        </div>
      </div>

      <div class="flex items-center justify-end gap-2 border-t border-[rgba(255,255,255,0.07)] px-4 py-3">
        <button
          class="h-9 rounded-lg bg-[#e5e2dc] px-4 text-[0.84rem] font-bold text-[#171717] transition hover:bg-[#f0ede7] disabled:cursor-not-allowed disabled:opacity-45"
          type="submit"
          disabled={!canSubmit() || props.busy}
        >
          Submit answer
        </button>
      </div>
    </form>
  )
}
