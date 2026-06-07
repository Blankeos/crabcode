import tailwindcss from "@tailwindcss/vite"
import { fileURLToPath } from "node:url"
import vike from "vike/plugin"
import vikeSolid from "vike-solid/vite"
import { defineConfig } from "vite"
import tsConfigPaths from "vite-tsconfig-paths"

const root = fileURLToPath(new URL(".", import.meta.url))

export default defineConfig({
  root,
  build: {
    outDir: "dist/client",
  },
  plugins: [tailwindcss(), tsConfigPaths(), vike(), vikeSolid()],
})
