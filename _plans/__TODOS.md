- [ ] Rearchitect - multi-workspace, just like codex. Since it's a terminal, special case is it runs even when closed. Doing /sessions can switch between running sessions. Show a loading (use claude code loading animation), for loading sessions. Group by folders, not by Today, etc. Move the /sessions dialog to the "left". Run as a process? Allow for interruption as well. Maybe via a `/` command or a `ctrl-x` shortcut.

- [ ] Just like opencode. I want to see the `94.4.k (9%) ∙ $0.39` detail just next to the helpful tips under the input box. Use the same data sources.

- [ ] Scrollbar, make it like opencode. As thin as opencode. That's the only change I want really.

- [ ] Add print-mode just like `opencode run "<PROMPT>"`. See the reference. But two things I want to deviate from the original implementation:
  - The preamble, just print whatever is printed, that's IT!
  - Also add Call it `opencode -p`. It's gonna be exactly the same as `opencode run`.
  - Add `--no-session-persistence` flag, exactly like Claude Code.
  - Other than that, very similar to the original implementation.
