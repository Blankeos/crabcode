[DONE]

# Fuzzy Search Implementation Plan (Using Nucleo)

## Overview

Implement high-performance fuzzy search using the `nucleo` crate from helix-editor. This will replace simple prefix matching with intelligent fuzzy matching similar to fzf, providing faster and more intuitive results.

## Library Details

- **Repo**: https://github.com/helix-editor/nucleo
- **Crate**: `nucleo` (high-level) or `nucleo-matcher` (low-level)
- **Performance**: ~6x faster than skim, significantly faster than fzf for low-selectivity patterns
- **Features**: Same scoring system as fzf, better Unicode handling, lock-free streaming

## Integration Points

### 1. File Autocomplete (`src/autocomplete/file.rs`)

**Current behavior**: Uses simple `starts_with` prefix matching
**Target behavior**: Use fuzzy matching with scores and rankings

#### Implementation Approach A: Low-level (nucleo-matcher)

```rust
use nucleo_matcher::{Matcher, Config, pattern::{Pattern, CaseMatching, Normalization}};

impl FileAuto {
    fn get_fuzzy_suggestions(&self, input: &str) -> Vec<(String, u16)> {
        let mut matcher = Matcher::new(Config::DEFAULT.match_paths());
        let pattern = Pattern::parse(input, CaseMatching::Ignore, Normalization::Smart);

        let entries = self.get_all_files();
        let mut scored: Vec<(String, u16)> = entries
            .iter()
            .filter_map(|name| {
                pattern
                    .match_item(name, &mut matcher)
                    .map(|score| (name.clone(), score))
            })
            .collect();

        scored.sort_by_key(|(_, score)| std::cmp::Reverse(*score));
        scored
    }
}
```

#### Implementation Approach B: High-level (nucleo)

```rust
use nucleo::{Nucleo, Config, pattern::CaseMatching, Utf32String};
use std::sync::Arc;

pub struct FileAuto {
    matcher: Nucleo<String>,
    injector: nucleo::Injector<String>,
}

impl FileAuto {
    pub fn new() -> Self {
        let notify = Arc::new(|| {});
        let mut matcher = Nucleo::new(
            Config::DEFAULT,
            notify,
            None, // num_threads
            1,    // columns
        );
        let injector = matcher.injector();

        // Pre-populate with files (lazy loading recommended)
        Self { matcher, injector }
    }

    pub fn get_suggestions(&self, input: &str) -> Vec<String> {
        self.matcher.pattern.reparse(
            input,
            CaseMatching::Ignore,
            nucleo::pattern::Normalization::Smart,
        );

        self.matcher.tick(10);
        let snapshot = self.matcher.snapshot();

        snapshot
            .matched_items(..)
            .take(20)
            .map(|item| item.data.clone())
            .collect()
    }
}
```

**Recommendation**: Start with **Approach A (nucleo-matcher)** because:

- Simpler integration for autocomplete
- Less boilerplate
- Sufficient for synchronous use cases
- Easier to test

### 2. Command Autocomplete (`src/autocomplete/command.rs`)

Apply same fuzzy matching logic to command suggestions.

### 3. Model Provider Selection (`src/model/providers/`)

If there's a picker for AI model providers, add fuzzy search there.

### 4. File Browser/Picker (Future Enhancement)

Create a dedicated fuzzy file picker component using nucleo's high-level API:

```rust
pub struct FilePicker {
    nucleo: Nucleo<FileInfo>,
    selected_index: usize,
}

impl FilePicker {
    pub fn select(&mut self) -> Option<PathBuf> {
        let snapshot = self.nucleo.snapshot();
        snapshot.get_matched_item(self.selected_index as u32)
            .map(|item| item.data.path.clone())
    }
}
```

## Implementation Steps

### Phase 1: Core Integration

1. **Add dependency** to `Cargo.toml`:

   ```toml
   nucleo-matcher = "0.3"  # Start with low-level API
   # Later: nucleo = "0.5"  # For high-level streaming API
   ```

2. **Refactor `FileAuto`** to support both fuzzy and exact modes:
   - Add `fuzzy: bool` configuration option
   - Keep existing prefix matching as fallback
   - Add `get_suggestions_with_scores()` method

3. **Update tests** in `src/autocomplete/file.rs`:
   - Add fuzzy match tests
   - Verify scoring behavior
   - Test case insensitivity

### Phase 2: Enhanced Features

1. **Unicode support**: Leverage nucleo's grapheme-aware matching
2. **Path-aware matching**: Use `Config::match_paths()` for better file matching
3. **Configurable scoring**: Expose `Config` options to user settings
4. **Index highlighting**: Use `fuzzy_indices()` to highlight matched characters in TUI

### Phase 3: Performance Optimization (if needed)

1. **Streaming with high-level nucleo**:
   - Lock-free injection of file list
   - Background threadpool for matching
   - Non-blocking UI updates

2. **Caching strategy**:
   - Cache `Utf32String` representations
   - Reuse `Matcher` instances
   - Lazy file scanning with incremental updates

3. **Frecency integration**:
   - Combine nucleo scores with frecency from `src/utils/frecency.rs`
   - Re-rank results by `(nucleo_score * 0.7) + (frecency_score * 0.3)`

## Key Configuration Options

From `nucleo_matcher::Config`:

- `match_paths()`: Better matching for file paths
- `prefer_prefix`: Prefer matches earlier in string
- `ignore_case`: Case-insensitive matching
- `bonus_*`: Score bonuses for word boundaries, capitals, etc.

Example:

```rust
let config = Config {
    match_paths: true,
    prefer_prefix: false,
    ignore_case: true,
    ..Config::DEFAULT
};
```

## Pattern Syntax (from nucleo)

Users can type special characters to control matching:

- `foo` - Fuzzy match
- `^foo` - Prefix match (anchor to start)
- `foo$` - Postfix match (anchor to end)
- `'foo` - Exact match
- `!foo` - Inverse match (exclude)
- `foo bar` - Multiple patterns (AND logic)
- `foo|bar` - Multiple patterns (OR logic)

## Testing Strategy

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use nucleo_matcher::{Matcher, Config};
    use nucleo_matcher::pattern::{Pattern, CaseMatching, Normalization};

    #[test]
    fn fuzzy_match_basic() {
        let mut matcher = Matcher::new(Config::DEFAULT);
        let pattern = Pattern::parse("fb", CaseMatching::Ignore, Normalization::Smart);
        let files = vec!["file_browser.rs", "foo_bar.rs", "fuzzbuzz.c"];

        let matches: Vec<_> = pattern
            .match_list(&files, &mut matcher)
            .into_iter()
            .collect();

        assert!(matches.len() > 0);
    }

    #[test]
    fn fuzzy_case_insensitive() {
        let mut matcher = Matcher::new(Config::DEFAULT);
        let pattern = Pattern::parse("FBR", CaseMatching::Ignore, Normalization::Smart);
        let matches = pattern.match_item("file_browser.rs", &mut matcher);
        assert!(matches.is_some());
    }
}
```

## Migration Plan

1. **Add feature flag**: `nucleo` feature in `Cargo.toml`
2. **Backward compatibility**: Keep old implementation behind `fuzzy` flag
3. **Gradual rollout**:
   - File autocomplete (Phase 1)
   - Command autocomplete (Phase 1)
   - Other pickers (Phase 2)
4. **Performance benchmarks**: Compare with current implementation
5. **User feedback**: Allow toggling fuzzy vs prefix in config

## Potential Challenges & Solutions

| Challenge            | Solution                                     |
| -------------------- | -------------------------------------------- |
| Large file trees     | Use streaming API with incremental injection |
| Real-time typing lag | Debounce pattern updates (50-100ms)          |
| Memory overhead      | Reuse `Matcher` instances, avoid recreating  |
| Unicode complexity   | Rely on nucleo's built-in handling           |
| Integration with TUI | Use tick() pattern for non-blocking updates  |

## Future Enhancements

1. **Multi-column matching**: Match on filename + path separately
2. **Custom bonus system**: Boost recent files from frecency
3. **Async streaming**: Integrate with tokio for non-blocking file discovery
4. **CLI picker**: Standalone `crabcode pick` command for file selection
5. **Preview window**: Show file preview in TUI while selecting

## References

- Nucleo README: https://github.com/helix-editor/nucleo
- Nucleo matcher crate: https://crates.io/crates/nucleo-matcher
- Helix editor implementation: https://github.com/helix-editor/helix
