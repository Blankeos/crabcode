import type { Config } from "vike/types"
import vikeSolid from "vike-solid/config"

export default {
  extends: [vikeSolid],
  ssr: false,
  prerender: true,
  // We emit the full viewport meta from +Head.tsx (vike-solid only supports width/initial-scale).
  viewport: null,
} satisfies Config
