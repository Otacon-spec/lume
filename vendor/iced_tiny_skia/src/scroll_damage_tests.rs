//! Differential oracle for partial redraw after changing visible line numbers.
use super::*;
use crate::core::renderer::Renderer as _;
use crate::core::text::{Paragraph as _, Renderer as _};

fn frame(
    renderer: &mut Renderer,
    first: usize,
    bounds: Rectangle,
    line_h: f32,
    font_size: f32,
) -> Vec<Paragraph> {
    renderer.reset(bounds);
    renderer.fill_quad(
        renderer::Quad {
            bounds,
            ..Default::default()
        },
        Color::BLACK,
    );
    let paragraphs: Vec<_> = (first..first + 34)
        .map(|line| {
            Paragraph::with_text(crate::core::Text {
                content: line.to_string().as_str(),
                bounds: Size::new(42.0, line_h),
                size: Pixels(font_size),
                line_height: crate::core::text::LineHeight::Absolute(Pixels(line_h)),
                font: Font::MONOSPACE,
                align_x: crate::core::text::Alignment::Left,
                align_y: crate::core::alignment::Vertical::Top,
                shaping: crate::core::text::Shaping::Basic,
                wrapping: crate::core::text::Wrapping::None,
            })
        })
        .collect();
    for (row, paragraph) in paragraphs.iter().enumerate() {
        renderer.fill_quad(
            renderer::Quad {
                bounds: Rectangle {
                    x: 0.0,
                    y: row as f32 * line_h,
                    width: 80.0,
                    height: line_h,
                },
                ..Default::default()
            },
            if row == if first == 1 { 0 } else { 31 } {
                Color::from_rgb8(36, 40, 46)
            } else {
                Color::from_rgb8(30, 33, 38)
            },
        );
        renderer.fill_paragraph(
            paragraph,
            Point::new(70.0 - paragraph.min_bounds().width, row as f32 * line_h),
            Color::WHITE,
            bounds,
        );
    }
    paragraphs
}

fn check_case(line_h: f32, font_size: f32, scale: f32) {
    let size = Size::new((160.0 * scale) as u32, (680.0 * scale) as u32);
    let bounds = Rectangle::with_size(Size::new(160.0, 680.0));
    let viewport = Viewport::with_physical_size(size, scale);
    let mut renderer = Renderer::new(Font::MONOSPACE, Pixels(font_size));
    let mut pixels = tiny_skia::Pixmap::new(size.width, size.height).unwrap();
    let mut mask = tiny_skia::Mask::new(size.width, size.height).unwrap();
    let mut owners = frame(&mut renderer, 1, bounds, line_h, font_size);
    renderer.draw(
        &mut pixels.as_mut(),
        &mut mask,
        &viewport,
        &[bounds],
        Color::BLACK,
    );
    let identical = renderer.layers().to_vec();
    assert!(
        Layer::damage(&identical[0], &identical[0]).is_empty(),
        "unchanged paragraph clones must not repaint"
    );
    for first in [38, 72, 106] {
        let previous = renderer.layers().to_vec();
        let successor = frame(&mut renderer, first, bounds, line_h, font_size);
        let damage = graphics::damage::diff(
            &previous,
            renderer.layers(),
            |layer| vec![layer.bounds],
            Layer::damage,
        );
        let grouped = graphics::damage::group(damage, bounds);
        renderer.draw(
            &mut pixels.as_mut(),
            &mut mask,
            &viewport,
            &grouped,
            Color::BLACK,
        );
        let mut full = tiny_skia::Pixmap::new(size.width, size.height).unwrap();
        let mut full_mask = tiny_skia::Mask::new(size.width, size.height).unwrap();
        renderer.draw(
            &mut full.as_mut(),
            &mut full_mask,
            &viewport,
            &[bounds],
            Color::BLACK,
        );
        let differing = pixels
            .data()
            .chunks_exact(4)
            .zip(full.data().chunks_exact(4))
            .enumerate()
            .filter_map(|(index, (a, b))| (a != b).then_some(index))
            .collect::<Vec<_>>();
        assert!(
            differing.is_empty(),
            "partial redraw differs at first line {first}: {} pixels; first={:?}; damage={grouped:?}",
            differing.len(),
            differing
                .first()
                .map(|index| (index % size.width as usize, index / size.width as usize))
        );
        owners = successor;
    }
    assert_eq!(owners.len(), 34);
}

#[test]
fn scrolling_line_numbers_partial_redraw_matches_full_redraw() {
    for scale in [1.0, 1.25, 1.5, 2.0] {
        check_case(20.0, 14.0, scale);
        check_case(20.625, 12.5, scale);
    }
}
