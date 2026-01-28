# Streaming Markdown Rendering Plan

## Overview

This document outlines the plan for adding **efficient** markdown rendering to AI responses in crabcode. The AI responses come in as streaming text chunks and need to be rendered with proper markdown formatting while maintaining 60fps UI performance.

## Performance Requirements

- **Target**: 60fps during streaming (16.67ms per frame budget)
- **Content size**: Up to 100KB responses (large code explanations)
- **Re-parsing**: Must avoid re-parsing entire content on every chunk
- **Memory**: Minimal allocations during streaming

## Current Architecture Analysis

### Message Flow
1. **Streaming**: `src/streaming/client.rs` → `StreamParser` parses SSE chunks into `StreamEvent::TextDelta`
2. **Storage**: `src/session/types.rs` → `Message` stores raw content string
3. **Rendering**: `src/ui/components/chat.rs` → `Chat::format_message()` called every frame

### Current Rendering (simplified)
```rust
// In Chat::format_message() for Assistant messages:
let content = message.content.clone();
let wrapped_lines = textwrap::wrap(&content, max_width);
for line in wrapped_lines {
    lines.push(Line::from(line.to_string()));
}
```

**Problem**: Current implementation is O(n) where n = content length, but it's just simple text wrapping. Markdown parsing is more expensive.

## Research: Markdown Parsing Performance

### `tui-markdown` Internals
From source analysis (`tui-markdown/src/lib.rs`):

```rust
pub fn from_str(input: &str) -> Text<'_> {
    let parser = Parser::new_ext(input, parse_opts);  // O(n) - single pass
    let mut writer = TextWriter::new(parser, options.styles.clone());
    writer.run();  // Iterates events, builds Text
    writer.text
}
```

**Performance characteristics**:
- `pulldown-cmark::Parser`: Single-pass, streaming parser (very fast)
- Event processing: One pass through events
- For 10KB markdown: ~1-2ms parse time
- For 100KB markdown: ~10-20ms parse time

**Conclusion**: Re-parsing 100KB every frame = 10-20ms = 50-100fps, which is acceptable but not ideal. For larger content or slower CPUs, we need optimization.

## Decision: Use `tui-markdown` with Incremental Caching

### Strategy: Block-Based Incremental Rendering

Instead of re-parsing the entire content on every chunk, we:

1. **Parse once per chunk** (unavoidable - need to see new content)
2. **Cache parsed blocks** that haven't changed
3. **Only re-render** from the last modified block

### Architecture

```
┌─────────────────────────────────────────────────────────────┐
│              StreamingMarkdownRenderer                       │
├─────────────────────────────────────────────────────────────┤
│  Content Buffer          Parsed Blocks Cache                 │
│  ┌─────────────────┐    ┌──────────────────────────────┐    │
│  │ Full markdown   │    │ Block 1: Text (lines 1-5)    │    │
│  │ text accumulated│    │ Block 2: Code (lines 6-15)   │    │
│  │                 │    │ Block 3: List (lines 16-20)  │    │
│  │                 │    │ Block 4: Text (lines 21-??)  │◄───┼── Current
│  └─────────────────┘    └──────────────────────────────┘    │   streaming
│           │                          ▲                      │   block
│           │    Parse new chunk       │                      │
│           └──────────────────────────┘                      │
│                          Only re-parse from changed block   │
└─────────────────────────────────────────────────────────────┘
```

## Implementation Design

### Core Data Structures

```rust
// src/ui/markdown/streaming.rs

use pulldown_cmark::{Event, Parser, Tag, TagEnd};
use ratatui::text::{Line, Text};
use std::sync::Arc;

/// A parsed block of markdown that can be cached
#[derive(Clone)]
pub struct MarkdownBlock {
    /// Start position in the raw markdown string
    pub start_byte: usize,
    /// End position in the raw markdown string
    pub end_byte: usize,
    /// The rendered lines for this block
    pub lines: Vec<Line<'static>>,
    /// Block type for debugging/optimization
    pub block_type: BlockType,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum BlockType {
    Paragraph,
    Heading(u8),  // level 1-6
    CodeBlock,
    BlockQuote,
    List,
    ListItem,
    Rule,
}

/// Efficient streaming markdown renderer
pub struct StreamingMarkdownRenderer {
    /// Accumulated markdown content
    content: String,
    /// Cached parsed blocks
    blocks: Vec<MarkdownBlock>,
    /// Current block being built during streaming
    current_block: Option<PartialBlock>,
    /// Whether we need a full re-parse
    dirty_from: Option<usize>,
}

/// A block that's currently being built
struct PartialBlock {
    start_byte: usize,
    block_type: BlockType,
    events: Vec<Event<'static>>,
}
```

### Key Methods

```rust
impl StreamingMarkdownRenderer {
    pub fn new() -> Self {
        Self {
            content: String::new(),
            blocks: Vec::new(),
            current_block: None,
            dirty_from: None,
        }
    }

    /// Append new content from stream
    pub fn append(&mut self, chunk: &str) {
        let old_len = self.content.len();
        self.content.push_str(chunk);
        
        // Mark blocks from this position as dirty
        self.mark_dirty_from(old_len);
    }

    /// Get rendered text (called every frame)
    pub fn render(&mut self) -> Text<'_> {
        self.ensure_parsed();
        
        // Collect all lines from blocks
        let mut all_lines = Vec::new();
        for block in &self.blocks {
            all_lines.extend(block.lines.clone());
        }
        
        Text::from(all_lines)
    }

    /// Mark blocks dirty from a byte position
    fn mark_dirty_from(&mut self, byte_pos: usize) {
        // Find which block contains this position
        let dirty_idx = self.blocks.iter()
            .position(|b| b.end_byte >= byte_pos)
            .unwrap_or(self.blocks.len());
        
        // Remove all blocks from this point
        self.blocks.truncate(dirty_idx);
        self.dirty_from = Some(
            self.blocks.last().map(|b| b.end_byte).unwrap_or(0)
        );
    }

    /// Ensure all content is parsed
    fn ensure_parsed(&mut self) {
        let start_pos = self.dirty_from.unwrap_or(self.content.len());
        if start_pos >= self.content.len() {
            return; // Nothing to parse
        }

        // Parse only from dirty position
        let parser = Parser::new_ext(&self.content[start_pos..], PARSE_OPTIONS);
        let mut block_accumulator = BlockAccumulator::new(start_pos);
        
        for event in parser {
            if block_accumulator.process_event(event) {
                // Block complete, save it
                if let Some(block) = block_accumulator.finish_block() {
                    self.blocks.push(block);
                }
            }
        }
        
        self.dirty_from = None;
    }
}
```

### Block Accumulator

```rust
/// Accumulates events into blocks
struct BlockAccumulator {
    start_byte: usize,
    current_events: Vec<Event<'static>>,
    block_stack: Vec<Tag<'static>>,
}

impl BlockAccumulator {
    fn new(start_byte: usize) -> Self {
        Self {
            start_byte,
            current_events: Vec::new(),
            block_stack: Vec::new(),
        }
    }

    /// Process an event, returns true if a block was completed
    fn process_event(&mut self, event: Event<'static>) -> bool {
        match &event {
            Event::Start(tag) => {
                if self.block_stack.is_empty() && !self.current_events.is_empty() {
                    // Starting new top-level block, finish current
                    return true;
                }
                self.block_stack.push(tag.clone());
            }
            Event::End(_) => {
                self.block_stack.pop();
                if self.block_stack.is_empty() {
                    // Completed a top-level block
                    return true;
                }
            }
            _ => {}
        }
        
        self.current_events.push(event);
        false
    }

    fn finish_block(&mut self) -> Option<MarkdownBlock> {
        if self.current_events.is_empty() {
            return None;
        }

        // Render events to lines using tui-markdown's approach
        let lines = render_events_to_lines(&self.current_events);
        
        let block = MarkdownBlock {
            start_byte: self.start_byte,
            end_byte: self.start_byte + self.current_events.len(), // Approximate
            lines,
            block_type: detect_block_type(&self.current_events),
        };
        
        self.current_events.clear();
        self.start_byte = block.end_byte;
        
        Some(block)
    }
}
```

## Integration with Chat Component

### Modified Chat Structure

```rust
// src/ui/components/chat.rs

use crate::ui::markdown::StreamingMarkdownRenderer;

#[derive(Debug, Clone, Default)]
pub struct Chat {
    pub messages: Vec<Message>,
    // ... other fields ...
    
    /// Markdown renderer for the last (streaming) message
    /// Only created when we have an incomplete assistant message
    streaming_renderer: Option<StreamingMarkdownRenderer>,
}
```

### Modified Message Rendering

```rust
impl Chat {
    fn format_message(
        &mut self,
        message: &Message,
        max_width: usize,
        idx: usize,
        model: &str,
        colors: &ThemeColors,
    ) -> Vec<Line> {
        match message.role {
            MessageRole::Assistant => {
                // Check if this is the last (potentially streaming) message
                let is_last = idx == self.messages.len() - 1;
                let is_streaming = is_last && !message.is_complete;
                
                if is_streaming {
                    // Use streaming renderer
                    self.format_streaming_message(message, max_width, colors)
                } else {
                    // Use one-shot renderer for complete messages
                    self.format_complete_markdown(&message.content, max_width, colors)
                }
            }
            // ... other roles ...
        }
    }

    fn format_streaming_message(
        &mut self,
        message: &Message,
        max_width: usize,
        colors: &ThemeColors,
    ) -> Vec<Line> {
        // Get or create renderer
        let renderer = self.streaming_renderer.get_or_insert_with(
            StreamingMarkdownRenderer::new
        );
        
        // Update content if changed
        if renderer.content() != message.content {
            renderer.reset();
            renderer.append(&message.content);
        }
        
        // Get rendered text
        let text = renderer.render();
        
        // Convert to lines (respecting width)
        text.lines.into_iter()
            .flat_map(|line| wrap_line(line, max_width))
            .collect()
    }

    fn format_complete_markdown(
        &self,
        content: &str,
        max_width: usize,
        colors: &ThemeColors,
    ) -> Vec<Line> {
        // For complete messages, use tui_markdown directly
        // No need for incremental caching
        let text = tui_markdown::from_str(content);
        
        text.lines.into_iter()
            .flat_map(|line| wrap_line(line, max_width))
            .collect()
    }
}
```

## Alternative: Simpler Approach (Recommended for MVP)

The block-based approach is complex. For MVP, use this simpler strategy:

### Simple Caching Strategy

```rust
pub struct SimpleStreamingRenderer {
    content: String,
    cached_text: Option<Text<'static>>,
    content_hash: u64,
}

impl SimpleStreamingRenderer {
    pub fn append(&mut self, chunk: &str) {
        self.content.push_str(chunk);
        self.cached_text = None; // Invalidate cache
    }

    pub fn render(&mut self) -> &Text<'_> {
        if self.cached_text.is_none() {
            // Only re-parse when content changed
            self.cached_text = Some(tui_markdown::from_str(&self.content));
        }
        self.cached_text.as_ref().unwrap()
    }
}
```

**Performance**: 
- Re-parses only when content changes (not every frame)
- At 60fps with 10 chunks/second, parses 10 times instead of 60 times
- 6x reduction in parsing overhead

## Implementation Phases

### Phase 1: Basic Integration (MVP)
- [ ] Add `tui-markdown` dependency
- [ ] Create `SimpleStreamingRenderer` with content-change caching
- [ ] Integrate into `Chat::format_message()`
- [ ] Only parse when content changes, not every frame

**Expected performance**: 10KB @ 10 chunks/sec = ~20ms parsing/sec = 50fps minimum

### Phase 2: Frame Budget Optimization
- [ ] Add frame time budget check
- [ ] If parsing takes >8ms, defer to next frame
- [ ] Show "parsing..." indicator if needed

### Phase 3: Block-Based Incremental (Future)
- [ ] Implement full block-based caching
- [ ] Only re-parse from changed block
- [ ] Handle large (100KB+) responses efficiently

### Phase 4: Syntax Highlighting
- [ ] Enable `highlight-code` feature
- [ ] Cache syntax highlighter instances
- [ ] Lazy load syntax definitions

## Performance Benchmarks

Target metrics:

| Content Size | Parse Time | Frames @ 60fps | Strategy |
|--------------|-----------|----------------|----------|
| 1KB | 0.1ms | 600 | Simple caching |
| 10KB | 1ms | 60 | Simple caching |
| 50KB | 5ms | 12 | Simple caching |
| 100KB | 10ms | 6 | Block-based |
| 500KB | 50ms | 1.2 | Block-based + deferred |

## Testing Strategy

### Performance Tests
```rust
#[test]
fn test_render_performance_10kb() {
    let content = generate_markdown(10_000); // 10KB
    let mut renderer = SimpleStreamingRenderer::new();
    renderer.append(&content);
    
    let start = Instant::now();
    let _text = renderer.render();
    let elapsed = start.elapsed();
    
    assert!(elapsed < Duration::from_millis(5), 
            "10KB parse took {:?}", elapsed);
}

#[test]
fn test_streaming_append_performance() {
    let mut renderer = SimpleStreamingRenderer::new();
    
    // Simulate streaming 100 chunks
    for i in 0..100 {
        let chunk = format!("Paragraph {}\n\n", i);
        renderer.append(&chunk);
        
        let start = Instant::now();
        let _text = renderer.render();
        let elapsed = start.elapsed();
        
        // Each append+render should be < 1ms amortized
        assert!(elapsed < Duration::from_millis(1));
    }
}
```

### Stress Tests
- [ ] 100KB response with code blocks
- [ ] Rapid streaming (100 chunks/second)
- [ ] Concurrent scroll during streaming

## Risks and Mitigations

| Risk | Impact | Mitigation |
|------|--------|------------|
| Parsing 100KB+ content too slow | High | Block-based incremental + deferred parsing |
| Memory growth with large content | Medium | Clear renderer when message completes |
| Incomplete markdown looks weird | Low | Acceptable UX; resolves on completion |
| Syntax highlighting slow | Medium | Lazy load; cache highlighters |

## References

- `tui-markdown` source: https://github.com/joshka/tui-markdown
- `pulldown-cmark` docs: https://docs.rs/pulldown-cmark/
- Current chat rendering: `src/ui/components/chat.rs`
