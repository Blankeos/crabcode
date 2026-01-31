// Generate Crabcode's built-in theme set from OpenCode's TUI themes.
// Run via: `bun run scripts/gen-themes.ts`

// @ts-nocheck

import { mkdirSync, rmSync, writeFileSync } from 'node:fs'
import { join } from 'node:path'

type GitHubFile = {
  name: string
  download_url?: string
}

const OPENCODE_REF = process.env.OPENCODE_REF ?? 'production'
const GITHUB_API_URL = `https://api.github.com/repos/anomalyco/opencode/contents/packages/opencode/src/cli/cmd/tui/context/theme?ref=${encodeURIComponent(
  OPENCODE_REF,
)}`
const THEMES_DIR = join(process.cwd(), 'src', 'generated_themes')

async function fetchThemes() {
  const response = await fetch(GITHUB_API_URL)
  if (!response.ok) {
    throw new Error(`Failed to fetch themes: ${response.status} ${response.statusText}`)
  }

  const files = (await response.json()) as GitHubFile[]

  rmSync(THEMES_DIR, { recursive: true, force: true })
  mkdirSync(THEMES_DIR, { recursive: true })

  for (const file of files) {
    if (!file?.name?.endsWith('.json')) continue
    if (!file.download_url) continue

    console.log(`Fetching ${file.name}...`)
    const themeResponse = await fetch(file.download_url)
    if (!themeResponse.ok) {
      console.error(
        `Failed to fetch ${file.name}: ${themeResponse.status} ${themeResponse.statusText}`,
      )
      continue
    }

    const themeContent = await themeResponse.text()
    const themePath = join(THEMES_DIR, file.name)
    writeFileSync(themePath, themeContent)
    console.log(`Saved ${file.name}`)
  }

  console.log(`\nDone! Themes saved to ${THEMES_DIR}`)
}

fetchThemes().catch((err) => {
  console.error(err)
  process.exitCode = 1
})
