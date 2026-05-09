- [ ] Rearchitect - multi-workspace, just like the codex desktop app.
  - Since it's a terminal, we have a special case to make it run even when closed, or when there are multiple instances of the program running. They have the same sort of "streaming" state. I will elaborate.
  - Mutli-workspace feature is essentially having multiple "chat sessions" running. Currently.. Every run of `crabcode` is its own isolated session.
  - We want to change that by making `crabcode` a multi-workspace agentic TUI by default, just like the codex desktop app, superconductor, etc. But simpler because the idea is literally just like a chat app on the web. Wherein, I want to be able to check the "sessions" in the sidebar, create new chats in the same tab (in this case a tab is a run of `crabcode`).
  - So we can model this off of existing chat apps I've made (INSERT REFERENCE HERE)
  - Because we can create multiple sessions, we can swap between them because each chat session will now be isolated with their own state. No worktrees for now because that's complicated.
  - Since they each have their own state, that means the streaming will have their own states and when I do `/sessions` I can clearly see what's currently streaming and already done. We want to indicate "streaming" with the same icon claude uses (I had a very nice working example here /Users/carlo/Desktop/Projects/lazygitrs
    )
  - Also the idea is, we can run create multiple "sessions" in the same run of `crabcode`. And we can even open multiple `crabcode` runs in the terminal, and it'll still have the same states for "streaming" when I check the other sessions with `/session`.
  - /sessions can switch between running sessions. Show a loading (use claude code loading animation), for loading sessions. Group by folders, not by Today, etc. Move the /sessions dialog to the "left". Run as a process? Allow for interruption as well. Maybe via a `/` command or a `ctrl-x` shortcut.

- [ ] Just like opencode. I want to see the `94.4.k (9%) ∙ $0.39` detail just next to the helpful tips under the input box. Use the same data sources.

- [ ] Scrollbar, make it like opencode. As thin as opencode. That's the only change I want really.

- [ ] Add print-mode just like `opencode run "<PROMPT>"`. See the reference. But two things I want to deviate from the original implementation:
  - The preamble, just print whatever is printed, that's IT!
  - Also add Call it `opencode -p`. It's gonna be exactly the same as `opencode run`.
  - Add `--no-session-persistence` flag, exactly like Claude Code.
  - Other than that, very similar to the original implementation.
