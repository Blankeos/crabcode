use ratatui::{
    layout::Rect,
    style::{Color, Style},
    Frame,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ScrollMetrics {
    pub content_len: usize,
    pub viewport_len: usize,
    pub offset: usize,
}

impl ScrollMetrics {
    pub(crate) fn new(content_len: usize, viewport_len: usize, offset: usize) -> Self {
        Self {
            content_len,
            viewport_len,
            offset,
        }
    }

    pub(crate) fn max_offset(self) -> usize {
        self.content_len.saturating_sub(self.viewport_len)
    }

    fn should_show(self) -> bool {
        self.viewport_len > 0 && self.max_offset() > 0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ScrollbarThumb {
    pub top: u16,
    pub len: u16,
}

pub(crate) fn scrollbar_thumb(metrics: ScrollMetrics, track: Rect) -> Option<ScrollbarThumb> {
    if !metrics.should_show() || track.width == 0 || track.height == 0 {
        return None;
    }

    let track_height = track.height as usize;
    let content_len = metrics.content_len.max(metrics.viewport_len);
    if content_len == 0 {
        return None;
    }

    let thumb_len = ((metrics.viewport_len * track_height) as f32 / content_len as f32)
        .round()
        .max(1.0)
        .min(track_height as f32) as usize;
    let max_thumb_top = track_height.saturating_sub(thumb_len);
    let max_offset = metrics.max_offset();
    let offset = metrics.offset.min(max_offset);
    let thumb_top = if max_thumb_top == 0 || max_offset == 0 {
        0
    } else {
        ((offset * max_thumb_top) as f32 / max_offset as f32)
            .round()
            .clamp(0.0, max_thumb_top as f32) as usize
    };

    Some(ScrollbarThumb {
        top: track.y + thumb_top as u16,
        len: thumb_len as u16,
    })
}

pub(crate) fn scrollbar_offset_from_row(metrics: ScrollMetrics, track: Rect, row: u16) -> usize {
    let Some(thumb) = scrollbar_thumb(metrics, track) else {
        return 0;
    };

    let clamped_row = row.clamp(track.y, track.y + track.height.saturating_sub(1));
    let row_offset = clamped_row.saturating_sub(track.y) as usize;
    let thumb_center = (thumb.len as usize) / 2;
    let desired_top = row_offset.saturating_sub(thumb_center);
    scrollbar_offset_from_thumb_top(metrics, track, desired_top)
}

fn scrollbar_offset_from_thumb_top(metrics: ScrollMetrics, track: Rect, thumb_top: usize) -> usize {
    let max_offset = metrics.max_offset();
    if max_offset == 0 || track.height == 0 {
        return 0;
    }

    let thumb_len = scrollbar_thumb(metrics, track)
        .map(|thumb| thumb.len as usize)
        .unwrap_or(1);
    let max_thumb_top = track.height as usize - thumb_len.min(track.height as usize);
    if max_thumb_top == 0 {
        return 0;
    }

    let desired_top = thumb_top.min(max_thumb_top);
    ((desired_top * max_offset) as f32 / max_thumb_top as f32).round() as usize
}

pub(crate) fn render_scrollbar(
    frame: &mut Frame,
    metrics: ScrollMetrics,
    track: Rect,
    track_color: Color,
    thumb_color: Color,
) {
    let Some(thumb) = scrollbar_thumb(metrics, track) else {
        return;
    };

    let buf = frame.buffer_mut();
    for y in track.y..track.y + track.height {
        let cell = &mut buf[(track.x, y)];
        cell.set_symbol("▕");
        cell.set_style(Style::default().fg(track_color));
    }
    for y in thumb.top..thumb.top + thumb.len {
        let cell = &mut buf[(track.x, y)];
        cell.set_symbol("▐");
        cell.set_style(Style::default().fg(thumb_color));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{backend::TestBackend, style::Color, Terminal};

    #[test]
    fn scrollbar_stays_hidden_when_content_fits() {
        let metrics = ScrollMetrics::new(5, 5, 0);
        let track = Rect::new(9, 4, 1, 5);

        assert_eq!(scrollbar_thumb(metrics, track), None);
    }

    #[test]
    fn scrollbar_thumb_reaches_bottom_when_scrolled_to_bottom() {
        let metrics = ScrollMetrics::new(25, 5, 20);
        let track = Rect::new(9, 4, 1, 5);

        let thumb = scrollbar_thumb(metrics, track).expect("thumb");
        assert_eq!(thumb.top + thumb.len, track.y + track.height);
    }

    #[test]
    fn scrollbar_offset_mapping_hits_top_middle_and_bottom() {
        let metrics = ScrollMetrics::new(25, 5, 0);
        let track = Rect::new(9, 4, 1, 5);

        assert_eq!(scrollbar_offset_from_row(metrics, track, 4), 0);
        assert_eq!(scrollbar_offset_from_row(metrics, track, 6), 10);
        assert_eq!(scrollbar_offset_from_row(metrics, track, 8), 20);
    }

    #[test]
    fn render_scrollbar_uses_thin_track_and_thumb_symbols() {
        let backend = TestBackend::new(1, 5);
        let mut terminal = Terminal::new(backend).expect("terminal");

        terminal
            .draw(|frame| {
                render_scrollbar(
                    frame,
                    ScrollMetrics::new(25, 5, 0),
                    Rect::new(0, 0, 1, 5),
                    Color::Reset,
                    Color::Reset,
                );
            })
            .expect("draw");

        let buffer = terminal.backend().buffer();
        assert_eq!(buffer[(0, 0)].symbol(), "▐");
        for y in 1..5 {
            assert_eq!(buffer[(0, y)].symbol(), "▕");
        }
    }
}
