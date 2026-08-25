// Generate Crabcode's built-in theme set from OpenCode's TUI themes.
// Run via: `bun run scripts/gen-themes.ts`

// @ts-nocheck

import { mkdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs'
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

/**
 * Solid backgrounds by default. Upstream themes sometimes set `background` to
 * "transparent" or semi-transparent `#rrggbbaa`; replace / strip so the TUI
 * paints a real opaque bg. Users can still enable transparency at runtime via
 * /themes (ctrl+t).
 */
function solidifyBackground(themeJson: Record<string, unknown>) {
  if (!themeJson.theme || !themeJson.defs) return

  const theme = themeJson.theme as Record<string, unknown>
  const defs = themeJson.defs as Record<string, string>
  const bg = theme.background
  if (bg === undefined) return

  const isTransparent = (v: unknown) =>
    typeof v === 'string' && v.trim().toLowerCase() === 'transparent'

  const stripAlpha = (hex: string): string => {
    const h = hex.trim()
    if (/^#[0-9a-fA-F]{8}$/.test(h)) return h.slice(0, 7)
    if (/^#[0-9a-fA-F]{4}$/.test(h)) return `#${h[1]}${h[1]}${h[2]}${h[2]}${h[3]}${h[3]}`
    return h
  }

  const solidForMode = (mode: ThemeMode): string => {
    const panel = getModeValue(theme.backgroundPanel, mode)
    const menu = getModeValue(theme.backgroundMenu, mode)
    const element = getModeValue(theme.backgroundElement, mode)
    for (const ref of [panel, menu, element]) {
      if (!ref || isTransparent(ref)) continue
      const hex = resolveToHex(defs, theme, ref)
      if (hex.startsWith('#')) return stripAlpha(hex)
    }
    return mode === 'dark' ? '#0d0d0d' : '#fafafa'
  }

  const solidifyValue = (v: unknown, mode: ThemeMode): string => {
    if (typeof v !== 'string' || isTransparent(v)) return solidForMode(mode)
    if (v.startsWith('#')) return stripAlpha(v)
    // def ref — resolve, strip alpha, keep as hex so we don't mutate shared defs
    const hex = resolveToHex(defs, theme, v)
    if (hex.startsWith('#')) return stripAlpha(hex)
    return solidForMode(mode)
  }

  if (typeof bg === 'string') {
    theme.background = {
      dark: solidifyValue(bg, 'dark'),
      light: solidifyValue(bg, 'light'),
    }
    return
  }

  if (bg && typeof bg === 'object') {
    const dual = bg as Record<string, unknown>
    for (const mode of ['dark', 'light'] as const) {
      if (dual[mode] !== undefined) {
        dual[mode] = solidifyValue(dual[mode], mode)
      } else {
        dual[mode] = solidForMode(mode)
      }
    }
    theme.background = dual
  }
}

/**
 * Tag each theme as "dark" or "light" based on its primary (dark-mode) background
 * luminance. Searchable in /themes.
 */
function injectAppearance(themeJson: Record<string, unknown>) {
  if (typeof themeJson.appearance === 'string') return
  if (!themeJson.theme || !themeJson.defs) {
    themeJson.appearance = 'dark'
    return
  }

  const theme = themeJson.theme as Record<string, unknown>
  const defs = themeJson.defs as Record<string, string>
  const bgRef =
    getModeValue(theme.background, 'dark') ??
    getModeValue(theme.backgroundPanel, 'dark') ??
    getModeValue(theme.backgroundMenu, 'dark')
  if (!bgRef || bgRef.toLowerCase() === 'transparent') {
    themeJson.appearance = 'dark'
    return
  }
  const hex = resolveToHex(defs, theme, bgRef)
  const lum = luminance(hex)
  themeJson.appearance = lum !== undefined && lum > 0.5 ? 'light' : 'dark'
}

/** True when theme.background exposes a distinct light Mode hex. */
function hasLightMode(themeJson: Record<string, unknown>): boolean {
  const theme = themeJson.theme
  if (!theme || typeof theme !== 'object') return false
  const background = (theme as Record<string, unknown>).background
  return (
    !!background &&
    typeof background === 'object' &&
    typeof (background as Record<string, unknown>).light === 'string'
  )
}

/**
 * Some OpenCode themes declare `{ dark, light }` Mode objects but use the same
 * values for both (e.g. aura, nightowl). Emitting a *-light sibling would be a
 * duplicate — skip those.
 */
function hasDistinctLightPalette(themeJson: Record<string, unknown>): boolean {
  const theme = themeJson.theme
  if (!theme || typeof theme !== 'object') return false

  let compared = 0
  for (const value of Object.values(theme as Record<string, unknown>)) {
    if (!value || typeof value !== 'object') continue
    const mode = value as Record<string, unknown>
    if (typeof mode.dark !== 'string' || typeof mode.light !== 'string') continue
    compared++
    if (mode.dark !== mode.light) return true
  }
  // No Mode slots, or every Mode slot is identical dark===light.
  return false
}

/**
 * Dual-mode OpenCode themes keep both palettes. Emit a sibling `{id}-light.json`
 * with `appearance: "light"` so `/themes` can filter/select the light palette
 * as its own entry (dark sibling stays the default).
 *
 * Skipped when light Mode is missing or identical to dark (fake dual-mode).
 */
function makeLightSibling(
  themeJson: Record<string, unknown>,
  baseId: string,
): Record<string, unknown> | undefined {
  if (!hasLightMode(themeJson) || !hasDistinctLightPalette(themeJson)) {
    return undefined
  }
  // Keep dual Mode slots; runtime picks light via appearance → dark_mode=false.
  const sibling = structuredClone(themeJson) as Record<string, unknown>
  sibling.id = `${baseId}-light`
  sibling.appearance = 'light'
  if (typeof sibling.name === 'string' && sibling.name.length > 0) {
    sibling.name = `${sibling.name} Light`
  } else {
    // Title-case the id for nicer /themes labels (e.g. "Github Light").
    sibling.name =
      baseId
        .split('-')
        .map((part) => part.charAt(0).toUpperCase() + part.slice(1))
        .join(' ') + ' Light'
  }
  return sibling
}

function writeBundledThemesList(generatedIds: string[]) {
  // Hand-authored themes live in src/themes/; generated ones in src/generated_themes/.
  const handAuthored: Array<{ id: string; path: string }> = [
    { id: 'crabcode-orange', path: 'themes/crabcode-orange.json' },
    { id: 'groknight', path: 'themes/groknight.json' },
    { id: 'grokday', path: 'themes/grokday.json' },
  ]

  const formatEntry = (id: string, path: string) => {
    if (id.includes('-')) {
      return `    (\n        "${id}",\n        include_str!("${path}"),\n    ),`
    }
    return `    ("${id}", include_str!("${path}")),`
  }

  const entries = [
    ...handAuthored.map(({ id, path }) => formatEntry(id, path)),
    ...generatedIds.map((id) => formatEntry(id, `generated_themes/${id}.json`)),
  ].join('\n')

  const themeRsPath = join(process.cwd(), 'src', 'theme.rs')
  const themeRs = readFileSync(themeRsPath, 'utf8')
  const start = 'const BUNDLED_THEMES: &[(&str, &str)] = &['
  const end = '];'
  const startIdx = themeRs.indexOf(start)
  if (startIdx < 0) throw new Error('BUNDLED_THEMES start marker not found in src/theme.rs')
  const endIdx = themeRs.indexOf(end, startIdx)
  if (endIdx < 0) throw new Error('BUNDLED_THEMES end marker not found in src/theme.rs')

  const next =
    themeRs.slice(0, startIdx) +
    start +
    '\n' +
    entries +
    '\n' +
    end +
    themeRs.slice(endIdx + end.length)
  writeFileSync(themeRsPath, next)
  console.log(
    `Updated BUNDLED_THEMES in src/theme.rs (${handAuthored.length + generatedIds.length} themes)`,
  )
}

async function fetchThemes() {
  const response = await fetch(GITHUB_API_URL)
  if (!response.ok) {
    throw new Error(`Failed to fetch themes: ${response.status} ${response.statusText}`)
  }

  const files = (await response.json()) as GitHubFile[]

  rmSync(THEMES_DIR, { recursive: true, force: true })
  mkdirSync(THEMES_DIR, { recursive: true })

  const generatedIds: string[] = []

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
    const baseId = file.name.replace(/\.json$/, '')
    const themePath = join(THEMES_DIR, file.name)

    try {
      const themeJson = JSON.parse(themeContent) as Record<string, unknown>
      injectTextWeak(themeJson)
      solidifyBackground(themeJson)
      injectAppearance(themeJson)
      writeFileSync(themePath, JSON.stringify(themeJson, null, 2) + '\n')
      generatedIds.push(baseId)
      console.log(`Saved ${file.name}`)

      const lightSibling = makeLightSibling(themeJson, baseId)
      if (lightSibling) {
        const lightName = `${baseId}-light.json`
        writeFileSync(join(THEMES_DIR, lightName), JSON.stringify(lightSibling, null, 2) + '\n')
        generatedIds.push(`${baseId}-light`)
        console.log(`Saved ${lightName}`)
      }
    } catch {
      writeFileSync(themePath, themeContent)
      generatedIds.push(baseId)
      console.log(`Saved ${file.name} (raw)`)
    }
  }

  generatedIds.sort((a, b) => a.localeCompare(b))
  writeBundledThemesList(generatedIds)

  console.log(`\nDone! Themes saved to ${THEMES_DIR} (${generatedIds.length} generated)`)
}

fetchThemes().catch((err) => {
  console.error(err)
  process.exitCode = 1
})
