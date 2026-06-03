import { For, Show } from "solid-js"
import {
  Attachment,
  AttachmentInfo,
  AttachmentPreview,
  AttachmentRemove,
  Attachments,
} from "../../components/ai-elements/attachments"
import {
  IconBrainGlyph,
  IconIconFileCss,
  IconIconFileDefault,
  IconIconFileHtml,
  IconIconFileJs,
  IconIconFileJson,
  IconIconFileMarkdown,
  IconIconFileRust,
  IconIconFileToml,
  IconIconFileTs,
  IconIconFileTsx,
  IconIconFileYaml,
} from "../../assets/icons"
import { Popover, PopoverContent, PopoverTrigger } from "../../components/ui/popover"
import { IconArrowUp, IconCaretDown, IconCheck, IconFolder, IconPaperclip, IconTerminal } from "../../icons"
import { cx } from "../../lib/cx"
import type { RemoteModel, RemoteSuggestion } from "../../remote-api"
import { AGENT_MODES, COMPOSER_TEXT_CLASS, ICON_BUTTON, IMAGE_FILE_TYPES, MENU_ROW, MENU_ROW_ACTIVE, PANEL_BASE, POPOVER_ANIMATION } from "./page-constants"
import type { ComposerController } from "./page-types"
import { handleImagePreviewKeyDown, promptTextPartClass, promptTextParts, promptTextPartStyle } from "./prompt-utils"
import { QuestionRequestPanel, PermissionRequestPanel } from "./request-panels"
import { providerLabel, sameToken } from "./shared-utils"

export function ComposerDock(props: { composer: ComposerController }) {
  const composer = props.composer

  return (
    <div class="pointer-events-none absolute right-0 bottom-0 left-0 z-30 grid flex-none gap-3 px-4 pb-[max(1rem,env(safe-area-inset-bottom))] max-[900px]:px-3">
      <Show when={composer.pendingPermission()}>
        {(permission) => (
          <PermissionRequestPanel
            permission={permission()}
            busy={composer.permissionBusy()}
            onAnswer={composer.onAnswerPermission}
          />
        )}
      </Show>
      <Show when={!composer.pendingPermission() ? composer.pendingQuestion() : null}>
        {(question) => (
          <QuestionRequestPanel
            prompt={question()}
            busy={composer.questionBusy()}
            onSubmit={composer.onAnswerQuestion}
            onCancel={composer.onCancelQuestion}
          />
        )}
      </Show>
      <form
        class="pointer-events-auto relative mx-auto w-[min(100%,67rem)] overflow-visible rounded-[18px] border border-[var(--line-strong)] bg-[var(--composer)] shadow-[0_0.5rem_2.5rem_var(--shadow)]"
        onSubmit={composer.onSubmit}
        onDragOver={(event) => event.preventDefault()}
        onDrop={composer.onDrop}
      >
        <input
          ref={composer.setImageInputRef}
          class="hidden"
          type="file"
          accept={IMAGE_FILE_TYPES.join(",")}
          multiple
          onChange={(event) => {
            const files = Array.from(event.currentTarget.files ?? [])
            event.currentTarget.value = ""
            void composer.onAddImageFiles(files)
          }}
        />
        <Show when={composer.attachments().length > 0}>
          <div class="max-h-40 overflow-y-auto px-3 pt-3">
            <Attachments variant="grid" class="grid-cols-[repeat(auto-fill,minmax(8rem,1fr))]">
              <For each={composer.attachmentData()}>
                {(attachment) => (
                  <Attachment
                    data={attachment}
                    onRemove={() => composer.onRemoveAttachment(attachment.id)}
                    class="cursor-zoom-in transition hover:border-[rgba(255,255,255,0.16)] hover:bg-[#242424] focus-visible:border-[rgba(157,177,239,0.55)] focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-[rgba(157,177,239,0.18)]"
                    role="button"
                    tabIndex={0}
                    onClick={() => composer.onPreviewImage(attachment)}
                    onKeyDown={(event) => handleImagePreviewKeyDown(event, () => composer.onPreviewImage(attachment))}
                  >
                    <AttachmentPreview />
                    <div class="grid grid-cols-[minmax(0,1fr)_auto] items-center gap-1 px-2 py-1.5">
                      <AttachmentInfo />
                      <AttachmentRemove class="opacity-70 group-hover:opacity-100" />
                    </div>
                  </Attachment>
                )}
              </For>
            </Attachments>
          </div>
        </Show>
        <div class="relative">
          <div
            ref={composer.setPromptOverlayRef}
            class={cx(
              "pointer-events-none absolute inset-0 max-h-56 min-h-[4.9rem] overflow-hidden whitespace-pre-wrap break-words border-0 bg-transparent text-[var(--text)]",
              COMPOSER_TEXT_CLASS
            )}
            aria-hidden="true"
          >
            <For each={promptTextParts(composer.prompt(), composer.promptAttachmentCount())}>
              {(part) => (
                <span
                  class={promptTextPartClass(part)}
                  style={promptTextPartStyle(part)}
                >
                  {part.text}
                </span>
              )}
            </For>
            <span aria-hidden="true">&#8203;</span>
          </div>
          <textarea
            ref={composer.setPromptRef}
            class={cx(
              "relative z-10 block max-h-56 min-h-[4.9rem] w-full resize-none border-0 bg-transparent text-transparent caret-[var(--text)] outline-none placeholder:text-[#54524e] selection:bg-[rgba(126,157,234,0.28)]",
              COMPOSER_TEXT_CLASS
            )}
            value={composer.prompt()}
            onInput={composer.onPromptInput}
            onKeyDown={composer.onPromptKeyDown}
            onKeyUp={composer.onRefreshCompletion}
            onClick={composer.onRefreshCompletion}
            onScroll={composer.onPromptScroll}
            onPaste={composer.onPromptPaste}
            placeholder="Ask for follow-up changes or attach images"
            rows={1}
          />
        </div>
        <ComposerSuggestions composer={composer} />
        <div class="flex items-center justify-between gap-4 px-4 pt-2 pb-2.5">
          <div class="flex min-w-0 flex-1 items-center gap-3 max-[560px]:gap-2">
            <button
              class={cx(ICON_BUTTON, "h-[1.95rem] w-[1.95rem] shrink-0 border border-[var(--line)] bg-[#202020]")}
              type="button"
              aria-label="Attach image"
              title="Attach image"
              onClick={composer.openImageInput}
            >
              <IconPaperclip class="h-4 w-4" />
            </button>
            <ModelSelector composer={composer} />
            <AgentSelector composer={composer} />
            <ReasoningSelector composer={composer} />
          </div>
          <div class="flex min-w-0 items-center gap-3">
            <button
              class={cx(
                "grid h-11 w-11 place-items-center rounded-full transition shadow-[inset_0_0_0_1px_rgba(255,255,255,0.08)]",
                composer.streaming()
                  ? "bg-[#3c2528] text-[#d4929a] hover:bg-[#482b2f]"
                  : "bg-[var(--brand-primary)] text-[#111318] hover:bg-[#7d9dea]"
              )}
              type="submit"
              aria-label={composer.streaming() ? "Stop" : "Send"}
            >
              <Show
                when={composer.streaming()}
                fallback={<IconArrowUp class="h-[1.15rem] w-[1.15rem]" />}
              >
                <span class="h-3 w-3 rounded-[2px] bg-current" />
              </Show>
            </button>
          </div>
        </div>
      </form>
    </div>
  )
}

function ComposerSuggestions(props: { composer: ComposerController }) {
  const composer = props.composer

  return (
    <Show when={composer.suggestions().length > 0}>
      <div
        ref={composer.setSuggestionsRef}
        class="absolute right-4 bottom-[calc(100%+0.6rem)] left-4 max-h-[min(22rem,42vh)] overflow-auto rounded-[14px] border border-[var(--line-strong)] bg-[#171717] p-2 shadow-[0_1rem_2.4rem_var(--shadow)]"
        role="listbox"
      >
        <For each={composer.suggestions()}>
          {(suggestion, index) => (
            <button
              class={cx(
                "grid min-h-[3.05rem] w-full grid-cols-[1.7rem_minmax(0,1fr)] items-center gap-3 rounded-[9px] px-2 py-1.5 text-left text-[var(--text)] hover:bg-white/[0.07]",
                index() === composer.suggestionIndex() && "bg-white/[0.07]"
              )}
              type="button"
              role="option"
              aria-selected={index() === composer.suggestionIndex()}
              data-composer-suggestion-index={index()}
              onMouseEnter={() => composer.setSuggestionIndex(index())}
              onMouseDown={(event) => event.preventDefault()}
              onClick={() => composer.onChooseSuggestion(suggestion)}
            >
              <SuggestionIcon suggestion={suggestion} />
              <span class="flex min-w-0 flex-col gap-0.5">
                <span class="overflow-hidden text-ellipsis whitespace-nowrap text-[0.92rem] font-semibold text-[var(--text)]">
                  <span class="text-[var(--muted)]">{suggestionPrefix(suggestion)}</span>
                  {suggestion.name}
                </span>
                <Show when={suggestion.description}>
                  {(description) => (
                    <span class="overflow-hidden text-ellipsis whitespace-nowrap text-[0.78rem] text-[var(--muted)]">
                      {description()}
                    </span>
                  )}
                </Show>
              </span>
            </button>
          )}
        </For>
      </div>
    </Show>
  )
}

function ModelSelector(props: { composer: ComposerController }) {
  const composer = props.composer

  return (
    <div class="min-w-0 flex-[0_1_auto] max-[560px]:flex-1">
      <Popover open={composer.modelOpen()} onOpenChange={composer.onModelOpenChange} placement="top-start" gutter={10}>
        <PopoverTrigger
          as="button"
          class="inline-flex h-[1.95rem] max-w-[min(38vw,18rem)] min-w-0 items-center gap-2 rounded-[7px] border border-[var(--line)] bg-[#202020] px-2.5 text-[0.86rem] text-[var(--muted)] transition hover:bg-[#252525] hover:text-[var(--text)] focus-visible:bg-[#252525] focus-visible:text-[var(--text)] max-[900px]:max-w-[44vw] max-[560px]:max-w-full"
          type="button"
        >
          <IconBrainGlyph class="h-[1.05rem] w-[1.05rem] shrink-0" />
          <span class="min-w-0 overflow-hidden text-ellipsis whitespace-nowrap">{composer.modelLabel()}</span>
          <IconCaretDown class="h-3 w-3 shrink-0 text-[var(--faint)]" />
        </PopoverTrigger>
        <PopoverContent
          class={cx(
            PANEL_BASE,
            POPOVER_ANIMATION,
            "z-[90] grid max-h-[min(30rem,62vh)] w-[min(26rem,calc(100vw-1.4rem))] grid-rows-[auto_1fr] overflow-hidden"
          )}
          onCloseAutoFocus={composer.onControlPopoverCloseAutoFocus}
          onEscapeKeyDown={composer.onControlEscape}
        >
          <input
            ref={composer.setModelSearchRef}
            class="h-[2.55rem] w-full border-0 border-b border-[var(--line)] bg-transparent px-3 text-[var(--text)] outline-none"
            placeholder="Search models"
            value={composer.modelQuery()}
            onInput={(event) => composer.setModelQuery(event.currentTarget.value)}
            onKeyDown={composer.onModelSearchKeyDown}
            role="combobox"
            aria-expanded={composer.modelOpen()}
            aria-controls="model-listbox"
          />
          <div id="model-listbox" class="min-h-0 overflow-y-auto overscroll-contain p-2" role="listbox">
            <ModelList
              models={composer.filteredModels()}
              activeIndex={composer.modelActiveIndex()}
              onActiveIndex={composer.setModelActiveIndex}
              onSelect={composer.onSelectModel}
            />
          </div>
        </PopoverContent>
      </Popover>
    </div>
  )
}

function AgentSelector(props: { composer: ComposerController }) {
  const composer = props.composer

  return (
    <Popover open={composer.agentOpen()} onOpenChange={composer.onAgentOpenChange} placement="top-start" gutter={8}>
      <PopoverTrigger
        as="button"
        class="inline-flex h-[1.95rem] shrink-0 items-center justify-center gap-1.5 rounded-[7px] border border-[var(--line)] bg-[#202020] px-3 text-[0.78rem] font-semibold text-[var(--muted)] hover:bg-[#252525] hover:text-[var(--text)] max-[560px]:px-2"
        type="button"
        onKeyDown={composer.onAgentKeyDown}
      >
        <span>{composer.status()?.agent || "Build"}</span>
        <IconCaretDown class="h-3 w-3 text-[var(--faint)]" />
      </PopoverTrigger>
      <PopoverContent
        class={cx(PANEL_BASE, POPOVER_ANIMATION, "z-[90] min-w-36 overflow-hidden p-1")}
        tabIndex={-1}
        onCloseAutoFocus={composer.onControlPopoverCloseAutoFocus}
        onEscapeKeyDown={composer.onControlEscape}
        onKeyDown={composer.onAgentKeyDown}
      >
        <For each={AGENT_MODES}>
          {(agent, index) => (
            <button
              class={cx(
                MENU_ROW,
                (sameToken(agent, composer.status()?.agent || "Build") || index() === composer.agentActiveIndex()) &&
                  MENU_ROW_ACTIVE
              )}
              type="button"
              onClick={() => composer.onSelectAgentMode(agent)}
              onMouseEnter={() => composer.setAgentActiveIndex(index())}
            >
              <span>{agent}</span>
              <Show when={sameToken(agent, composer.status()?.agent || "Build")}>
                <IconCheck class="h-3.5 w-3.5 text-[var(--muted)]" />
              </Show>
            </button>
          )}
        </For>
      </PopoverContent>
    </Popover>
  )
}

function ReasoningSelector(props: { composer: ComposerController }) {
  const composer = props.composer

  return (
    <Show when={composer.reasoningOptions().length > 0}>
      <Popover open={composer.reasoningOpen()} onOpenChange={composer.onReasoningOpenChange} placement="top-start" gutter={8}>
        <PopoverTrigger
          as="button"
          class="inline-flex h-[1.95rem] min-w-[4.6rem] shrink-0 items-center justify-center gap-1.5 rounded-[7px] border border-[var(--line)] bg-[#202020] px-3 text-[0.78rem] font-semibold text-[var(--muted)] hover:bg-[#252525] hover:text-[var(--text)] max-[560px]:min-w-[3.9rem] max-[560px]:px-2"
          type="button"
          onKeyDown={composer.onReasoningKeyDown}
        >
          <span>{composer.reasoningLabel()}</span>
          <IconCaretDown class="h-3 w-3 text-[var(--faint)]" />
        </PopoverTrigger>
        <PopoverContent
          class={cx(PANEL_BASE, POPOVER_ANIMATION, "z-[90] min-w-36 overflow-hidden p-1")}
          tabIndex={-1}
          onCloseAutoFocus={composer.onControlPopoverCloseAutoFocus}
          onEscapeKeyDown={composer.onControlEscape}
          onKeyDown={composer.onReasoningKeyDown}
        >
          <For each={composer.reasoningOptions()}>
            {(effort, index) => (
              <button
                class={cx(
                  MENU_ROW,
                  (sameToken(effort, composer.reasoningLabel()) || index() === composer.reasoningActiveIndex()) &&
                    MENU_ROW_ACTIVE
                )}
                type="button"
                onClick={() => composer.onSelectReasoningEffort(effort)}
                onMouseEnter={() => composer.setReasoningActiveIndex(index())}
              >
                <span>{effort}</span>
                <Show when={sameToken(effort, composer.reasoningLabel())}>
                  <IconCheck class="h-3.5 w-3.5 text-[var(--muted)]" />
                </Show>
              </button>
            )}
          </For>
        </PopoverContent>
      </Popover>
    </Show>
  )
}

function ModelList(props: {
  models: RemoteModel[]
  activeIndex: number
  onActiveIndex: (index: number) => void
  onSelect: (model: RemoteModel) => void
}) {
  let group = ""
  return (
    <For each={props.models}>
      {(model, index) => {
        const showGroup = model.group !== group
        group = model.group
        return (
          <>
            <Show when={showGroup}>
              <div class="px-2 pt-3 pb-1 text-[0.66rem] font-bold uppercase tracking-[0.07em] text-[var(--faint)]">
                {model.group || "Models"}
              </div>
            </Show>
            <button
              class={cx(
                "flex min-h-[2.7rem] w-full items-center justify-between gap-3 rounded-lg px-2 py-2 text-left text-[var(--text)] transition hover:bg-[#2b2b2b]",
                props.activeIndex === index() && MENU_ROW_ACTIVE
              )}
              type="button"
              role="option"
              aria-selected={props.activeIndex === index()}
              onMouseEnter={() => props.onActiveIndex(index())}
              onClick={() => props.onSelect(model)}
            >
              <span class="flex min-w-0 flex-1 flex-col gap-0.5">
                <span class="flex min-w-0 items-center gap-2">
                  <span class="min-w-0 overflow-hidden text-ellipsis whitespace-nowrap text-[0.86rem] font-semibold">
                    {model.name || model.id}
                  </span>
                  <Show when={model.active}>
                    <span class="shrink-0 rounded-full border border-[rgba(92,168,134,0.35)] px-1.5 py-0.5 text-[0.64rem] font-bold text-[var(--green)]">
                      Active
                    </span>
                  </Show>
                </span>
                <span class="block min-w-0 overflow-hidden text-ellipsis whitespace-nowrap text-[0.72rem] text-[var(--faint)]">
                  {providerLabel(model)}
                </span>
              </span>
            </button>
          </>
        )
      }}
    </For>
  )
}

function SuggestionIcon(props: { suggestion: RemoteSuggestion }) {
  const suggestion = props.suggestion
  if (suggestion.kind === "command") return <IconTerminal class="h-[1.35rem] w-[1.35rem] text-[var(--muted)]" />
  if (suggestion.kind === "agent") return <IconBrainGlyph class="h-[1.35rem] w-[1.35rem] text-[#d9a6ff]" />
  if (suggestion.is_directory) return <IconFolder class="h-[1.35rem] w-[1.35rem] text-[#85827a]" />

  const FileIcon = iconForFile(suggestion.name)
  return <FileIcon class="h-[1.35rem] w-[1.35rem]" />
}

function iconForFile(path: string) {
  const lower = path.toLowerCase()
  if (lower.endsWith(".rs")) return IconIconFileRust
  if (lower.endsWith(".tsx")) return IconIconFileTsx
  if (lower.endsWith(".ts")) return IconIconFileTs
  if (lower.endsWith(".jsx") || lower.endsWith(".js")) return IconIconFileJs
  if (lower.endsWith(".json") || lower.endsWith(".jsonc")) return IconIconFileJson
  if (lower.endsWith(".md") || lower.endsWith(".mdx")) return IconIconFileMarkdown
  if (lower.endsWith(".toml")) return IconIconFileToml
  if (lower.endsWith(".yaml") || lower.endsWith(".yml")) return IconIconFileYaml
  if (lower.endsWith(".css")) return IconIconFileCss
  if (lower.endsWith(".html") || lower.endsWith(".htm")) return IconIconFileHtml
  return IconIconFileDefault
}

function suggestionPrefix(suggestion: RemoteSuggestion) {
  if (suggestion.kind === "command") return "/"
  if (suggestion.kind === "agent") return "@"
  return ""
}
