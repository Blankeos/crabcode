import { createMemo, Show } from "solid-js"
import { IconArrowLeft } from "../../icons"
import { cx } from "../../lib/cx"
import type { SubagentFooterController } from "./page-types"
import { agentAccentClass, displayAgentMode } from "./shared-utils"

export function SubagentSessionFooter(props: { footer: SubagentFooterController }) {
  const activeTab = createMemo(() => {
    const bundle = props.footer.tabs()
    if (!bundle) return null
    return bundle.tabs.find((tab) => tab.active) ?? bundle.tabs.find((tab) => tab.kind !== "main") ?? null
  })

  return (
    <div class="shrink-0 border-t border-[var(--line)] bg-[#111] px-4 py-2 pb-[max(0.5rem,env(safe-area-inset-bottom))]">
      <div class="mx-auto flex min-h-8 w-[min(100%,64rem)] items-center justify-between gap-3">
        <button
          type="button"
          class="inline-flex h-7 shrink-0 items-center gap-1.5 border border-[var(--line)] bg-[#171717] px-2.5 font-mono text-[0.7rem] font-medium text-[var(--muted)] transition hover:border-[var(--line-strong)] hover:bg-[#1d1d1d] hover:text-[var(--text)]"
          onClick={() => void props.footer.onBackToParent()}
        >
          <IconArrowLeft class="h-3.5 w-3.5" />
          Parent
        </button>

        <Show when={activeTab()}>
          {(tab) => (
            <div class="flex min-w-0 flex-1 items-center justify-end gap-2 border-l px-3 py-1" style={{ "border-left-color": tab().accent }}>
              <Show when={props.footer.streaming()}>
                <span class="h-1.5 w-1.5 shrink-0 rounded-full bg-[var(--brand-primary)] animate-toolPulse" aria-label="Subagent running" />
              </Show>
              <span class={cx("shrink-0 font-mono text-[0.72rem] font-bold", agentAccentClass(tab().agent))}>
                @{displayAgentMode(tab().agent)}
              </span>
              <Show when={tab().model}>
                {(model) => (
                  <span class="min-w-0 overflow-hidden text-ellipsis whitespace-nowrap font-mono text-[0.68rem] text-[var(--faint)]">
                    {model()}
                  </span>
                )}
              </Show>
            </div>
          )}
        </Show>
      </div>
    </div>
  )
}
