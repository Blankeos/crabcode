import type { JSX } from "solid-js"
import { onMount } from "solid-js"
import { Toaster } from "solid-sonner"
import { useMetadata } from "vike-metadata-solid"
import { REMOTE_CLIENT_VIEWPORT } from "./viewport-meta"

useMetadata.setGlobalDefaults({
  title: "CrabCode",
  viewport: {
    width: "device-width",
    initialScale: 1,
    maximumScale: 1,
    userScalable: false,
    interactiveWidget: "resizes-content",
  },
  icons: {
    icon: { url: "/favicon.png", type: "image/png" },
    shortcut: { url: "/favicon.png", type: "image/png" },
  },
})

function syncViewportMeta() {
  let meta = document.querySelector('meta[name="viewport"]')
  if (!meta) {
    meta = document.createElement("meta")
    meta.setAttribute("name", "viewport")
    document.head.appendChild(meta)
  }
  meta.setAttribute("content", REMOTE_CLIENT_VIEWPORT)
}

export default function Layout(props: { children: JSX.Element }) {
  useMetadata({ title: "CrabCode" })

  onMount(() => {
    syncViewportMeta()
  })

  return (
    <>
      {props.children}
      <Toaster
        theme="dark"
        position="bottom-right"
        richColors
        toastOptions={{
          class:
            "rounded-xl border border-[var(--line-strong)] bg-[#202020] text-[var(--text)] shadow-[0_1rem_3rem_rgba(0,0,0,0.35)]",
          classes: {
            title: "text-[0.86rem] font-semibold",
            description: "text-[0.78rem] text-[var(--muted)]",
          },
        }}
      />
    </>
  )
}
