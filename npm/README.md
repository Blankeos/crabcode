# 🦀 crabcode

A purely Rust-based AI CLI coding agent with a beautiful terminal UI for interactive "agentic engineering".

> In the words of the buildwithpi.ai creators, 'There are many coding agents, this one is mine'.
>
> It's OpenCode but in pure Rust 🦀 w/ my personal flavors.
>
> ~ Carlo (Author)

![Crabcode banner](_docs/[images]/crabcode_banner.jpg)

## Features

- **Made with Rust** - Uses ratatui, crossterm and nucleo (fuzzy search), all fast tech.
- **Notifications** - Sounds, desktop notifications, and terminal alert signals are built in.
- **TPS, TTFT, Latency metrics** - Also wanted this in opencode, just made it built-in.
- **Opens instantly** - one of my main motivations why I made this! :D Very lightweight after build.
- **Terminal UI (TUI)** - Beautiful, responsive interface built with [ratatui](https://github.com/ratatui-org/ratatui)
- **Built for the OpenCode user** - works out of the box w/ opencode themes, every UX, and some existing configs so you don't need to force your team to use crabcode.
  - **Same UX** - carefully ported most of the good UX from OpenCode i.e. shortcuts, etc.
  - **Agent System** - Switch between PLAN (read-only analysis) and BUILD (implementation) agents with TAB, and custom agents.
  - **Multiple Model Support** - Works w/ the same models.dev support.
  - **Command System** - Intuitive commands: `/sessions`, `/new`, `/connect`, `/models`, `/exit` + custom commands.
  - **Session Management** - Create and manage multiple chat sessions
  - **Streaming Responses** - Real-time streaming of AI responses (w/ [aisdk.rs](https://aisdk.rs))

## Installation

```sh
brew install blankeos/tap/crabcode # Homebrew (macOS/Linux)
npm install -g crabcode            # or npm
bun install -g crabcode            # or bun
cargo binstall crabcode            # or cargo-binstall (prebuilt binary, faster)
cargo install crabcode             # or cargo (build from source)
curl -sSL https://raw.githubusercontent.com/Blankeos/crabcode/main/install.sh | sh # or linux/macos (via curl)
```

## Quick Start

1. Run crabcode:

   ```bash
   crabcode
   ```

2. Configure your AI model:

   ```
   /connect
   ```

3. Start coding! Type your questions or requests and press Enter.

## Usage

It works (almost) exactly like OpenCode. Just opens faster, with some intuitive changes I like, here are most of them:

- Opens instantly!
- Sounds out-of-the-box + clean Desktop notifications!
- Multiworkspace by default, can run like 3+ sessions in the same instance, just works like a webapp.
- Ollama Local CLI connections works out-of-the-box.
- My own remote implementation. Probably worse.
- My own UX preferences:
  - Can click on `[Image #1]` tags to open them.
  - Themes has no background, all tranluscent (don't really care right now).
  - Lots of toolcall-shapes inspired by the actual Codex harness.
  - When switching models, you can press `⇆` to change thinking efforts.
  - Copy on select is disabled by default. Copy is two-step in crabcode. Gets annoying in OpenCode, especially w/ clipboard history.

### Agent Types

- **PLAN** - Read-only analysis and planning agent. Best for understanding codebases, architecture questions, and planning changes.
- **BUILD** - Full access implementation agent. Best for writing code, implementing features, and making changes.

## Configuration

Your credentials are stored in crabcode's state directory:

- Default: `~/.local/state/crabcode/auth.json`
- With `XDG_STATE_HOME`: `$XDG_STATE_HOME/crabcode/auth.json`

Read the [configuration docs here](/_docs/config/index.mdx).

### Supported Providers

> Will be powered by mostly [aisdk](https://github.com/lazy-hq/aisdk) + [models.dev](https://models.dev)
> So **most of them** will work out of the box.

I tried crabcode specifically for these providers:

- [x] **openai** (both API key and OAuth, thank you OpenAI for supporting harnesses!)
- [x] **xAI / Grok** (API key and SuperGrok/X Premium OAuth, thank you xAI for openly supporting OSS harnesses: based on [OpenClaw](https://x.ai/news/grok-openclaw), [OpenCode](https://x.ai/news/grok-opencode), [KiloCode](https://x.ai/news/grok-kilocode), [Hermes](https://x.ai/news/grok-hermes))
- [x] **opencode-zen** and **opencode-go**
- [x] **nano-gpt**
- [x] **commandcode** (Pro)
- [x] **ollama** (Local CLI)
- [x] **ollama-cloud**
- [x] **zai**
- [x] **xiaomi-token-plan-sgp**
- [x] **minimax**
- [x] **fireworks**
- [x] **baseten**

> Feel free to create an issue / add to this list if you tried

### Known unsupported providers

> I might work harder to support these in the future.

- Kimi For Coding Subscription - I keep getting 401 but it works in OpenCode, I may have to contact them first. **might support later**
- Gemini - It's OAuth + also very unsure. So currently no.
- Claude Code Subscription - Known to explicitly not like harnesses. So never will, sorry.

## Performance

Like any benchmark, please take this with a grain of salt. I have a cherry-picked benchmark for 1 purpose only: "Is crabcode at least as reliable as codex/opencode?". It's a useful feedback loop for my Crabcode trying to improve itself. We're honestly only chasing for at least: best parity if not better perf while having massively better TUI UX based on my personal preferences. Here is my most recent run (Jul 13, 2026):

| Agent       | Score | Checks | Avg time | Est. tokens | Est. cost |
| ----------- | ----: | -----: | -------: | ----------: | --------: |
| 🦀 crabcode |  100% |  19/19 |    21.3s |        2851 |   $0.0103 |
| 🔲 opencode |  100% |  19/19 |    24.2s |        3837 |   $0.0201 |
| ⚛️ codex    |  100% |  19/19 |    24.9s |       56172 |   $0.5435 |

## Contributing

Contributions are welcome! Please feel free to submit a Pull Request.

## License

This project is licensed under the MIT License - see the [LICENSE](LICENSE) file for details.

## Inspiration

This project was inspired by [anomalyco/opencode](https://github.com/anomalyco/opencode). Also made this project w/ OpenCode btw, so thank you OpenCode! 🙏

## Scope and Limits

- [x] Chat, switch models, agents
- [x] Minimal configurations (I want it to just feel at least like vanilla opencode)
- [x] The cheapest model providers (GLM, etc.)
- [x] A ding sound, my only opencode plugin at the moment.
- [x] No reverse-engineering oauth from big AI (Claude Code, Gemini), at least for now (Don't wanna get in trouble).
- [x] Exceptions: ChatGPT OAuth and xAI Grok OAuth where supported by upstream harnesses.
- [x] Copy chat contents, copy the chat input
- [x] Image inputs
- [x] Personal remote usage + Browser client equivalent.
- [ ] ACP w/ Zed? (very far, idk how to do that)
- [x] No Claude Code oauth spoofing.
- [x] No plugin ecosystem (If I think it's worth building, just make it built-in and configurable i.e. sounds)
- [x] No desktop app

## Why?

I'm learning rust :D. Built a few TUIs as practice. Also been making AI chat apps on web, so I wanna work on this.
