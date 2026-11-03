//! Misure ottiche escluse: tempi CPU del renderer software e worker reale.
use super::*;
use lume_core::{Range, SelectionSet};
use lume_ui::editor::{EditorState, EditorView};

#[test]
#[ignore = "misura esplicita seriale: measure-multi"]
fn multi_misura_budget_worker_e_viewport() {
    use iced::advanced::{Renderer as _, Widget, widget::Tree};
    let mut app = app();
    let root = tempfile::tempdir().unwrap();
    write_and_open(&mut app, root.path(), "worker.txt", "foo");
    let id = app.active_doc().unwrap();
    let text = "x".repeat(10 * 1024 * 1024) + " foo";
    app.workspace.get_mut(id).unwrap().reload_from_str(&text);
    let len = app.workspace.get(id).unwrap().rope().len_chars();
    app.workspace
        .get_mut(id)
        .unwrap()
        .set_selection(Range::new(len - 3, len));
    let _ = runtime_file_tests();
    let threads = || {
        std::fs::read_dir("/proc/self/task")
            .map(|entries| entries.count())
            .ok()
    };
    let before = threads();
    let start = std::time::Instant::now();
    let result = primo_output_opzionale(app.next_occurrence_for(id)).unwrap();
    let micros = start.elapsed().as_micros();
    assert!(matches!(
        &result,
        Message::OccurrenceSelected(
            _,
            Err(lume_core::occurrence::SelectionActionError::NoMoreOccurrences)
        )
    ));
    assert!(app.multi_occurrence.is_some());
    let duplicate = app.next_occurrence_for(id);
    assert!(iced_runtime::task::into_stream(duplicate).is_none());
    let _ = app.update(result);
    assert!(app.multi_occurrence.is_none());
    eprintln!(
        "MULTICURSORE_MISURE:worker_10mib_us={micros},thread_prima={before:?},thread_dopo={:?},pool_bloccante_puo_restare_vivo=true",
        threads()
    );
    let theme = lume_theme::LumeTheme::default();
    let text = ("x".repeat(63) + "\n").repeat(16384);
    for count in [1, 1024] {
        let mut doc = lume_core::Document::from_str(&text, None, lume_core::DocId(700));
        doc.set_selections(
            SelectionSet::try_new(
                (0..count).map(Range::point).collect(),
                count - 1,
                doc.rope().len_chars(),
            )
            .unwrap(),
        );
        let mut view = EditorView::new(&doc, None, &theme, &[], |_| ());
        let mut renderer = iced::Renderer::Secondary(iced_tiny_skia::Renderer::new(
            iced::Font::default(),
            iced::Pixels(16.0),
        ));
        let mut tree = Tree::new(&view as &dyn Widget<(), iced::Theme, iced::Renderer>);
        tree.state
            .downcast_mut::<EditorState>()
            .is_focused
            .set(true);
        let size = iced::Size::new(800.0, 600.0);
        let bounds = iced::Rectangle::with_size(size);
        let node = view.layout(
            &mut tree,
            &renderer,
            &iced::advanced::layout::Limits::new(iced::Size::ZERO, size),
        );
        let mut samples = Vec::new();
        for sample in 0..23 {
            renderer.reset(bounds);
            let start = std::time::Instant::now();
            view.draw(
                &tree,
                &mut renderer,
                &iced::Theme::Dark,
                &iced::advanced::renderer::Style::default(),
                iced::advanced::Layout::new(&node),
                iced::mouse::Cursor::Unavailable,
                &bounds,
            );
            let iced::Renderer::Secondary(software) = &mut renderer else {
                unreachable!()
            };
            let pixels = iced_tiny_skia::window::compositor::screenshot(
                software,
                &iced_tiny_skia::graphics::Viewport::with_physical_size(
                    iced::Size::new(800, 600),
                    1.0,
                ),
                theme.bg,
            );
            let elapsed = start.elapsed();
            assert_eq!(pixels.len(), 800 * 600 * 4);
            if sample >= 3 {
                samples.push(elapsed);
            }
        }
        samples.sort();
        eprintln!(
            "MULTICURSORE_MISURE:modo=debug,renderer_software_cpu,byte=1048576,caret={count},mediana_us={},max_us={}",
            samples[10].as_micros(),
            samples[19].as_micros()
        );
    }
    eprintln!(
        "MULTICURSORE_MISURE:completo; esclusi latenza fisica tastiera_schermo e private_bytes Windows"
    );
}
