import tailwindcss from "@tailwindcss/vite"
import { fileURLToPath } from "node:url"
import vike from "vike/plugin"
import vikeSolid from "vike-solid/vite"
import { defineConfig, loadEnv } from "vite"
import tsConfigPaths from "vite-tsconfig-paths"

const root = fileURLToPath(new URL(".", import.meta.url))

export default defineConfig(({ mode }) => {
  const env = loadEnv(mode, root, "")
  const remoteApiOrigin = env.CRABCODE_REMOTE_API_ORIGIN || "http://127.0.0.1:8421"

  return {
    root,
    build: {
      outDir: "dist/client",
    },
    server: {
      host: true,
      port: 4271,
      proxy: {
        "/api": {
          target: remoteApiOrigin,
          changeOrigin: true,
        },
      },
    },
    plugins: [tailwindcss(), tsConfigPaths(), vike(), vikeSolid()],
  }
})
