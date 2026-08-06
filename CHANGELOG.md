# Changelog

All notable changes to this project will be documented in this file.

## [0.0.10] - 2026-08-06

### Bug Fixes

- Treat `@ai-sdk/gateway` as OpenAI-compatible provider by @Blankeos
- Group adjacent tool calls and outputs in API payload by @Blankeos
- Bad merge left uplicate enabled_providers/disabled_providers by @Blankeos
- Correct multibyte character handling in get_selected_text() (#14) by @visitorise in [#14](https://github.com/Blankeos/crabcode/pull/14)
- Sanitize wrapped lines and honor hard line breaks by @Blankeos
- Add OpenAI Responses Lite contract for codex models by @Blankeos
- For kimi-k3 & possibly anthropic models, require explicit non-final stop to request phase-less follow-up by @Blankeos
- Vercel ai gateway endpoint by @Blankeos

### Features

- Report session activity to herdr agent panel by Blankeos
- Add prompt-caching diagnostics for provider streams + docs by @Blankeos
- Add Anthropic prompt caching for gateway and direct requests by @Blankeos
- Apply top-level runtime settings (#12) by @yan-ad in [#12](https://github.com/Blankeos/crabcode/pull/12)
- Add editor protocol server (#8) by @yan-ad in [#8](https://github.com/Blankeos/crabcode/pull/8)
- Add continuation guidance for interrupted turns by @Blankeos
- Add shell completion command (#9) by @yan-ad in [#9](https://github.com/Blankeos/crabcode/pull/9)
- Add upgrade command (#7) by @yan-ad in [#7](https://github.com/Blankeos/crabcode/pull/7)


### New Contributors

- @yan-ad made their first contribution in [#12](https://github.com/Blankeos/crabcode/pull/12)
## [0.0.9] - 2026-07-28

### Bug Fixes

- Reserve chat scroll space for active question/permission dialogs by @Blankeos
- Stop child subagent (stuck loading) streams on parent queue interrupts by @Blankeos
- Prevent responsive layout race and stabilize popover focus by @Blankeos
- Disable response storage for subagent requests by @Blankeos

### Documentation

- More planning for v2-shape by @Blankeos

### Features

- Remove teh add+0 and del-0 when working tree is clean by @Blankeos
- Make file diffs collapsible and default to all-view by @Blankeos
- Add interactive git diff viewer with untracked file support by @Blankeos
- Add custom provider configuration support and mcp error screen broken fix (#5) by @visitorise in [#5](https://github.com/Blankeos/crabcode/pull/5)
- Refining the remote ui by @Blankeos
- Add xAI Grok Build OAuth transport overrides and retry policy by @Blankeos


### New Contributors

- @visitorise made their first contribution in [#5](https://github.com/Blankeos/crabcode/pull/5)
## [0.0.8] - 2026-07-23

### Bug Fixes

- Resolve workspace-aware file tool paths by @Blankeos
- Allow `.env.example` files without permission prompts by @Blankeos
- Tooltip for copy + input handling for it by @Blankeos
- Failed to publish refreshed model catalog by @Blankeos
- Skip doom-loop checks for safe read-like actions by @Blankeos
- Preserve selection during scroll and prioritize top search match by @Blankeos
- Prevent models dialog scroll from firing on reasoning control by @Blankeos
- Refine refresh popup layout, copy, and footer action hint by @Blankeos
- Preserve websocket delta continuation only on live OpenAI socket by @Blankeos
- Harden cancellation and subagent session isolation by @Blankeos

### Chores

- Needs a user agent by @Blankeos
- Automated trusted-publishing after just tag. by @Blankeos

### Features

- More lenient auth.json parsing by @Blankeos
- Add native effective catalog snapshot by @Blankeos
- Add stream rollback recovery and websocket fallback hardening by @Blankeos
- Add interactive terminal sessions with PTY and non-interactive bash hardening by @Blankeos
- Add assistant response markdown copy action by @Blankeos

### Performance

- Make subagent tab switching warm-cache and fix per-refresh usage walk by @Blankeos
- Move message parts during snapshot conversion by @Blankeos
- Trim per-frame buffer clones and subagent tab rebuilds by @Blankeos
- Cache streaming tool rows and stop deep-cloning viewport lines by @Blankeos
- Batch streaming snapshots in one transaction with WAL by @Blankeos
- Reopen /models from cache when providers unchanged by @Blankeos
- Optimize subagent streaming and rendering cadence by @Blankeos
- Run model discovery and refresh commands asynchronously by @Blankeos
- Cache discovered and runtime models in-process by @Blankeos
- Optimize streaming drain scheduling and sessions dialog refresh path by @Blankeos

### Refactor

- Remove redundant non-unix process-group killer by @Blankeos

### Ci

- Validate release refs originate from main before publishing by @Blankeos

## [0.0.7] - 2026-07-12

### Bug Fixes

- Classify SSE errors as retryable/permanent and enforce stream termination by @Blankeos

### Chores

- Add per-task model selection and hard-task defaults by @Blankeos

### Features

- Add watched file indexer for completion suggestions by @Blankeos
- Support mouse handling for permission/question dialogs by @Blankeos
- Play notification sounds for print-mode lifecycle events by @Blankeos
- Add configurable terminal title composition by @Blankeos
- Add dedicated subagent completion notification event by @Blankeos

### Performance

- Improve subagent-aware chunk coalescing and markdown render performance by @Blankeos

## [0.0.6] - 2026-07-09

### Bug Fixes

- Make active tool marker animation stateless by @Blankeos
- Infer apply_patch hunk line numbers from surrounding context by @Blankeos
- Preserve selection when restoring and updating search filters by @Blankeos
- Infer commandcode image support from capabilities by @Blankeos
- Surface websocket fallback warnings for stream disconnects by @Blankeos
- Add provider alias for connect autocomplete by @Blankeos
- Preserve chat input draft when running command palette commands by @Blankeos

### Features

- Prioritize current workspace sessions in search by @Blankeos
- Add expandable large paste placeholders with hover tooltip by @Blankeos
- Prioritize reasoning effort options from discovery metadata by @Blankeos
- Add editor-anchored opening for chat selections and file links by @Blankeos
- Scroll into view on load by @Blankeos
- Reuse Enter key for repeat navigation after search by @Blankeos

### Performance

- Optimize streaming rendering and token usage updates by @Blankeos

## [0.0.5] - 2026-06-30

### Bug Fixes

- Clear command input before processing command submissions by @Blankeos
- Derive fork titles from session name by @Blankeos
- Exclude non-decode waits from streaming TPS and duration metrics by @Blankeos
- Jump to latest child session for subagent navigation by @Blankeos
- Propagate cancellation between parent and subagent sessions by @Blankeos
- Inline streaming assistant indicator into message view by @Blankeos
- Use assistant tool parts to resolve file path hyperlinks by @Blankeos
- Avoid marking directories as clickable hyperlinks by @Blankeos
- Hide placeholder assistant entries from timeline list by @Blankeos
- Improve SSE error parsing and OAuth refresh error handling by @Blankeos
- Keep chat input cursor fixed during mouse scrolling by @Blankeos
- Fix Alt+Backspace word deletion for UTF-8 and boundary cases by @Blankeos
- Improve subagent footer layout at narrow widths by @Blankeos
- Restore input navigation in running subagent sessions by @Blankeos
- Add command/option shortcuts for custom text editing by @Blankeos

### Chores

- Fix release yml by @Blankeos

### Features

- Add configurable small model for automatic session titles by @Blankeos
- Lazily hydrate sessions and cache message counts by @Blankeos
- Add MCP server configuration, management, and tool execution by @Blankeos
- Add hierarchical subagent thread tabs and parent/child session navigation by @Blankeos
- Remote client ui add touch manipulation by @Blankeos
- Remember permission grants by pattern scope by @Blankeos
- Propagate OpenAI request options into subagent sessions by @Blankeos
- Include dialog item IDs in search matching by @Blankeos
- Add resume-session shortcut and active-project sidebar sync by @Blankeos
- Render write_files tool output with per-file diffs by @Blankeos
- Surface status-driven primary agents and improve streaming thread UX by @Blankeos
- Prioritize favorite models during search by @Blankeos
- Add primary-agent picker dialog and command-driven switching by @Blankeos
- Add interactive /move command to relocate current session by @Blankeos
- Format assistant reasoning and add thinking toggle controls by @Blankeos
- Add cancellable streaming retries with backoff and status UI by @Blankeos

### Doc

- Mcp docs up-to-date by @Blankeos

## [0.0.4] - 2026-06-14

### Bug Fixes

- Make file mutations atomic and context-safe by @Blankeos
- Preserve selected provider by id during refresh by @Blankeos

### Documentation

- Update install docs and TODOs for search migration by @Blankeos

### Features

- Add vision subagent fallback for non-vision image turns by @Blankeos
- Prevent new chat actions from wrapping by @Blankeos
- Add git status side panel and remote git API by @Blankeos
- Add configurable reasoning effort for subagent configs by @Blankeos
- Add persistent project expansion and expand-all sidebar control by @Blankeos
- Improve mobile chat layout and enable queued streaming prompts by @Blankeos
- Support unauthenticated free model browsing and requests by @Blankeos
- Add pluggable provider extension framework for models by @Blankeos
- Add xAI OAuth support and generalize provider OAuth flow by @Blankeos

### Ci

- Harden release pipeline and Homebrew publish flow by @Blankeos

## [0.0.3] - 2026-06-10

### Bug Fixes

- Treat assistant streaming as active before first token by @Blankeos
- Keep streaming status visible in compact terminal widths by @Blankeos
- Avoid accidental selection during cursor navigation by @Blankeos
- Normalize interleaved tool-call/result replay ordering by @Blankeos
- Improve markdown table wrapping and terminal error notifications by @Blankeos
- Improve compaction selection, token accounting, and stats messaging by @Blankeos
- Preserve draft media state when navigating prompt history by @Blankeos
- Fix chat selection shortcut ordering by @Blankeos
- Add active-tab scrolling and dynamic height sizing by @Blankeos

### Chores

- Add Homebrew publishing pipeline by @Blankeos

### Features

- Coalesce terminal mouse input and improve chat wheel scrolling by @Blankeos
- Add chat find bar and improve command-backspace line navigation by @Blankeos
- Optimize incremental streaming rendering and persistence by @Blankeos
- Add reasoning capability mappings for supported models by @Blankeos
- Add reusable action dialog for copy and message actions by @Blankeos

## [0.0.2] - 2026-06-07

### Bug Fixes

- Gate platform-specific notification and sound-path code by @Blankeos
- Isolate reasoning effort overrides per instance and show workspace in notifications by @Blankeos
- Handle kitty key events and add textarea line shortcuts by @Blankeos
- Stabilize terminal restoration and preserve dialog scroll focus by @Blankeos
- Focus first session item when opening workspace dialog by @Blankeos
- Fail on remote-client missing during build by @Blankeos
- Clarify Tailscale workflow docs and stabilize composer dock layout by @Blankeos
- Handle wrapped prompt lines in history navigation. by @Blankeos
- Keep selected `/models` item visible when it is the last row by @Blankeos
- Match hidden command tokens during search. by @Blankeos
- Hide registered "skills" from autocomplete suggestions (they're not commands). by @Blankeos
- Stdin pipe for -p mode. by @Blankeos
- Preserve table line breaks in rendered markdown tables. by @Blankeos
- Keep list markers inline with item text when wrapping. by @Blankeos
- Kimi k2.6 fixes. by @Blankeos
- More fixes on premature complete esp for other models, qwen 3.7 max. by @Blankeos
- Anthropic-style for qwen 3.7 max. by @Blankeos
- Outside input box not triggering tooltip for copy. by @Blankeos
- Add bounded retry for websocket stream disconnects. by @Blankeos
- Diff bleeding. by @Blankeos
- Hyperlink on hover only. by @Blankeos
- Avoid false positive hyperlinks for single-segment absolute paths. by @Blankeos
- Exclude diff gutters from selection copy and highlighting. by @Blankeos
- Apply background color at line level for inline code rows. by @Blankeos
- Handle stale websocket connections and normalize assistant message shapes. by @Blankeos
- Address premature completion recurrence through prompt parity and structured tool history. by @Blankeos
- Preserve explicit newlines in user messages. by @Blankeos
- Premature completion 2. by @Blankeos
- Defer finish when tool messages still running. by @Blankeos
- Chat input box wrapping fixes. by @Blankeos
- Premature stops. by @Blankeos
- Use connect timeout instead of request timeout for streaming SSE connections. by @Blankeos
- Allow scroll passthrough when permission dialog is open. by @Blankeos
- Keep chat pinned to bottom when new stream content arrives. by @Blankeos
- Style image placeholders with markdown_image color. by @Blankeos
- Add structured stream logging with request/summary diagnostics. by @Blankeos
- Remove compact tool panel spacing special case. by @Blankeos
- Issue w/ opencodego models, replace parse_tool_calls with streaming ToolCallAccumulator. by @Blankeos
- Add vertical padding and background styling to user message bubbles. by @Blankeos
- Defer session creation until first message with pending title support. by @Blankeos
- Proper table rendering. by @Blankeos
- Plan to build so it can call tools. by @Blankeos
- Layout shifts when focusing timeline dialog. by @Blankeos
- Horizontal centering of mascot. by @Blankeos
- Better spacing using a clever glyph (upper half block). by @Blankeos
- Cache chat rendering and adapt event loop poll rate to reduce idle CPU usage. by @Blankeos
- For sessions and themes dialogs. autofocus what is current. by @Blankeos
- Minor spacings in scrollbar stuff. by @Blankeos
- Border-l of my chat messages. by @Blankeos
- Popup padding commands. by @Blankeos
- Ctrl+a in /models and a few jank fixes. by @Blankeos
- 'qui' instead of 'quit' (cutoff). by @Blankeos
- Fixed the `esc` in the dialog to stick to the right. by @Blankeos
- Minor fix on popup paddings. + lots more polish. by @Blankeos
- BIG! PASTE WORKS!!!!! by @Blankeos
- Focus highlight width. by @Blankeos
- Dialog item styles (description). by @Blankeos
- Fix bugs with session sorting and display. by @Blankeos
- Highlight list content area dialog thing. by @Blankeos
- Added padding for dialogs. by @Blankeos
- Completely working api key persistence. by @Blankeos
- Api key dialog shows up after entering on a provider in connect. by @Blankeos
- Cursor visibility improvmenets. by @Blankeos
- Minor UI fixes. by @Blankeos
- Issues after refactoring. by @Blankeos
- Shift+enter controls on zed. by @Blankeos

### Chores

- Add git-cliff changelog generation to release pipeline by @Blankeos
- More readme details by @Blankeos
- Readme images. by @Blankeos
- Added a root favicon so crabcode/t3code picks it up. by @Blankeos
- Author by @Blankeos
- Remote usage plan. by @Blankeos
- Todo by @Blankeos
- Just progress mgmt stuff. by @Blankeos
- Remove aisdk_debug.log. by @Blankeos
- Todos by @Blankeos
- Add makeshift agent benchmarking script. by @Blankeos
- Added opencode ai plugin. by @Blankeos
- Remove ralphy. by @Blankeos
- Install script for linux/macos via curl. by @Blankeos
- Used the crabcode branch for my aisdk-rs fork. by @Blankeos
- Put plans in _plans. by @Blankeos
- Just local dev stuff. by @Blankeos
- Fmt. by @Blankeos
- Added more todos for myself. by @Blankeos
- Moved plans. by @Blankeos
- Other plans added. by @Blankeos
- Added old plans for implementing tool calls and system prompts. by @Blankeos
- Formatting. by @Blankeos
- Initial AGENTS.md by @Blankeos
- Added debug by @Blankeos
- Transferred all plan dcs in _plans. by @Blankeos
- Done on some features. by @Blankeos
- Added intiial justfile. by @Blankeos
- Added ratatui skill. by @Blankeos
- Deps. by @Blankeos
- Upgraded version lots of cleaning. by @Blankeos

### Documentation

- Remove outdated `?` command hint by @Blankeos
- Add WebSocket reset bug investigation to premature complete notes. by @Blankeos
- Add formatting reminder to AGENTS.md. by @Blankeos
- Fix config.mdx references, add remote-usage plan, enable theme and sounds. by @Blankeos
- Add `--dangerously-skip-permissions` to crabcode benchmark commands. by @Blankeos
- Added codex parity docs (but might not use). by @Blankeos
- Updated docs on multiworkspace. by @Blankeos
- More install options. by @Blankeos
- Added banner image. by @Blankeos
- Better docs. by @Blankeos
- Add bundled sound defaults and JSON schema for config by @Blankeos
- Initial docs w/ gittydocs. by @Blankeos
- Added some todos. by @Blankeos
- License and docs. by @Blankeos
- Chat experience plan. by @Blankeos
- Tracking progress. by @Blankeos
- Doc for first publish. by @Blankeos
- Mark git branch detection and CWD display as complete in PLAN.md by @Blankeos

### Features

- Inline LLM SDK and migrate imports to local module by @Blankeos
- Add CommandCode remote provider discovery by @Blankeos
- Add configurable websearch integration with provider adapters by @Blankeos
- Add configurable macOS desktop notification backend by @Blankeos
- Add syntax-highlighted apply_patch and edit diff previews by @Blankeos
- Render apply_patch tool output as diff previews by @Blankeos
- Show active dialog entries as markers instead of right-side labels by @Blankeos
- Add vertical permission action list and keyboard navigation by @Blankeos
- Add dedicated weak text theme token for placeholders by @Blankeos
- Add remote host launch flow and grouped tool output by @Blankeos
- Preserve assistant tool-call lifecycle as ordered message parts by @Blankeos
- Centralize model dialog description helper by @Blankeos
- Add print-mode prompt size preflight. by @Blankeos
- BIG add remote mode support with client UI and release plumbing by @Blankeos
- Add command palette toggle for assistant thinking visibility by @Blankeos
- Persist theme selection as state fallback by @Blankeos
- Queue and batch user messages during active compaction. by @Blankeos
- Add /fork command with /branch alias for session cloning. by @Blankeos
- Add reasoning-effort override and apply_patch tool support. by @Blankeos
- Add non-interactive print mode and batched file write support. by @Blankeos
- Add issue triage pipeline benchmark task. by @Blankeos
- Extract bench-agents into modular benchmarking package. by @Blankeos
- Restore undo attachments and update terminal title state. by @Blankeos
- Optimization. by @Blankeos
- Add OpenCode-compatible agent registry with @mentions and markdown agents by @Blankeos
- Add reasoning_content support to tool call messages. by @Blankeos
- Handle text-only models when images are attached. by @Blankeos
- Add edge scrolling for text selection drag. by @Blankeos
- Enforce at most one in_progress item in update_plan. by @Blankeos
- Proper context compaction. by @Blankeos
- Add "Skills" command palette entry to open skills dialog. by @Blankeos
- Emit terminal BEL on permission/question events, fix scroll-on-click. by @Blankeos
- Add local Ollama provider integration with optional API keys. by @Blankeos
- Add tool image output support and chat hyperlinks. by @Blankeos
- Add view_image tool for local image inspection. by @Blankeos
- Add codex-imagegen skill (exampleonly). by @Blankeos
- Queue messages sent while streaming and auto-submit after current turn. by @Blankeos
- Group assistant turn parts into logical message blocks for clipboard, fork, click, and highlight. by @Blankeos
- Add storage dialog, refactor permissions, improve syntax highlighting. by @Blankeos
- Add syntax-highlighted diffs via syntect by @Blankeos
- Add configurable image opening and improve stream interruption handling. by @Blankeos
- Add OpenAI Responses WebSocket transport with incremental delta. by @Blankeos
- Gate app.log logging behind `--emit-logs` flag. by @Blankeos
- Open message actions on direct chat message click. by @Blankeos
- Normalize plan status markers and add helper functions. by @Blankeos
- Add command palette overlay accessible via ctrl+p. by @Blankeos
- Add premature-completion diagnostics and relax read-tool permissions. by @Blankeos
- Add terminal notification signals (BEL) for completion events. by @Blankeos
- Add esc to cancel pending delete. by @Blankeos
- Add tool error diagnostics and non-fatal error recovery. by @Blankeos
- Add assistant message phase and response completed streaming support. by @Blankeos
- Emit ChunkType::End on [DONE] and finish_reason; enforce terminal stream events. by @Blankeos
- Compact large paste content into placeholders during input. by @Blankeos
- Normalize and expand GPT-5 model matching for OpenAI OAuth. by @Blankeos
- Preserve system message content in instructions when stripping system messages. by @Blankeos
- Implement custom slash commands. by @Blankeos
- Persist partial messages on streaming failure. by @Blankeos
- Add reasoning effort support with Ctrl+T cycling and models dialog controls. by @Blankeos
- Replace "♥︎ Favorite" tip with standalone "❤︎" and refactor timeline highlight. by @Blankeos
- Show sessions from all workspaces with group reordering. by @Blankeos
- Add workflow-planner-ts benchmark case with hidden test runner. by @Blankeos
- Add live reports, safe runner, static server, and 3 new tasks. by @Blankeos
- Add OpenAI Responses API function call support and --dangerously-skip-permissions flag. by @Blankeos
- Overhaul HTML conversion, add streaming, Cloudflare handling, and content validation. by @Blankeos
- Rename `todowrite` tool to `update_plan` and overhaul tool rendering UI. by @Blankeos
- Execute same-step tool calls concurrently and deduplicate repeated task calls. by @Blankeos
- (better) refactor subagent UI to footer with locked input in child sessions. by @Blankeos
- Made mouse in command and file popovers. by @Blankeos
- Add session compaction for reducing context token usage. by @Blankeos
- Handle SSE metadata lines and preserve sessions dialog on delete. by @Blankeos
- Add image attachment support with clipboard paste and @-file autocomplete. by @Blankeos
- Better "diff" more similar to codex when editing. by @Blankeos
- Group consecutive exploration tool messages and refactor list tool. by @Blankeos
- Bound tool output and compact read/list UI. by @Blankeos
- Remove vertical centering of content, render at top. by @Blankeos
- Implement multi-step subagent tool loops with child session navigation. by @Blankeos
- Add multi-session management with workspace-aware streaming, pin/archive, and status tracking. by @Blankeos
- Allow command submission during streaming; refactor dialog actions and better scrollbar drag. by @Blankeos
- Switch to session on dialog click and fix popup scroll. by @Blankeos
- Migrate data storage to XDG_STATE_HOME with private permissions. by @Blankeos
- Collapse consecutive assistant messages into one timeline item. by @Blankeos
- Make navigation mouse-driven and require explicit confirmation. by @Blankeos
- Auto-generate fallback options and detect skips; fix tool call streaming. by @Blankeos
- Better interactive question dialog + fixed toolcall rendering. by @Blankeos
- Replace eventsource-stream with custom SSE parser and add inline diff rendering. by @Blankeos
- Working ai sdk port. by @Blankeos
- Expand plan mode permissions and log skipped tools. by @Blankeos
- Opencode parity 1st run. Lots of tools added. by @Blankeos
- Better chat input box background theme. by @Blankeos
- PERFECT spacing using clever glyphs. by @Blankeos
- Better spacing + color for the chat input box. by @Blankeos
- Restore undone message content to input, make home screen layout responsive. by @Blankeos
- Improve highlight styling and scrollbar consistency. by @Blankeos
- Show token usage as percentage of model limit, fix UTF-8 truncation boundary. by @Blankeos
- Added /rename <opt:newname> command. by @Blankeos
- Add print mode, session cost tracking, mascot animation, and transcript copy. by @Blankeos
- Add message actions dialog with copy/fork/undo, chat-only commands, and emoji fixes. by @Blankeos
- Simplify timeline dialog and fix mouse selection behavior. by @Blankeos
- Add timeline dialog and text selection with copy-on-select. by @Blankeos
- Use cuid2 identifiers instead of sequential names for sessions. by @Blankeos
- Add session resume CLI flag, remove landing page, simplify model display. by @Blankeos
- Add hidden token support for command aliases. by @Blankeos
- Mascot done. by @Blankeos
- Include item tips in dialog search matching. by @Blankeos
- Implement skills system with file-system discovery and tool integration. by @Blankeos
- Add skills dialog and update OpenAI provider API. by @Blankeos
- Better permission dialog (not exactly a dialog anymore). by @Blankeos
- Infinite steps + more consistent timeout w/ opencode. by @Blankeos
- Openai codex oauth. by @Blankeos
- Better markdown color themes. by @Blankeos
- BIG completely revamped toasts. No more ratatui_toolkit. by @Blankeos
- A lot better theme for Plan and Build agents. by @Blankeos
- Added 'notify' feature along w/ the sounds. by @Blankeos
- Added npm release scripts by @Blankeos
- Add built-in sound effects for error and complete events by @Blankeos
- Hide modal when click outside. by @Blankeos
- Add accurate token counting during streaming responses by @Blankeos
- Lower scroll speed so it doesn't feel janky. by @Blankeos
- Perfect search UX by @Blankeos
- Proper status bar theme usage. (branch) by @Blankeos
- Complete work of art for the theme-token usage. It works. by @Blankeos
- Basic theme preview as selection goes. by @Blankeos
- Added initial configuration discovery + plan. by @Blankeos
- Made api key made optional. by @Blankeos
- Added proper metrics (tps, ttft, total latency). by @Blankeos
- Added /refreshmodels by @Blankeos
- Added AGENTS.md and CLAUDE.md discovery. by @Blankeos
- Prevent sending messages while streaming... by @Blankeos
- Added anthropic compat. by @Blankeos
- Working initial tool calls. by @Blankeos
- Added tools registry. 6 core tools. by @Blankeos
- Added scroll up/down whichkeys. by @Blankeos
- Added markdown response streaming. by @Blankeos
- Prompt history persistence, working with up key now! by @Blankeos
- Added wave spinner, smoother animation, throttled updates for non-ui stuff. by @Blankeos
- Polish chat experience 2. by @Blankeos
- Polish chat experience 1. by @Blankeos
- Added which-key (inspired from neovim) and mouse events in input. by @Blankeos
- Finally working chat w/ oai compatible providers. by @Blankeos
- Working aisdk integration + Streaming works w/ unbounded channels. by @Blankeos
- Made the active model's provider name be shown in the input. by @Blankeos
- Proper theme usage. by @Blankeos
- Added persisted 'active model' switching & Favorites and Recent. by @Blankeos
- Agent color switching, when tab. by @Blankeos
- Initial tab switching of the current 'Agent'. by @Blankeos
- Perfect commands suggestions popup UI polish. by @Blankeos
- Mouse events and switching between sessions works. by @Blankeos
- Amazing scrollbar. by @Blankeos
- Better scrollbar UI look. by @Blankeos
- 2-step gradient for logo. by @Blankeos
- Perfect scrolling UX. by @Blankeos
- Added working connect dialog. by @Blankeos
- Added persistence (rusqlite, cuid2). by @Blankeos
- Connected feature plan. by @Blankeos
- Massive polish to "Provider" + "Model Name" fuzzy search. by @Blankeos
- Added nucleo for fuzzy search matching. by @Blankeos
- Filter only text models. by @Blankeos
- Better scroll and mouse experience in the models dialog. by @Blankeos
- Added basic theming (vibe coded purely). by @Blankeos
- Added theme scraping script. by @Blankeos
- Better UX. by @Blankeos
- Added the models dialog. very good. by @Blankeos
- Better placeholder + cwd indicator. by @Blankeos
- Good padding between input area. by @Blankeos
- Good style changes + dynamic height for the input.rs by @Blankeos
- A lot better layout. by @Blankeos
- Good landing. by @Blankeos
- Implement Provider trait definition for AI model providers by @Blankeos
- Implement /sessions and /new commands with session management by @Blankeos
- Implement auto-suggestion popup with ratatui by @Blankeos
- Implement basic text input using tui-textarea by @Blankeos
- Implement landing page with logo display by @Blankeos
- Add project structure and module stubs by @Blankeos
- Initial rust project. by @Blankeos

### Refactor

- Remove stale exports and stabilize dialog/model discovery tests by @Blankeos
- Split index page into modular remote client implementation by @Blankeos
- (annoying) prevent opening message actions when clicking assistant messagesfix: prevent opening message actions when clicking assistant messages. by @Blankeos
- Group adjacent tool calls into single assistant message. by @Blankeos
- Extract SSE stream parsing into composable helpers. by @Blankeos
- Changed from 'annotate' to 'add to prompt'. by @Blankeos
- Unify sounds and notifications into single notifications config. by @Blankeos
- Simplify post-close logo styling with single-color lines. by @Blankeos
- Theme post-close logo and replace startup diagnostics with logging. by @Blankeos
- Extract content padding to deduplicate layout logic. by @Blankeos
- Suppress tool call/result output in print mode. by @Blankeos
- Centralize CWD resolution and embed themes at compile time. by @Blankeos
- Extract search area height into a named constant. by @Blankeos
- Performance optimizations in chat loading. by @Blankeos
- Render timeline highlight as full-width background. by @Blankeos
- Replace inline timeline highlight with overlay band. by @Blankeos
- Refactor LLM client into modular components by @Blankeos
- Cleaned up dead code. by @Blankeos
- Dialog.rs refactoring (Removed 'connected: bool'). by @Blankeos
- Refactored to my prefer structure. by @Blankeos

### Doc

- More clarity on merging. by @Blankeos
- Updated docs by @Blankeos


### New Contributors

- @Blankeos made their first contribution

