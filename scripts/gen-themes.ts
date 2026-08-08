// Generate Crabcode's built-in theme set from OpenCode's TUI themes.
// Run via: `bun run scripts/gen-themes.ts`

// @ts-nocheck

import { mkdirSync, rmSync, writeFileSync } from 'node:fs'
import { join } from 'node:path'

type GitHubFile = {
  name: string
  download_url?: string
}

type ThemeMode = 'dark' | 'light'

const OPENCODE_REF = process.env.OPENCODE_REF ?? 'production'
const GITHUB_API_URL = `https://api.github.com/repos/anomalyco/opencode/contents/packages/tui/src/theme/assets?ref=${encodeURIComponent(
  OPENCODE_REF,
)}`
const THEMES_DIR = join(process.cwd(), 'src', 'generated_themes')
const PLACEHOLDER_CONTRAST_RATIO = 0.62

function parseHex(hex: string): [number, number, number] | undefined {
  const h = hex.replace('#', '').trim()
  if (!/^[0-9a-fA-F]{6}$/.test(h)) return undefined
  return [parseInt(h.slice(0, 2), 16), parseInt(h.slice(2, 4), 16), parseInt(h.slice(4, 6), 16)]
}

function toHex(r: number, g: number, b: number): string {
  return `#${[r, g, b]
    .map((c) => Math.round(Math.max(0, Math.min(255, c))).toString(16).padStart(2, '0'))
    .join('')}`
}

function blendToward(hex: string, target: string, amount: number): string | undefined {
  const sourceRgb = parseHex(hex)
  const targetRgb = parseHex(target)
  if (!sourceRgb || !targetRgb) return undefined

  const [r1, g1, b1] = sourceRgb
  const [r2, g2, b2] = targetRgb
  return toHex(r1 + (r2 - r1) * amount, g1 + (g2 - g1) * amount, b1 + (b2 - b1) * amount)
}

function luminance(hex: string): number | undefined {
  const rgb = parseHex(hex)
  if (!rgb) return undefined

  const [r, g, b] = rgb.map((c) => {
    const s = c / 255
    return s <= 0.03928 ? s / 12.92 : Math.pow((s + 0.055) / 1.055, 2.4)
  })
  return 0.2126 * r + 0.7152 * g + 0.0722 * b
}

function contrastRatio(a: string, b: string): number | undefined {
  const aLum = luminance(a)
  const bLum = luminance(b)
  if (aLum === undefined || bLum === undefined) return undefined

  const lighter = Math.max(aLum, bLum)
  const darker = Math.min(aLum, bLum)
  return (lighter + 0.05) / (darker + 0.05)
}

function resolveToHex(defs: Record<string, string>, theme: Record<string, unknown>, value: string): string {
  const trimmed = value.trim()
  if (trimmed.startsWith('#')) return trimmed
  if (defs[trimmed]) return defs[trimmed]

  const indirect = theme[trimmed]
  if (typeof indirect === 'string') return resolveToHex(defs, theme, indirect)

  return trimmed
}

function getModeValue(entry: unknown, mode: ThemeMode): string | undefined {
  if (typeof entry === 'string') return entry
  if (entry && typeof entry === 'object' && mode in entry) return (entry as Record<ThemeMode, string>)[mode]
  return undefined
}

function safeDefName(ref: string, mode: ThemeMode): string {
  return `${ref.replace(/[^a-zA-Z0-9_]/g, '') || mode}Weak`
}

function insertThemeKeyAfter(
  theme: Record<string, unknown>,
  afterKey: string,
  newKey: string,
  value: unknown,
): Record<string, unknown> {
  if (newKey in theme) {
    theme[newKey] = value
    return theme
  }

  const reordered: Record<string, unknown> = {}
  let inserted = false

  for (const [key, existingValue] of Object.entries(theme)) {
    reordered[key] = existingValue
    if (key === afterKey) {
      reordered[newKey] = value
      inserted = true
    }
  }

  if (!inserted) reordered[newKey] = value
  return reordered
}

/**
 * Placeholder text should sit clearly below real input text. Upstream themes only
 * define `textMuted`, which is useful throughout the UI but not always subdued
 * enough for placeholder copy. Generate a dedicated `textWeak` token for this.
 */
function injectTextWeak(themeJson: Record<string, unknown>) {
  if (!themeJson.theme || !themeJson.defs) return

  let theme = themeJson.theme as Record<string, unknown>
  const defs = themeJson.defs as Record<string, string>
  if (!theme.text || !theme.textMuted) return

  const textWeak: Record<ThemeMode, string> = { dark: '', light: '' }

  for (const mode of ['dark', 'light'] as const) {
    const textRef = getModeValue(theme.text, mode)
    const mutedRef = getModeValue(theme.textMuted, mode)
    const backgroundRef = getModeValue(theme.backgroundElement, mode) ?? getModeValue(theme.background, mode)
    if (!textRef || !mutedRef || !backgroundRef) return

    const textHex = resolveToHex(defs, theme, textRef)
    const mutedHex = resolveToHex(defs, theme, mutedRef)
    const backgroundHex = resolveToHex(defs, theme, backgroundRef)
    const textContrast = contrastRatio(textHex, backgroundHex)
    if (textContrast === undefined) return

    const targetContrast = textContrast * PLACEHOLDER_CONTRAST_RATIO
    let weakHex = mutedHex

    for (let amount = 0; amount <= 0.9; amount += 0.1) {
      const candidate = amount === 0 ? mutedHex : blendToward(mutedHex, backgroundHex, amount)
      if (!candidate) break

      const candidateContrast = contrastRatio(candidate, backgroundHex)
      if (candidateContrast !== undefined && candidateContrast <= targetContrast) {
        weakHex = candidate
        break
      }
    }

    let weakKey = safeDefName(mutedRef, mode)
    if (defs[weakKey] && defs[weakKey] !== weakHex) weakKey = `${weakKey}${mode === 'dark' ? 'Dark' : 'Light'}`
    defs[weakKey] = weakHex
    textWeak[mode] = weakKey
  }

  theme = insertThemeKeyAfter(theme, 'textMuted', 'textWeak', textWeak)
  themeJson.theme = theme
}

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

    try {
      const themeJson = JSON.parse(themeContent) as Record<string, unknown>
      injectTextWeak(themeJson)
      writeFileSync(themePath, JSON.stringify(themeJson, null, 2) + '\n')
    } catch {
      writeFileSync(themePath, themeContent)
    }

    console.log(`Saved ${file.name}`)
  }

  console.log(`\nDone! Themes saved to ${THEMES_DIR}`)
}

fetchThemes().catch((err) => {
  console.error(err)
  process.exitCode = 1
})
