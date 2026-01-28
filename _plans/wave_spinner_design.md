# ASCII Wave Loading Animation with Fade Effect

## Visual Concept

An 8-character wave animation where the leading edge is brightest/most opaque, and trailing characters fade in intensity. Uses the agent's theme color (Orange for Plan, Purple for Build).

## Color Palette

From `src/ui/components/chat.rs`:
- **Plan**: `Color::Rgb(255, 165, 0)` - Orange
- **Build**: `Color::Rgb(147, 112, 219)` - Purple

## Fade Gradient Design

For a 4-character wave moving across 8 positions, we need opacity levels:

```
Opacity Levels (for a 4-char wave):
- Position 0 (Lead):     100% opacity (brightest)
- Position 1:            75% opacity
- Position 2:            50% opacity  
- Position 3 (Trail):    25% opacity (faintest)
- Empty positions:       0% opacity (not shown)
```

## Animation Frames with Fade

The wave moves left to right, with each character having its own opacity:

```
Frame 0:  ⬝⬝⬝⬝⬝⬝⬝⬝  (all empty)

Frame 1:  ■⬝⬝⬝⬝⬝⬝⬝  
          [100%]        (single lead block)

Frame 2:  ■■⬝⬝⬝⬝⬝⬝
          [75%][100%]   (trail fading in)

Frame 3:  ■■■⬝⬝⬝⬝⬝
          [50%][75%][100%]

Frame 4:  ■■■■⬝⬝⬝⬝   (full wave, left-aligned)
          [25%][50%][75%][100%]

Frame 5:  ⬝■■■■⬝⬝⬝   (wave shifting right)
          [25%][50%][75%][100%]

Frame 6:  ⬝⬝■■■■⬝⬝
          [25%][50%][75%][100%]

Frame 7:  ⬝⬝⬝■■■■⬝
          [25%][50%][75%][100%]

Frame 8:  ⬝⬝⬝⬝■■■■   (full wave, right-aligned)
          [25%][50%][75%][100%]

Frame 9:  ⬝⬝⬝⬝⬝■■■   (shrinking from left)
          [50%][75%][100%]

Frame 10: ⬝⬝⬝⬝⬝⬝■■
          [75%][100%]

Frame 11: ⬝⬝⬝⬝⬝⬝⬝■
          [100%]

Frame 12: ⬝⬝⬝⬝⬝⬝⬝⬝  (all empty - transition point)

Frame 13: ⬝⬝⬝⬝⬝⬝⬝■  (reversing - right side)
          [100%]

Frame 14: ⬝⬝⬝⬝⬝⬝■■
          [100%][75%]

Frame 15: ⬝⬝⬝⬝⬝■■■
          [100%][75%][50%]

Frame 16: ⬝⬝⬝⬝■■■■   (full wave, right side)
          [100%][75%][50%][25%]
          ^ Note: reversed fade direction!

Frame 17: ⬝⬝⬝■■■■⬝   (shifting left)
          [100%][75%][50%][25%]

Frame 18: ⬝⬝■■■■⬝⬝
          [100%][75%][50%][25%]

Frame 19: ⬝■■■■⬝⬝⬝
          [100%][75%][50%][25%]

Frame 20: ■■■■⬝⬝⬝⬝   (full wave, left side)
          [100%][75%][50%][25%]

Frame 21: ■■■⬝⬝⬝⬝⬝   (shrinking from right)
          [100%][75%][50%]

Frame 22: ■■⬝⬝⬝⬝⬝⬝
          [100%][75%]

(back to Frame 1)
```

## Key Insight: Fade Direction

The fade direction **reverses** when the wave changes direction:

- **Moving Right** (Frames 1-11): Fade increases left→right (trail is dimmer)
- **Moving Left** (Frames 13-22): Fade increases right→left (trail is dimmer)

This creates the illusion that the wave has a "front" and "back" regardless of direction.

## Color Calculation

For a base color `(R, G, B)`, apply opacity multiplier:

```rust
fn apply_opacity(color: Color, opacity: f32) -> Color {
    match color {
        Color::Rgb(r, g, b) => Color::Rgb(
            (r as f32 * opacity) as u8,
            (g as f32 * opacity) as u8,
            (b as f32 * opacity) as u8,
        ),
        _ => color,
    }
}

// Opacity levels
const OPACITIES: [f32; 4] = [1.0, 0.75, 0.5, 0.25];
```

## Frame Data Structure

Each frame needs to track position AND opacity for each character:

```rust
struct FrameChar {
    symbol: char,      // '■' or '⬝'
    opacity: f32,      // 0.0 to 1.0
}

struct AnimationFrame {
    chars: [FrameChar; 8],
}
```

## Simplified Approach: Pre-computed Spans

Since ratatui uses `Span` with styles, pre-compute the styled spans:

```rust
pub struct WaveSpinner {
    frames: Vec<Vec<Span<'static>>>,
    current_frame: usize,
    last_update: Instant,
    base_color: Color,
}

impl WaveSpinner {
    const FRAME_DURATION: Duration = Duration::from_millis(100);
    
    pub fn new(base_color: Color) -> Self {
        let frames = Self::generate_frames(base_color);
        Self {
            frames,
            current_frame: 0,
            last_update: Instant::now(),
            base_color,
        }
    }
    
    fn generate_frames(base_color: Color) -> Vec<Vec<Span<'static>>> {
        // Generate all 22 frames with proper colors
    }
}
```

## Visual Preview (ASCII Art with opacity hints)

```
Direction: RIGHT →

Frame 4:  ■■■■⬝⬝⬝⬝
          ████░░░░   (100% 75% 50% 25% 0% 0% 0% 0%)

Frame 8:  ⬝⬝⬝⬝■■■■
          ░░░░████   (0% 0% 0% 0% 25% 50% 75% 100%)

Direction: LEFT ←

Frame 16: ⬝⬝⬝⬝■■■■
          ░░░░████   (0% 0% 0% 0% 100% 75% 50% 25%)
          
Frame 20: ■■■■⬝⬝⬝⬝
          ████░░░░   (100% 75% 50% 25% 0% 0% 0% 0%)
```

## Implementation Notes

1. **Symbol choice**: Use '■' for filled (with color), '⬝' for empty (dim gray or invisible)
2. **Empty characters**: Could show ⬝ in dark gray (10% opacity) or just spaces
3. **Smoothness**: 100ms per frame gives fluid motion
4. **Color switching**: Pass the agent color when creating the spinner

## Alternative: Gradient Wave

Instead of discrete opacity steps, could use more granular fade:

```rust
const OPACITIES: [f32; 4] = [1.0, 0.6, 0.35, 0.15];
```

Or even calculate based on distance from wave center for smoother gradient.

## Integration with Chat Component

Replace "Streaming..." text:

```rust
// In chat.rs, where streaming indicator is shown
let spinner = WaveSpinner::new(agent_color);
// Render spinner.spans() instead of static text
```

The spinner automatically picks up the current agent's color (Plan=Orange, Build=Purple).
