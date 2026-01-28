use ratatui::style::{Color, Style};
use ratatui::text::Span;
use std::time::{Duration, Instant};

#[derive(Debug)]
pub struct WaveSpinner {
    frames: Vec<Vec<Span<'static>>>,
    current_frame: usize,
    last_update: Instant,
    base_color: Color,
    frame_duration: Duration,
}

impl WaveSpinner {
    const DEFAULT_FRAME_DURATION: Duration = Duration::from_millis(50);
    const OPACITIES: [f32; 5] = [1.0, 0.8, 0.6, 0.4, 0.2];

    pub fn new(base_color: Color) -> Self {
        let frames = Self::generate_frames(base_color);
        Self {
            frames,
            current_frame: 0,
            last_update: Instant::now(),
            base_color,
            frame_duration: Self::DEFAULT_FRAME_DURATION,
        }
    }

    pub fn with_speed(base_color: Color, frame_duration_ms: u64) -> Self {
        let mut spinner = Self::new(base_color);
        spinner.set_speed(frame_duration_ms);
        spinner
    }

    pub fn set_speed(&mut self, frame_duration_ms: u64) {
        self.frame_duration = Duration::from_millis(frame_duration_ms);
    }

    pub fn update(&mut self) {
        let elapsed = self.last_update.elapsed();
        if elapsed >= self.frame_duration {
            // Advance multiple frames if enough time has passed (prevents
            // catching up after lag, but also prevents going too fast)
            let frames_to_advance =
                (elapsed.as_millis() / self.frame_duration.as_millis()) as usize;
            self.current_frame = (self.current_frame + frames_to_advance) % self.frames.len();
            self.last_update = Instant::now();
        }
    }

    pub fn spans(&self) -> Vec<Span<'static>> {
        self.frames[self.current_frame].clone()
    }

    pub fn set_color(&mut self, base_color: Color) {
        if self.base_color != base_color {
            self.base_color = base_color;
            self.frames = Self::generate_frames(base_color);
        }
    }

    fn generate_frames(base_color: Color) -> Vec<Vec<Span<'static>>> {
        let mut frames = Vec::new();

        // Moving right (frames 0-12) - 5 block wave
        // Frame 0: single lead block at position 0
        frames.push(Self::create_frame(&[(0, 0)], base_color));

        // Frame 1: two blocks at positions 0,1
        frames.push(Self::create_frame(&[(0, 1), (1, 0)], base_color));

        // Frame 2: three blocks at positions 0,1,2
        frames.push(Self::create_frame(&[(0, 2), (1, 1), (2, 0)], base_color));

        // Frame 3: four blocks at positions 0,1,2,3
        frames.push(Self::create_frame(
            &[(0, 3), (1, 2), (2, 1), (3, 0)],
            base_color,
        ));

        // Frame 4: five blocks at positions 0,1,2,3,4 (full wave, left-aligned)
        frames.push(Self::create_frame(
            &[(0, 4), (1, 3), (2, 2), (3, 1), (4, 0)],
            base_color,
        ));

        // Frame 5: five blocks at positions 1,2,3,4,5 (shifting right)
        frames.push(Self::create_frame(
            &[(1, 4), (2, 3), (3, 2), (4, 1), (5, 0)],
            base_color,
        ));

        // Frame 6: five blocks at positions 2,3,4,5,6
        frames.push(Self::create_frame(
            &[(2, 4), (3, 3), (4, 2), (5, 1), (6, 0)],
            base_color,
        ));

        // Frame 7: five blocks at positions 3,4,5,6,7 (full wave, right-aligned)
        frames.push(Self::create_frame(
            &[(3, 4), (4, 3), (5, 2), (6, 1), (7, 0)],
            base_color,
        ));

        // Frame 8: four blocks at positions 4,5,6,7 (shrinking from left)
        frames.push(Self::create_frame(
            &[(4, 3), (5, 2), (6, 1), (7, 0)],
            base_color,
        ));

        // Frame 9: three blocks at positions 5,6,7
        frames.push(Self::create_frame(&[(5, 2), (6, 1), (7, 0)], base_color));

        // Frame 10: two blocks at positions 6,7
        frames.push(Self::create_frame(&[(6, 1), (7, 0)], base_color));

        // Frame 11: single block at position 7
        frames.push(Self::create_frame(&[(7, 0)], base_color));

        // Frame 11: empty (transition point)
        frames.push(Self::create_empty_frame());

        // PAUSE: Hold empty for 2 frames before bouncing back
        frames.push(Self::create_empty_frame());
        frames.push(Self::create_empty_frame());

        // Moving left (frames 15-26) - fade direction reverses
        // Frame 15: single lead block at position 7 (moving left, so lead is at right)
        frames.push(Self::create_frame(&[(7, 0)], base_color));

        // Frame 16: two blocks at positions 6,7 (lead at 7, trail at 6)
        frames.push(Self::create_frame(&[(6, 1), (7, 0)], base_color));

        // Frame 17: three blocks at positions 5,6,7
        frames.push(Self::create_frame(&[(5, 2), (6, 1), (7, 0)], base_color));

        // Frame 18: four blocks at positions 4,5,6,7
        frames.push(Self::create_frame(
            &[(4, 3), (5, 2), (6, 1), (7, 0)],
            base_color,
        ));

        // Frame 19: five blocks at positions 3,4,5,6,7 (full wave, right-aligned, reversed fade)
        frames.push(Self::create_frame(
            &[(3, 0), (4, 1), (5, 2), (6, 3), (7, 4)],
            base_color,
        ));

        // Frame 20: five blocks at positions 2,3,4,5,6 (shifting left)
        frames.push(Self::create_frame(
            &[(2, 0), (3, 1), (4, 2), (5, 3), (6, 4)],
            base_color,
        ));

        // Frame 21: five blocks at positions 1,2,3,4,5
        frames.push(Self::create_frame(
            &[(1, 0), (2, 1), (3, 2), (4, 3), (5, 4)],
            base_color,
        ));

        // Frame 22: five blocks at positions 0,1,2,3,4 (full wave, left-aligned, reversed fade)
        frames.push(Self::create_frame(
            &[(0, 0), (1, 1), (2, 2), (3, 3), (4, 4)],
            base_color,
        ));

        // Frame 23: four blocks at positions 0,1,2,3 (shrinking from right)
        frames.push(Self::create_frame(
            &[(0, 0), (1, 1), (2, 2), (3, 3)],
            base_color,
        ));

        // Frame 24: three blocks at positions 0,1,2
        frames.push(Self::create_frame(&[(0, 0), (1, 1), (2, 2)], base_color));

        // Frame 25: two blocks at positions 0,1
        frames.push(Self::create_frame(&[(0, 0), (1, 1)], base_color));

        // PAUSE: Hold empty for 6 frames before looping (was 2, now longer pause)
        frames.push(Self::create_empty_frame());
        frames.push(Self::create_empty_frame());
        frames.push(Self::create_empty_frame());
        frames.push(Self::create_empty_frame());
        frames.push(Self::create_empty_frame());
        frames.push(Self::create_empty_frame());

        frames
    }

    fn create_frame(positions: &[(usize, usize)], base_color: Color) -> Vec<Span<'static>> {
        let mut chars: Vec<Span<'static>> = Vec::with_capacity(8);

        for i in 0..8 {
            if let Some((_, opacity_idx)) = positions.iter().find(|(pos, _)| *pos == i) {
                let opacity = Self::OPACITIES[*opacity_idx];
                let color = Self::apply_opacity(base_color, opacity);
                chars.push(Span::styled("■", Style::default().fg(color)));
            } else {
                chars.push(Span::styled("⬝", Style::default().fg(Color::DarkGray)));
            }
        }

        chars
    }

    fn create_empty_frame() -> Vec<Span<'static>> {
        (0..8)
            .map(|_| Span::styled("⬝", Style::default().fg(Color::DarkGray)))
            .collect()
    }

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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_wave_spinner_new() {
        let spinner = WaveSpinner::new(Color::Rgb(255, 165, 0));
        assert_eq!(spinner.frames.len(), 34);
        assert_eq!(spinner.current_frame, 0);
    }

    #[test]
    fn test_wave_spinner_custom_speed() {
        let mut spinner = WaveSpinner::with_speed(Color::Rgb(255, 165, 0), 200);
        assert_eq!(spinner.frame_duration, Duration::from_millis(200));

        spinner.set_speed(75);
        assert_eq!(spinner.frame_duration, Duration::from_millis(75));
    }

    #[test]
    fn test_wave_spinner_spans_length() {
        let spinner = WaveSpinner::new(Color::Rgb(255, 165, 0));
        let spans = spinner.spans();
        assert_eq!(spans.len(), 8);
    }

    #[test]
    fn test_apply_opacity() {
        let color = Color::Rgb(255, 165, 0);
        let faded = WaveSpinner::apply_opacity(color, 0.5);
        assert_eq!(faded, Color::Rgb(127, 82, 0));
    }
}
