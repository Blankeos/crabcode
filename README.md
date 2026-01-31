# 🦀 crabcode

> [!WARNING]  
> This ambitious project is very very early (like experiment-early) don't expect it to get to OpenCode level anytime soon.
> Like it literally doesn't even work yet.

A purely Rust-based AI CLI coding agent with a beautiful terminal UI for interactive "agentic engineering".

> In the words of the buildwithpi.ai creator, 'There are many coding agents, this one is mine'.
>
> It's OpenCode but in pure Rust 🦀 w/ my personal flavors.
>
> ~ Carlo (Author)

![screenshot](_docs/screenshot.png)

## Features

- **Made with Rust** - Uses ratatui, crossterm and nucleo (fuzzy search), all fast tech.
- **Sounds** - I wanted this in opencode, I just made it built in instead of a plugin.
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

## Quick Start

Install via cargo:

```bash
cargo install crabcode
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

### Commands

| Command     | Description                      |
| ----------- | -------------------------------- |
| `/sessions` | List all sessions                |
| `/new`      | Create a new session             |
| `/connect`  | Open the provider connect dialog |
| `/models`   | List available models            |
| `/exit`     | Quit crabcode                    |

### Key Bindings

| Key              | Action                                 |
| ---------------- | -------------------------------------- |
| `Ctrl+X`         | Open the shortcuts dialog              |
| `TAB`            | Switch between PLAN and BUILD agents   |
| `Enter`          | Submit message or execute command      |
| `Ctrl+C` (once)  | Clear input                            |
| `Ctrl+C` (twice) | Quit                                   |
| `Esc`            | Close popup suggestions                |
| `↑/↓`            | Navigate in input or suggestions popup |

### Agent Types

- **PLAN** - Read-only analysis and planning agent. Best for understanding codebases, architecture questions, and planning changes.
- **BUILD** - Full access implementation agent. Best for writing code, implementing features, and making changes.

## Configuration

Your credentials are stored in an OS-specific data directory:

- macOS: `~/Library/Application Support/crabcode/auth.json`
- Linux: `~/.local/share/crabcode/auth.json`

Read the [extensive list of configs here](/_docs/config.mdx).

### Supported Providers

> Will be powered by mostly [aisdk](https://github.com/lazy-hq/aisdk) + [models.dev](https://models.dev)
> So **most of them** will work out of the box.

I tried crabcode specifically for these providers:

- [x] **opencode-zen**
- [x] **nano-gpt**
- [x] **zai**
- [x] **minimax**
- [x] **fireworks**
- [x] **baseten**
- [x] **ollama**

> Feel free to create an issue / add to this list if you tried

### Known unsupported providers

> I might work harder to support these in the future.

- ChatGPT/Codex Subscription (Though they have good-will to support OpenCode, so maybe CrabCode can as well). **might support later**.
- Kimi For Coding Subscription - I keep getting 401 but it works in OpenCode, I may have to contact them first. **might support later**
- Gemini - It's OAuth + also very unsure. So currently no.
- Claude Code Subscription - Known to explicitly not like harnesses. So never will, sorry.

## Development

### Build from source

```bash
git clone https://github.com/blankeos/crabcode.git
cd crabcode
cargo build --release
```

### Run tests

```bash
cargo test
```

## Contributing

Contributions are welcome! Please feel free to submit a Pull Request.

## License

This project is licensed under the MIT License - see the [LICENSE](LICENSE) file for details.

## Inspiration

This project was inspired by [anomalyco/opencode](https://github.com/anomalyco/opencode). Also made this project w/ OpenCode btw, so thank you OpenCode! 🙏

## Scope

- [x] Chat, switch models, agents
- [x] Minimal configurations (I want it to just feel at least like vanilla opencode)
- [x] The cheapest model providers (GLM, etc.)
- [ ] A ding sound, my only opencode plugin at the moment.
- [x] No reverse-engineering oauth from big AI (Codex, Claude Code, Gemini), at least for now (Don't wanna get in trouble).
- [ ] Possibly ralphy? (very far, idk how to do that)
- [ ] ACP w/ Zed? (very far, idk how to do that)
- [x] No plugin ecosystem
- [x] No desktop app
- [x] No web sharing thing

## Why?

I'm learning rust :D. Built a few TUIs as practice. Also been making AI chat apps on web, so I wanna work on this.
