# Polish Chat Experience - Implementation Plan

## Overview

Complete UI refactoring of the chat experience in crabcode to improve readability, add scrollability, and enhance AI response metadata display.

---

## 1. Scrollable Chat Viewport

### Requirements

- Chat screen should have a scrollable view
- Support mouse wheel scrolling
- Support scroll thumb dragging
- Reference: `dialog.rs` has existing implementation
- **Difference**: No up/down arrow key symbols (unlike dialog)

### Implementation Approach

- Use ratatui's `Scrollable` or custom scroll implementation
- Track scroll offset state
- Handle mouse events for wheel and thumb dragging
- Calculate content height vs viewport height for scroll bar sizing

### Files to Modify

- `src/ui/chat.rs` - Main chat UI component

---

## 2. Remove [You] and [AI] Indicators

### User Messages

- Remove `[You]` prefix
- Wrap user messages in a styled box/container
- Add padding inside the box
- Add left border to distinguish from AI responses
- **Border Color**: Match the current Agent mode when the message was sent (Plan or Build color)
- **Background**: Solid dark color (same as modal backgrounds)

### AI Messages

- Remove `[AI]` prefix
- No wrapping box - display content directly
- Add metadata line below each AI response
- **Metadata Icon Color**: Match the Agent mode color (Plan or Build)

---

## 3. AI Response Metadata Display

### Format

```
▣  Plan • glm-4.7 • 23t/s • 1.2s
```

### Components

- **Agent Icon** (▣) - Visual indicator, colored by Agent mode (bright color)
- **Agent Type** - "Plan" or "Build" (bright color, matches icon)
- **Separator** (•) - Dimmed color
- **Model ID** - The model identifier used (e.g., "glm-4.7") - Dimmed color
- **Tokens/Second** - Performance metric (e.g., "23t/s") - Dimmed color
- **Duration** - Total generation time (e.g., "1.2s") - Dimmed color

### Color Coding

- **Plan Mode**: Cyan/Blue color
- **Build Mode**: Green/Yellow color
- User message left border uses the Agent mode color active when the message was sent

### Separator

- Use `•` (bullet) as separator between fields

---

## 4. Database Migration for Tokens/Second

### Migration Details

- **Version**: v1 migration (greenfield approach)
- **New Field**: `tokens_per_second` (REAL/float)
- **Location**: Add to existing message/response table

### Schema Addition

```sql
-- Add to v1 migration script
ALTER TABLE responses ADD COLUMN token_count INTEGER;  -- Total tokens generated
ALTER TABLE responses ADD COLUMN duration_ms INTEGER;  -- Total generation time in milliseconds
ALTER TABLE messages ADD COLUMN agent_mode TEXT;  -- 'plan' or 'build' for user message border color
```

**Note**: Tokens/second is calculated on-the-fly as `token_count / (duration_ms / 1000.0)`, not stored in DB.

### Data Flow

1. AI generates response
2. Track start time and token count during streaming
3. Calculate total duration: `end_time - start_time`
4. Store in database:
   - `token_count`: Total tokens generated
   - `duration_ms`: Total generation time in milliseconds
5. Calculate tokens/second on-the-fly for display: `token_count / (duration_ms / 1000.0)`
6. Display in UI metadata line

### Real-time Streaming Display
- Show current tokens/second next to "Streaming..." label while response is being generated
- Format: `Streaming... 23t/s`
- Update in real-time as tokens arrive
- Final token_count and duration_ms stored in database after completion

---

## 5. UI Layout Structure

### Message Layout

```
┌─────────────────────────────────────┐
│ User message content here...        │  ← User: Box with left border (colored by agent mode)
│ More content...                     │     Background: solid dark (modal color)
└─────────────────────────────────────┘

AI response content here...
More AI content...
Streaming... 23t/s                     ← Real-time tokens/sec while streaming
▣  Plan • glm-4.7 • 23t/s • 1.2s       ← AI: No box, metadata line below
                                     ▣ and "Plan" in bright agent color
                                     • separators, model, tokens/s, duration in dimmed color

┌─────────────────────────────────────┐
│ Next user message...                │  ← Border color based on current agent mode
└─────────────────────────────────────┘
```

---

## 6. Files to Create/Modify

### New Files

- None (all modifications to existing)

### Files to Modify

1. **`src/ui/chat.rs`**
   - Implement scrollable viewport
   - Update user message rendering (boxed style)
   - Update AI message rendering (no box + metadata)
   - Add metadata line component

2. **`src/persistence/migrations.rs`**
   - Add v1 migration for:
     - `token_count` column (responses table)
     - `duration_ms` column (responses table)
     - `agent_mode` column (messages table)

3. **`src/model/types.rs` or response types**
   - Add `token_count` field to response structures
   - Add `duration_ms` field to response structures
   - Add `agent_mode` field to message structures

4. **`src/llm/client.rs` or generation code**
   - Calculate and pass tokens/second metric

---

## 7. Technical Considerations

### Scroll Implementation

- Track `scroll_offset: usize` in chat state
- Handle `MouseEventKind::ScrollUp/ScrollDown`
- Render scroll thumb based on content/viewport ratio
- Clamp scroll offset to valid range
- **Scroll limit**: Allow scrolling to the very beginning of the session (no artificial limit)

### Styling

- **User box left border**: Colored by the Agent mode active when message was sent
  - Plan mode → Cyan/Blue border
  - Build mode → Green/Yellow border
- **User box background**: Solid dark color (same as modal backgrounds)
- **AI metadata icon (▣)**: Bright agent mode color
- **AI metadata agent type** (Plan/Build): Bright agent mode color
- **AI metadata separators, model ID, tokens/s, duration**: Dimmed/subdued color
- Consistent with existing crabcode theme

### Performance

- Virtualize rendering if message list grows large
- Only render visible messages + small buffer

---

## 8. Implementation Order

1. **Database migration** - Add `token_count` and `duration_ms` columns
2. **Update data structures** - Add field to response types
3. **Update LLM client** - Calculate and store tokens/second
4. **Implement scrollable viewport** - Core scrolling functionality
5. **Redesign user messages** - Box with border/padding
6. **Redesign AI messages** - Remove box, add metadata line
7. **Testing** - Verify scroll, styling, and metadata display

---

## 9. Decisions Made

1. ✅ ~~Should the metadata line be clickable/interactive?~~ → **No, not interactive**
2. ✅ ~~Color scheme for user message box border?~~ → **Matched to current agent mode when sent**
3. ✅ ~~Should tokens/second be cached or calculated on-the-fly?~~ → **Calculated per-message, displayed real-time during streaming**
4. ✅ ~~Maximum scrollback history limit?~~ → **Scroll to beginning of session, no artificial limit**
5. ✅ ~~Should user messages also show metadata (timestamp, etc.)?~~ → **No timestamps needed**
6. ✅ ~~How to handle the first user message (no preceding AI to get color from)?~~ → **Each user message stores the agent mode that was active when it was sent**
