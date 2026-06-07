import { createEffect, createMemo, createSignal } from "solid-js"
import { cx } from "../../lib/cx"

const loadedProjectFaviconSrcs = new Set<string>()

type LoadStatus = "loading" | "loaded" | "error"

export function ProjectFavicon(props: { cwd: string; label: string; token?: string; class?: string }) {
  const src = createMemo(() => projectFaviconSrc(props.cwd, props.token))
  const [status, setStatus] = createSignal<LoadStatus>("loading")

  createEffect(() => {
    const next = src()
    setStatus(next && loadedProjectFaviconSrcs.has(next) ? "loaded" : "loading")
  })

  return (
    <>
      {status() !== "loaded" ? (
        <span
          class={cx(
            "grid h-[1.35rem] w-[1.35rem] shrink-0 place-items-center rounded-[0.42rem] bg-[#2a2a2a] text-[0.72rem] font-bold text-[#d6d4cf]",
            props.class
          )}
          aria-hidden="true"
        >
          {projectInitial(props.label)}
        </span>
      ) : null}
      {src() ? (
        <img
          src={src() || ""}
          alt=""
          class={cx(
            "h-[1.35rem] w-[1.35rem] shrink-0 rounded-[0.42rem] object-contain",
            status() === "loaded" ? "" : "hidden",
            props.class
          )}
          onLoad={() => {
            const currentSrc = src()
            if (!currentSrc) return
            loadedProjectFaviconSrcs.add(currentSrc)
            setStatus("loaded")
          }}
          onError={() => setStatus("error")}
        />
      ) : null}
    </>
  )
}

function projectInitial(label: string): string {
  return (label.trim()[0] || ".").toLowerCase()
}

function projectFaviconSrc(cwd: string, token: string | undefined): string | null {
  const projectCwd = cwd.trim()
  if (!projectCwd) return null

  const params = new URLSearchParams({ cwd: projectCwd })
  const authToken = token?.trim()
  if (authToken) params.set("token", authToken)

  return `/api/project-favicon?${params.toString()}`
}
