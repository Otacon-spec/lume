//! Calcoli puri del viewport editor, indipendenti da Iced.

/// Metriche usate da scroll, caret reveal e PageUp/PageDown.
pub struct ViewportMetrics;

impl ViewportMetrics {
    pub fn page_lines(height: f32) -> usize {
        (height / super::LINE_H).floor().max(1.0) as usize
    }

    pub fn clamp_x(scroll: f32, content: f32, viewport: f32) -> f32 {
        scroll.clamp(0.0, (content - viewport).max(0.0))
    }

    pub fn ensure_x_visible(scroll: f32, viewport: f32, caret: f32, margin: f32) -> f32 {
        if caret < scroll + margin {
            (caret - margin).max(0.0)
        } else if caret > scroll + viewport - margin {
            caret - viewport + margin
        } else {
            scroll
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use super::ViewportMetrics;
    use crate::editor::LINE_H;

    #[test]
    fn page_lines_come_from_the_visible_height() {
        assert_eq!(ViewportMetrics::page_lines(LINE_H * 8.8), 8);
        assert_eq!(ViewportMetrics::page_lines(0.0), 1);
    }

    #[test]
    fn horizontal_scroll_clamps_and_reveals_the_caret() {
        assert_eq!(ViewportMetrics::clamp_x(900.0, 600.0, 300.0), 300.0);
        assert_eq!(
            ViewportMetrics::ensure_x_visible(0.0, 300.0, 420.0, 12.0),
            132.0
        );
        assert_eq!(
            ViewportMetrics::ensure_x_visible(132.0, 300.0, 80.0, 12.0),
            68.0
        );
    }
}
