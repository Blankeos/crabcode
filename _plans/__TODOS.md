- [x] VERY VERY far future. Rearchitect - multi-workspace, just like the codex desktop app.
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

- [ ] Bug: If a non-Build primary agent calls a subagent, returning from the subagent resets the primary agent back to Build. Preserve the previously selected primary agent across subagent calls.

- [ ] Remote UI: Persist the selected primary agent per existing session. For `/new`, default to the last-used primary agent instead of always Build.

- [ ] Remote UI: In the Open Project searchable popover, selecting a project with an active streaming session should auto-open that streaming session instead of creating/navigating to that project's `/new` page.

- [x] Remote UI: Fix thread scroll getting forced back to bottom by streaming updates from other windows/sessions.

- [x] Remote UI: While streaming in the current session, only auto-scroll if the thread was already at the bottom; allow scrolling up without being yanked down.

- [x] Chore: Create a /checkparity-opencode (the most important thing is only the agent-loop, nothing else. We do differ a bit in terms of UX anyway, but the agent-loop, tool calling, etc has to be very very close so that the performance is mostly the same) and /checkparity-codex (au) command

- [x] Feature: Subagents just like opencode.

- [x] Feature: Rename command `/rename` - parity with opencode.

- [x] Let's make the 'theme' selection persisted somewhere in the 'state' (outside the config). So whatever I select, it gets selected. But this 'state' is the 2nd source of theme data, so it becomes a fallback. The primary is the config.. If the config is set, don't get the data from the persisted theme data state. But if it's not configured. Whatever is set, in persisted theme, that's what we use.

- [x] Bug: skill loading on conflict. i.e. duplicate frontend-design skill. Warning: duplicate skill name 'frontend-design' (existing: /Users/carlo/.claude/skills/frontend-design/SKILL.md, duplicate: /Users/carlo/.config/opencode/skill/frontend-design/SKILL.md)

- [x] Bug: Timeline livescroll and actual chat UI consistency - make them the same.

- [x] Parity: Like opencode, I wanna be able to queue messages. By sending some message even though it's still streaming, won't stop the agent, will just keep going.

- [x] Markdown: Proper Table rendering.

- [x] Rendering: Thinking Rendering always has this massive space below it, even if the agent didn't really think much.

- [x] Tool call rendering:
  - [x] editing files w/ diffs, like opencode does.
  - [x] webfetch rendering like codex does.
  - [x] todowrite - better looking, like opencode does.
  - [x] rendering subagents - just like opencode, clickable to go into their page.. OR I can do `ctrl-x ↓` to go into it if there's a subagent running. I can also switch between subagents with `←` and `→`

- [x] Fix: Chat content colors. Currently no matter what theme I use, the color of the chat especially the main text colors in markdown, are the default theme colors that were set during start time - meaning at config. Whatever I change via `/themes` dialog, it doesn't update the chat colors themes.

- [x] Bug: I can type a command see autosuggest, but can't press 'enter' to run the command. Pls fix.

- [x] A single AI response, is considered 1 message. So combine all its parts into a single message record. Not that every message part becomes a separate message in the timeline dialog.

- [x] Message model refactor: persist one logical assistant response as one assistant message with ordered parts (`reasoning`, `text`, `tool_call`, `tool_result`) instead of protocol-shaped `assistant/tool/assistant` rows. Keep provider replay as a flattening step, and make interrupted/error turns durable while streaming.

- [x] Allow me to paste images i.e. [Image #1] [Image #2] [Image #3]. When I click on them, the image would be opened with my Finder (OS-specific)

- [x] Let's make the 'questions' a bit more mouse-driven.

- [x] Better question handling for skipped (Skipped, if I didn't press enter. like when I do `arrow right` immediately)

- [x] Scroll like herdr. Stuff I like: as thin, as tall (no arrows - currently ours also has no arrows but it was a hack, we just remove the arrows with "" chars so they still take a height. The one from herdr looks like it's a pure scrollbar thumb without arrows and thin enough that I like)

- [x] Highlight enhancements, if I click 1 place, then shift+click another. Treat it like the highlight in the browser that doesn't need a drag. Whatever I last clicked (without shift+click), treat it as the anchor for the "select start", and then whatever I shift+click after, treat it as a "select end" and autohighlight that part. (not supported)

- [x] Remote usage. Also talk about how to use for remote usages in the docs later. I can imagine multiple usecases. But this stands out in particular:
  - Remotely accessing crabcode on VPS / another device.
    - via another PC.
    - via phone.
  - More questions from me:
    - Do we need a separate app?
    - Should we recommend tailscale

- [x] File referencing with @

- [x] compaction

- [x] More mouse-friendly chat input box floating popovers i.e. `@` for files. `/` for commands. Requirements:
  - scroll w/ my mouse (no thumbs, just scroll)
  - click the item with my mouse

- [x] Benchmark script to test performance against opencode + codex in comparison. As cheaply as possible. Using the same models. It doesn't need to be a state-of-the-art benchmark. It just needs to test a couple of usual things i.e. small stuff, see if the agent is at least just as capable, because what we're chasing is kinda exactly just the same as codex/opencode, not better. The "better" will be in the UX, it will have the better UX changes I want. So I will want to also explicitly say it's a make-shift benchmark. I want the benchmark to output:
  - [x] Cost to test - this is just my personal add
  - [x] Idk what metric usually is used, to define "better". - the goal is crabcode will have the same score as the others.

- [x] Paste compaction i.e. [Pasted Content 1865 chars]

- [x] multiworkspace not working when I open other directories, I should be able to see in

- [x] better timeline highlighting of each "message"

- [x] Timeline highlighting of each message is not very accurate. It's accurate for "my messages". but for the ai responses, ai can seem to only highlight, even via `ctrl+x g`, the first few messages before a tool call happens. This is the same with the mouse hover effects. Expectations:
  - I hover/timelinehighlight my message, it encapsulates the entire message box (met)
  - I hover/timelinehighlight an ai response's message, it encapsulates the entire block, including tool calls, including the thinking, etc. (not met).
  - Essentially, I was imagining kinda the same as having a 'copy' button under each "message" record in the "messages: []" array in vercel ai sdk. That's kinda the point here. But for the limitations of TUIs, I want to just use a click on the entire message block (mine or the AI response, and open a dialog -- which is mostly the current behavior now)
  - UI bonus: the hover/timelinehighlight on ai response messages are more subtle, shouldnt use the primary color -- it looks TOO strong.

- [x] IN /models, can we use the ❤︎ icon, but colored pink. instead of the long heart + favorite indicator.

- [x] Reasoning effort adjustment in /models. Or a hotkey? In opencode it's ctrl-t.

- [x] /commands and custom commands.

- [x] Read my <> (ask for permission), deny. The chat doesn't get persisted, just gone. Please save everything before errors. So we can easily say "continue"

- [x] wysiwyg double escape to G

- [x] Compaction logic is a little broken. I did /compact, and the context compacted is ALWAYS at the bottom. instead of just at the part where it tried to compact the messages. Can we study how codex and opencode do it? meaning if I send a new message after compacting. The "compacted" label is still at the bottom of that most recent message

- [x] When a message is sent, the [Image #1] or [Image #2] tags, become just white, not the unique color we have for them in the chat input box.

- [x] Syntax highlighting during "Edited" tool calls for diffs. Check how Codex does it, because it has syntax highlighting for some reason--It's very clean.

- [x] I also think the /copy transcript should show "Edit" tool call results no? Right now it looks as simple as:
      **Tool Result**

**Tool:** edit

```
Replaced at line 239
```

- [x] Fix issue where it's not scrolling down consistently when new stream data comes down.

- [x] During delete in "sessions dialog" can we color the current "to-be-deleted" list item with red instead of the primary color. And since we're showing "Confirm ctrl+d" after pressing ctrl+d the first time, can we also "esc" to cancel (instead of close the session dialog?)

- [x] Don't log to app.log with logging.rs in the future, but in the future, add a custom env build flag so that when I `cargo install --path` with this flag, I include the "development release build" - so I can use the fast compiled version while having logs. And the normal cargo install --path, will still just be like a production build.

- [x] Don't prevent scroll when there's a permission required dialog.

- [x] Proper textwrapping of input for the input chatbox. I can paste a long string (that doesnt compact), or type a long sentence, and it won't wrap to the next line. It just has horizontal scrolling. I dont want horizontal scrolling.

- [x] Codex's "update plan" tool sometimes has a weird premble before the actual checklist shows... Is this relevant for crabcode? Should we update our tool? Can we do it too?

- [x] ~Pressing 'enter' while focusing on a grouplabel header for a "workspace". Make it show a dropdown on the right
  - Archive (can unarchive on new sessions)~ - dont do anymore
  - Collapse
  - Uncollapse

- [x] ~The footer note for the current cwd/workspace. It trims out the very start. i.e. `...ects/_gamedev/my-game:main`. Instead of this, please show the "between" truncation ??~ Just maybe, but maybe not.

- [x] Make tool calls be AS PERMISSIVE, as codex. Meaning won't have to ask me to "read" sometimes.

- [x] Mouse hover on "chat messages". So that when I click it, it opens the "timeline view" > enter option kinda thing. So it shows either the "Copy", "Fork", "Undo" actions, just like opencode.

- [x] I have a "complete", "error", "question" (use this in both 'question' and 'permission') sounds. I'd love for them to be bundled in, or at least downloaded by default via fetching from github raw link if it doesnt exist yet.

- [x] Like opencode, let's make a command palette via `ctrl+p`.
  - [x] Additionally, since the bottom area takes up too much space with `/ commands ctrl+x shortcuts tab agents ctrl+cc quit`. Let's reduce it to just `ctrl+p`?.

- [x] linebreaks aren't really reserved when I finally send the message in the chat UI. For instance I send,

```
I want
- [x] To do this

But I dont want to do this.
```

I get

```
I want - [x] To do this But I dont want to do this
```

- [x] Make the "bash" permission parity to codex. Also I currently dont see the command that it wants to run, so I'm kinda blind on what to run here.

- [x] When pasting images and it creates this [Image #1] tag, make it hoverable (just change the color, not the background), then once clicked, goes to the preferred editor of the user.
  - Multiple paths here:
    - Should it be configurable?
    - Autodetected depending on the tool used: i.e. if Wezterm, other terminals "open w/ Finder on mac, or native image opener". If inside Zed, open image with Zed. If inside VSCode/Cursor, open with that IDE. (Ambitious but idk if possible)

- [x] Make the permissions, config-driven customizable behavior. Make it like OpenCode, so we just link the docs for it in OpenCode.

- [x] View image locally tool, instead of read image.
- [x] Clickable paths.

- [x] When in another workspace and there are existing sessions in there and I opened /sessions, make that "workspace" the focus especially since the first page is at home.rs.

- [x] I want to make a SPECIAL integration w/ ollama, specifically the local ollama cli. Maybe `ollama ls` can be cached at runtime? and refreshed with refreshmodels? And a special provider place where I can do /connect on it. And it won't require any API keys? I wanna put it somewhere clean though... So that it doesn't really bother with the models.dev stuff, but just fits in cleanly. A /connect provider called 'Ollama (Local)' would be cool. API key-less should be possible too!

- [x] When clicking, it opens message actions.. Special case for UX: don't change the scroll value when it comes from "clicking a message".. But the other /timeline and ctrl+x g paths should be just fine.

- [x] Zed alert circle thing when asking permission or question, please emit it. Currently it's only on completions by default I think.

- [x] Let's refactor highlights so that "highlighting" doesn't copy immediately. But rather, show a little dropdown like this so that I have control if I wanna copy or not. I want this because there are some parts that are kinda bothersome especially for users with clipboard history, it just quickly bloats it.

- [x] Mouse scroll ux just like opencode, when highlighting. Needs to scroll when I reach edges as I drag and click.

- [x] Sometimes list items that have "bold" characters on them kinda break a new line between the number enum and the actual sentence i.e.
  - 1. <br/>**Replaced old indicator**.
  - Even though when I copy it looks like

        ```
        1. **Replaced the old loading indicator** (`SheetCopilot.tsx:757`) with a new shimmer bar that shows unconditionally whenever `loading()` is true. Text reads "Generating Response..." with an animated sweep across a 1px track.

        2. **Removed the `draftPatch` label** (`SheetCopilot.tsx:1273`) from the tool-call topline — the card now renders without the external label.

    G 3. **Added shimmer CSS** (`sheetpilot.css:1165`) with `@keyframes sheetpilot-shimmer-sweep` and the `.sheetpilot-generating*` layout.

        Build it with your usual `pnpm dev` / `pnpm build` to see the changes.
        ```

- [x] Make "▼ 💭 Thinking" rendered like this. And an accordion, so if I click it with my mouse, or with a special hotkey + command palette command. It can be toggled on and off.

- [x] Subagent UI view is not rendering the full table it seems like.. I always see this.. just the top.
  - `┌─────────────────────────┬────────────────────────────────────────────────────────────────────────────` - never the full table
  - Thouh I think the table does have content. I think it's just being weird.

- [x] When I do "Undo" on a message that had an attachment / image. It goes back to my input, but it isn't highlighted anymore, meaning that image is probably not visible anymore right? Is there a way to persist that?

- [x] Emit the same Loading stuff that codex does. So that Zed knows when the agent is "in progress".

- [x] During /compact, i can't queue a message, the same way I can usually queue messages while streaming. Btw except in compact, compaction has to be completely done before it registers my queued message until it's fully processed.

- [x] If I queue multiple messages for example 3x of nice. Let's make them a single message.

- [x] /fork command like codex.

- [x] TUI: When very last item in /models. If the very last item is a "Thinking" model, then I can't really see the "currently selected/focused" item (the last item), because the thinking left and right key covers it.

- [x] Improve the look of the "Permission required" dialog. Make it look more fitting for vertically aligned. Right now it's like on a flex row so the options are right to left. I like the look of "Question tool" dialog though. Any way we can get an inspired look out of that and use that for the "Permission required" dialog?

- [x] Let's make "active" models in /models dialog, not use the "Active" as a right-side label (but yes, make it searchable with 'active'). Why, because I want to see the "❤︎" still because right now it's being overwritten by "Active". But yeah just like searching "Favorite" I can look up my favorites, I want the same for "Active" still (which is already an observed behavior). Instead of "Active" as a label though, let's make it a symbol like OpenCode. In OpenCode, an "active" model has a different color of text when not highlighted yet (not the bg). And has a circle on the left side of it.

In fact I also want the same aesthetic for "Active" themes in /themes.

For the /connect dialog it's a little unique. Let's keep it. Before this, I wanted this but nevermind: ~Right now for connect, we use "🟢 Connected". But let's just use a ✔︎ on the left side. And since there's a lot of "Connected" items, no need to change the text color, we just want the ✔︎ as a green thing on the left side. Still searchable via "Connected"~

- [x] On Wezterm, I did `config.enable_kitty_keyboard = true`, now cmd+left or cmd+right doesn't work anymore (for skipping to the first/last character on the current line). Idk if this is a wezterm problem I need to patch or just on the wezterm lua side. Currently still works on the Zed Terminal btw. Where I observed: In chat inputs, any input fields.

- [x] Syntax highlighting for apply_patch and edit tool calls on the remote-client browser UI.

- [x] IN the "Overview" of ocnfiguration docs, mention which ones "merge" int he "File Layout", very useful info. Like a legend on the table with an emoji, then say "\* Merges across both"

- [x] Working websearch APIs
  - [x] exa-mcp - what opencode uses (default on). limits not visible. free, frictionless. no need for user to setup.
  - [x] tavily - I think has the best usage 1000q/m + free tier
  - [x] exa - has free + best quality, good 1000q/m + free tier, expensive after.
  - [x] ollama-cloud - okay quality + free tier, comes w/ model sub, so good plus.
  - [x] serpapi - free tier, low usage 250q/m.
  - [x] perplexity - ⚠️ not tested, assumed. baseline good quality, $2 less than exa
  - [x] brave - ⚠️ not tested, assumed
  - imo, codex had the best, but idk how to replicate that, they have their own internal.

- [x] I run two instances of crabcode (in the terminal), I change the thinking effort of the model, it affects all instances. Idk how they all cross communicate but that's both cool and weird. I do want to isolate the model use per instance tho. esp if 1 is running something different, I wouldnt want to change it. But if I change the model, it's fine.

- [x] In the desktop notifications, we say Response complete, can we also mention the name of the workspace.

- [x] ~Generate images with a codex exec call. No oauth spoofing needed. Just needs codex to be there.~ (For now, no... lol)

- [x] Scroll is not intuitive for interruptions. I'm using Logitech MX Master 3s, if I scroll the mouse SUPER down like at super speed. The scroll seems to just get stuck even if I scroll the other direction or just stop.
  - [x] Also slightly unperformant. I can definitely notice the animations slowing down when I scroll

- [x] Add commandcode.ai since opencode is not planning to.

- [x] /copy should now open a dialog more options to copy.
  - [x] Copy session transcript to clipboard (first option, so I can just double-enter for the default behavior)
  - [x] Copy session id
  - [x] Copy session title
  - This will essentially be the start of the many 'Action dialogs' that I have that don't need search, idk if I should have a name for them.
    But there's a similar 'Action dialog' that needs this UX: 'Message Actions'. It doesn't need to be searchable.
    And I also think it should have shortcuts for autoselecting them like if I press a certain character.

- [x] Tables bug:
  - [x] I see a...

  ```
   ## Fastest runtime per PDF

   ## ┌──────┬─────────────────────────────┬─────────────────────────────────────────────────────────────────────────────────┐
     │ Rank │ Approach                    │ Runtime notes                                                                   │
     ├──────┼─────────────────────────────┼──────────────────────
  ```

  - [ ] Fixed, but future, maybe offset scrolling whenever I resize... Since I kinda lose progress on where I was currently at, just because the text now wraps.

  - Not bug but improvGement: I want table wrapping by default. Currently it's truncated by default, but wrapping might look a lot better too.

- [x] ~Switching models mid-stream causes issues. Make sure what the stream uses, uses the same model / thinking effort, and it only changes after the next prompt or interruption. Cuz with openai, it fails when I change the thinking effort midway from when I started (i think, because of websockets).~ (noticed, it's a non-issue)

- [x] Bug in chat input. I click a character or anywhere in the input once... Then press up or down (not left or right, no bugs here). It kinda looks like I'm selecting the text where my cursor goes using up and down.

- [ ] Workspaces. Make sure to open the root, when opening crabcode?? maybe

- [x] I wanna see the loading all the time, no matter how shrunken the width of the terminal is.

- [ ] Archive a "workspace"

- [x] OpenCode has a /move command.

- [x] Add 'search' (like search the chat panel for some messages) cmd palette

- [x] perf: Optimize streaming

- [x] Minor UX improvement with chat input. When I press cmd-backspace repeatedly. For example 2x in a row. It doesn't act like opencode right now.
      The more appropriate behavior is I cmd-backspace (erases to the start of the line). press cmd-backspace again (at that point), it goes to the previous line, but doesn't erase the previous line, just puts my cursor at the rightmost of the prev line. I press cmd-backspace again, it goes to the left-most of the line.

- [x] publish to homebrew

- [x] xAI support

- [x] Minor bug fix, when I ctrl-d disconnect from a provider. Don't change focus of current selection cursor to the first item again. Just stay on the same item.

- [x] Remote client mobile keyboard layout bug: when the keyboard opens, the chat message panel does not shrink into a scrollable viewport and instead gets pushed upward. Fix the mobile layout so the main messages area resizes correctly and remains scrollable above the keyboard. I think there should be a max height for the entire screen, and not zoomable on phone.. Extremely freaking difficult btw. Because iOS does not support `viewport.interactive-widget: resizes-content`

- [x] Remote client latency UX: sending messages feels delayed. Explore optimistic UI so submitted messages appear immediately in the chat panel, and/or add clear loading/sending states while waiting for the server.

- [x] Remote client mobile chat input: pressing Enter should insert a newline instead of submitting, but only on mobile. Keep desktop Enter-to-submit behavior unchanged.

- [x] Remote client streaming thinking accordions: during streaming, thinking accordions keep opening/closing and animating in a distracting way. Keep them stable/unobtrusive while streaming so they do not repeatedly auto-toggle or animate. This might be a re-rendering issue, it happens especially when there are items being added into a Thinking item block.

- [x] remote client Minor UI improvement, when opening sessions on the side, I wanna see the current "workspace" immediately. So maybe whatever the active workspace is, scroll to it when the sessions is open

- [x] in remote client, I don't like that thinking, text response, and ran command/added/appliedpatch are structured this way.. No changes in thinking, but text response and the ran command/added/appliedpatch tool blocks... I don't like the fact that those ran command, added, and applied patch, etc are always located at the very bottom of the ai response message block. I'd like those added/applied patch,etc (The ones that aren't grouped into the thinking block), to be okay with being mixed into the text responses... So it makes more sense when the Agent is like:
  - text response: I'll do one more check by importing...
  - tool call: Ran command
  - text response: Fixed the white map! Also added the ...

I think this is how the TUI works already anyway right?

- [x] In remote client, can we remove the "left" spacing within the "Thought Process" accoridon's content. For example the ✔︎ and 🔍 blocks are wayy to spaced to the right, Maybe we can just make it the same padding-left /margin-left as the thoguht process. So that even the subblocks inside of stuff like "Read" or "Updated" look just fine and not too spaced.

- [x] Sometimes, apply_patch fails because it has no "context". First, I don't know what "context" means in this context. Also it might be related that sometimes, I find that crabcode sometimes just makes changes even after I touched it personally just for small tweaks. And tends to replace what I changed. Tends to happen if it touched that file before and is confident it can edit ti again.
      Execution error: Not found: Could not apply patch hunk: context was not found

- [x] In remote client... Quick change i want to request for the when the chat input is empty. I want to not see the "Stop" button, but see the send button. And it should queue it.

- [ ] data: Every now and then, prune the empty workspaces from the db. remove them.

- [x] Remote client minor "git" viewer. so users can see changes.

- [x] Image inputs are not supported by this model, just strip the image, so the chat can still work.

- [ ] Integrate `fff` for the search. Instead of nucleo??

- [x] opt-backspace and opt-left opt-right cmd-left, cmd-right doesn't work in "type your own answer" in the question dialog tool.

- [x] Add retries and backoff just like opencode. For instance, if it hits a rate limit / error caused by hitting the api too frequently. Make it backoff and show that it will submit the next call after a while, not immediately stop and show error. Make sure it shows an indicator (not in toast messages), about when it'll retry and stuff

- [x] Format the thinking results properly, make it look more like part of the TUI because currently it's literally just a concatenation of tokens to form a flat string without any formatting. I think it should be as markdowny as the markdown text responses. Also this might be minor or notadd a ctrl-e for expand thinking and collapse thinking. Also in ctrl-x e for that btw.

- [x] Noticed cmd-right for the chat input stops working when im already in a running session. cmd-right as in when I try to move my input cursor to the very right. It just doesnt work.

- [ ] low prio: title generation? maybe, maybenot, just a waste of tokens honestly.

- [ ] better permission asking. Like if it's going to ask for reading a directory, just ask multiple anyway so it doesnt need to ask so much?

- [x] More than build | plan agents. /agents command (like opencode, opens a select agent cmdk dialog basically). and inferring what's in the "agents" config. command palette as well.

- [x] I want the user input cursor + mousescroll behavior to be more like the browser. So the current issue is when I have multiple lines in the chat input and it's scrollable.
  - Current: When I scroll with my mouse, the cursor also changes along with the scroll view.
  - Expected: When I scroll with my mouse, the cursor stays in place, does not change, even when out of bounds. Then when I type, even when out of bounds, the scroll goes back to where my cursor is.

- [x] When I do /timeline or 'esc esc'... If the message is still streaming... or I just submitted just now.. Meaning in the current structure of the messages array.. The latest message is mine.. Then Don't show any "Agent: " in the list. Number 1, I don't wanna see it. Number 2, it doesn't exist. The message is not there.

- [x] We have a feature to make paths clickable... But dont't make this path clickable: `/Users/carlo/Desktop/Projects/crabenv/src`. It's a folder not a file.

- [x] Show the "favorited" models in the beginning when searching in /models.

- [ ] write_files tool doesn't have a diff.. but apply_patch, write_file, etc. do. currently what I see:

  ```
  ⬢ write_files files=[{"file_path":"packages/…
    └ packages/_project_/src/adapters/java/params.ts: created 2236 bytes
      packages/_project_/src/adapters/csharp/params.ts: created 2525 bytes
      packages/_project_/src/adapters/php/params.ts: created 2513 bytes
  ```
