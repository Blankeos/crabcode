import { REMOTE_CLIENT_VIEWPORT } from "./viewport-meta"

/**
 * Prerender runs with `ssr: false`, so `useMetadata()` in +Layout never executes at
 * HTML build time. Vike only emits its default viewport unless we add tags here.
 */
export default function Head() {
  return (
    <>
      <meta name="viewport" content={REMOTE_CLIENT_VIEWPORT} />
      <link rel="icon" href="/favicon.png" type="image/png" />
      <link rel="shortcut icon" href="/favicon.png" type="image/png" />
    </>
  )
}