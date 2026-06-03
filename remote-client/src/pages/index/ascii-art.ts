import crabcodeLogo from "../../../../crabcode-logo.txt?raw"
import mascotArt from "../../../../mascot.txt?raw"

export const LOGO_ART = normalizeArt(crabcodeLogo, { trimCommonIndent: true })
export const MASCOT_FRAMES = mascotArt
  .trimEnd()
  .split(/\n\s*\n/)
  .filter((frame) => frame.trim().length > 0)
  .map((frame) => normalizeArt(frame))

function normalizeArt(source: string, options: { trimCommonIndent?: boolean } = {}) {
  let lines = source.trimEnd().split("\n")
  if (options.trimCommonIndent) {
    const indents = lines
      .filter((line) => line.trim().length > 0)
      .map((line) => line.match(/^ */)?.[0].length ?? 0)
    const indent = Math.min(...indents)
    if (Number.isFinite(indent) && indent > 0) {
      lines = lines.map((line) => line.slice(indent))
    }
  }

  const width = Math.max(0, ...lines.map((line) => line.length))
  return lines.map((line) => line.padEnd(width, " ")).join("\n")
}
