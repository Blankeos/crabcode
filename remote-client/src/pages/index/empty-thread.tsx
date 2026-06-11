import { LOGO_ART } from "./ascii-art"

export function EmptyThread(props: { projectName: string; mascotFrame: string }) {
  return (
    <div class="grid min-h-0 min-w-0 place-items-center overflow-hidden text-center text-[var(--faint)]">
      <div class="grid max-w-full justify-items-center gap-4 overflow-hidden" aria-label={props.projectName}>
        <pre class="remote-empty-mascot m-0 whitespace-pre font-mono text-[clamp(0.58rem,2.1vw,1.08rem)] font-bold leading-none tracking-normal text-[var(--brand-primary)] [font-variant-ligatures:none]">
          {props.mascotFrame}
        </pre>
        <pre class="remote-empty-logo m-0 whitespace-pre bg-[linear-gradient(180deg,var(--brand-primary)_0_63%,var(--brand-dim)_63%_100%)] bg-clip-text font-mono text-[clamp(0.52rem,2.55vw,1.22rem)] font-bold leading-none tracking-normal text-transparent [font-variant-ligatures:none]">
          {LOGO_ART}
        </pre>
      </div>
    </div>
  )
}
