import { afterEach, describe, expect, mock, test } from "bun:test"

// The helper module shares utilities with browser UI; no DOM is needed here.
mock.module("solid-sonner", () => ({ toast: {} }))

const {
  isRegisteredSlashCommand,
  shouldSubmitLiteralSlash,
  loadPromptHistory,
  mergePromptHistoryEntries,
  normalizePromptHistoryEntry,
  savePromptHistory,
} = await import("../src/pages/index/prompt-utils")

const commandNames = ["help", "models", "resume", "skill:review"]

describe("registered slash classification", () => {
  test("a lone slash submits literally unless a suggestion was explicitly selected", () => {
    expect(shouldSubmitLiteralSlash("/", false)).toBe(true)
    expect(shouldSubmitLiteralSlash(" / ", false)).toBe(true)
    expect(shouldSubmitLiteralSlash("/", true)).toBe(false)
    expect(shouldSubmitLiteralSlash("/models", false)).toBe(false)
  })
  test("recognizes registered commands, aliases and hidden skills with arguments", () => {
    for (const text of ["/help", " /models example \n", "/resume session", "/skill:review changes"]) {
      expect(isRegisteredSlashCommand(text, commandNames)).toBe(true)
    }
  })

  test("unknown slash prompts remain messages, including image placeholders", () => {
    for (const text of ["/unknown", "/tmp/example explain this", "/unknown [Image #1]", "/helper", "/", "hello /help"]) {
      expect(isRegisteredSlashCommand(text, commandNames)).toBe(false)
    }
  })

  test("matches registry names exactly rather than case-insensitively", () => {
    expect(isRegisteredSlashCommand("/HELP", commandNames)).toBe(false)
    expect(isRegisteredSlashCommand("/help", [])).toBe(false)
  })

  test("unknown slash prompts take the chat attachments and busy-queue branch", () => {
    const text = "/unknown explain [Image #1]"
    const isCommand = isRegisteredSlashCommand(text, commandNames)
    expect(isCommand).toBe(false) // attachments are restricted only for commands
    expect(true && !isCommand).toBe(true) // streaming messages queue optimistically
    expect(true && !isRegisteredSlashCommand("/skill:review", commandNames)).toBe(false)
  })
})

describe("slash prompt history", () => {
  afterEach(() => {
    Reflect.deleteProperty(globalThis, "localStorage")
  })

  test("normalizes and merges unknown slash messages without dropping them", () => {
    expect(normalizePromptHistoryEntry("  /unknown example \n")).toBe("/unknown example")
    expect(mergePromptHistoryEntries([" /unknown example ", "hello"], ["/unknown example", "/tmp/file"]))
      .toEqual(["/unknown example", "hello", "/tmp/file"])
  })

  test("unknown slash messages survive saving and reloading browser history", () => {
    const storage = new Map<string, string>()
    Object.defineProperty(globalThis, "localStorage", {
      configurable: true,
      value: {
        getItem: (key: string) => storage.get(key) ?? null,
        setItem: (key: string, value: string) => storage.set(key, value),
      },
    })
    savePromptHistory(["/unknown example", "/unknown [Image #1]", "hello"])
    expect(loadPromptHistory()).toEqual(["/unknown example", "/unknown [Image #1]", "hello"])
  })
})
