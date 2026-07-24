import { For, Show, type Accessor } from "solid-js"
import type { RemoteSession } from "../../remote-api"
import { LOGO_ART } from "./ascii-art"
import { relativeTime } from "./shared-utils"

/** Longest line width of crabcode-logo.txt (used to fit ASCII to container). */
const LOGO_COLS = 126
const MASCOT_COLS = 48

export function EmptyThread(props: {
  projectName: string
  mascotFrame: string
  recentSessions: Accessor<RemoteSession[]>
  onSwitchSession: (id: string) => void | Promise<void>
}) {
  return (
    <div class="grid h-full min-h-0 w-full min-w-0 max-w-full place-items-center overflow-x-hidden text-center text-[var(--faint)]">
      <div class="box-border flex w-full min-w-0 max-w-full flex-col items-center gap-5 px-4">
        <div
          class="grid w-full min-w-0 max-w-3xl justify-items-center gap-4 overflow-hidden [container-type:inline-size]"
          aria-label={props.projectName}
        >
          <pre
            class="remote-empty-mascot m-0 min-w-0 max-w-full overflow-hidden whitespace-pre font-mono font-bold leading-none tracking-normal text-[var(--brand-primary)] [font-variant-ligatures:none]"
            style={{ "font-size": `min(1.08rem, calc(100cqi / ${MASCOT_COLS}))` }}
          >
            {props.mascotFrame}
          </pre>
          <pre
            class="remote-empty-logo m-0 min-w-0 max-w-full overflow-hidden whitespace-pre bg-[linear-gradient(180deg,var(--brand-primary)_0_63%,var(--brand-dim)_63%_100%)] bg-clip-text font-mono font-bold leading-none tracking-normal text-transparent [font-variant-ligatures:none]"
            style={{ "font-size": `min(0.92rem, calc(100cqi / ${LOGO_COLS}))` }}
          >
            {LOGO_ART}
          </pre>
        </div>
        <Show when={props.recentSessions().length > 0}>
          <div class="box-border flex w-full min-w-0 max-w-[min(24rem,100%)] flex-col gap-2 text-left">
            <For each={props.recentSessions()}>
              {(session) => (
                <button
                  type="button"
                  class="box-border flex w-full min-w-0 max-w-full items-center justify-between gap-3 overflow-hidden rounded-xl border border-white/[0.08] bg-white/[0.03] px-3.5 py-2.5 text-left transition hover:border-white/[0.14] hover:bg-white/[0.055]"
                  onClick={() => void props.onSwitchSession(session.id)}
                >
                  <span class="min-w-0 flex-1 truncate text-[0.9rem] font-medium text-[var(--text)]">
                    {session.title || "Untitled chat"}
                  </span>
                  <span class="shrink-0 text-[0.72rem] text-[var(--faint)]">
                    {relativeTime(session.updated_at)}
                  </span>
                </button>
              )}
            </For>
          </div>
        </Show>
      </div>
    </div>
  )
}
