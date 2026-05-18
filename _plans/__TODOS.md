- [ ] VERY VERY far future. Rearchitect - multi-workspace, just like the codex desktop app.
  - Since it's a terminal, we have a special case to make it run even when closed, or when there are multiple instances of the program running. They have the same sort of "streaming" state. I will elaborate.
  - Mutli-workspace feature is essentially having multiple "chat sessions" running. Currently.. Every run of `crabcode` is its own isolated session.
  - We want to change that by making `crabcode` a multi-workspace agentic TUI by default, just like the codex desktop app, superconductor, etc. But simpler because the idea is literally just like a chat app on the web. Wherein, I want to be able to check the "sessions" in the sidebar, create new chats in the same tab (in this case a tab is a run of `crabcode`).
  - So we can model this off of existing chat apps I've made (INSERT REFERENCE HERE)
  - Because we can create multiple sessions, we can swap between them because each chat session will now be isolated with their own state. No worktrees for now because that's complicated.
  - Since they each have their own state, that means the streaming will have their own states and when I do `/sessions` I can clearly see what's currently streaming and already done. We want to indicate "streaming" with the same icon claude uses (I had a very nice working example here /Users/carlo/Desktop/Projects/lazygitrs
    )
  - Because we want this isolated state. Make sure that in the UI, I can switch session focus just easily and it won't affect the rendering. Each session I go to stream seamlessly. I can show you my existing architecture for this for webapps, it's very seamless. (INSERT REFERENCE HERE)
  - Also the idea is, we can run create multiple "sessions" in the same run of `crabcode`. And we can even open multiple `crabcode` runs in the terminal, and it'll still have the same states for "streaming" when I check the other sessions with `/session`.
  - /sessions can switch between running sessions. Show a loading (use claude code loading animation), for loading sessions. Group by folders, not by Today, etc. Move the /sessions dialog to the "left". Run as a process? Allow for interruption as well. Maybe via a `/` command or a `ctrl-x` shortcut.

- [x] Just like opencode. I want to see the `94.4.k (9%) ∙ $0.39` detail just next to the helpful tips under the input box. Use the same data sources.

- [x] Scrollbar, make it like opencode. As thin as opencode. That's the only change I want really.

- [x] Add print-mode just like `opencode run "<PROMPT>"`. See the reference. But two things I want to deviate from the original implementation:
  - The preamble, just print whatever is printed, that's IT!
  - Also add Call it `opencode -p`. It's gonna be exactly the same as `opencode run`.
  - Add `--no-session-persistence` flag, exactly like Claude Code.
  - Other than that, very similar to the original implementation.

- [x] Add a `/copy` command. See opencode reference for "Copy session transcript" for a similar implementation.

- [x] Minor, When I 'delete' and I delete the current, go to `home` page.

- [x] Minor, after forking. please scroll the conversation all the way down.

- [x] Weird bug: I fork any "agent" message. Anything that has an emoji. I get: 'panicked at src/app.rs:1892:54: byte index 40 is not a char boundary; it is inside '😄' (bytes 37..41) of `Thanks! I'm glad you think I'm cool. 😄'

- [ ] Minor, `chat_only` flag is codesmell... We better come up with strings for deciding "Only show this slash command in this context", just like how we do with 'Shortcuts' (in case shortcuts follow this codesmell as well, come up with a better approach)

- [x] Chore: Create a /checkparity-opencode (the most important thing is only the agent-loop, nothing else. We do differ a bit in terms of UX anyway, but the agent-loop, tool calling, etc has to be very very close so that the performance is mostly the same) and /checkparity-codex (au) command

- [ ] Feature: Subagents just like opencode.

- [x] Feature: Rename command `/rename` - parity with opencode.

- [ ] Bug: theme is not persisted? Or is it by config? Just make theme be based on state now, no more config for it.

- [x] Bug: skill loading on conflict. i.e. duplicate frontend-design skill. Warning: duplicate skill name 'frontend-design' (existing: /Users/carlo/.claude/skills/frontend-design/SKILL.md, duplicate: /Users/carlo/.config/opencode/skill/frontend-design/SKILL.md)

- [x] Bug: Timeline livescroll and actual chat UI consistency - make them the same.

- [ ] Parity: Like opencode, I wanna be able to queue messages. By sending some message even though it's still streaming, won't stop the agent, will just keep going.

- [x] Markdown: Proper Table rendering.

- [x] Rendering: Thinking Rendering always has this massive space below it, even if the agent didn't really think much.

- [ ] Tool call rendering:
  - [ ] editing files w/ diffs, like opencode does.
  - [ ] todowrite - better looking, like opencode does.
  - [ ] rendering subagents - just like opencode, clickable to go into their page.. OR I can do `ctrl-x ↓` to go into it if there's a subagent running. I can also switch between subagents with `←` and `→`

- [ ] Fix: Chat content colors. Currently no matter what theme I use, the color of the chat especially the main text colors in markdown, are the default theme colors that were set during start time - meaning at config. Whatever I change via `/themes` dialog, it doesn't update the chat colors themes.

- [ ] Bug: I can type a command see autosuggest, but can't press 'enter' to run the command. Pls fix.

- [x] A single AI response, is considered 1 message. So combine all its parts into a single message record. Not that every message part becomes a separate message in the timeline dialog.

- [ ] Allow me to paste images i.e. [Image #1] [Image #2] [Image #3]. When I click on them, the image would be opened with my Finder (OS-specific)

- [x] Let's make the 'questions' a bit more mouse-driven.

- [x] Better question handling for skipped (Skipped, if I didn't press enter. like when I do `arrow right` immediately)

- [x] Scroll like herdr. Stuff I like: as thin, as tall (no arrows - currently ours also has no arrows but it was a hack, we just remove the arrows with "" chars so they still take a height. The one from herdr looks like it's a pure scrollbar thumb without arrows and thin enough that I like)

- [x] Highlight enhancements, if I click 1 place, then shift+click another. Treat it like the highlight in the browser that doesn't need a drag. Whatever I last clicked (without shift+click), treat it as the anchor for the "select start", and then whatever I shift+click after, treat it as a "select end" and autohighlight that part. (not supported)
