import type { JSX } from "solid-js"
import { Toaster } from "solid-sonner"
import { useMetadata } from "vike-metadata-solid"

useMetadata.setGlobalDefaults({
  title: "CrabCode",
  icons: {
    icon: { url: "/favicon.png", type: "image/png" },
    shortcut: { url: "/favicon.png", type: "image/png" },
  },
})

export default function Layout(props: { children: JSX.Element }) {
  useMetadata({ title: "CrabCode" })
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
