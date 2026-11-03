#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
use lume_terminal::{TerminalModel, TerminalPoint, TerminalScroll, TerminalSize};
use std::sync::Arc;

#[test]
fn snapshot_preserves_grid_style_link_and_alt_screen() {
    let model = TerminalModel::new(TerminalSize::new(3, 20));
    model
        .advance(b"\x1b[31;1mred\x1b[0m \x1b]8;;https://example.com\x1b\\link\x1b]8;;\x1b\\")
        .unwrap();

    let normal = model.snapshot().unwrap();
    assert!(normal.cells.iter().any(|cell| cell.bold));
    let links = normal
        .cells
        .iter()
        .filter_map(|cell| cell.hyperlink.as_ref())
        .collect::<Vec<_>>();
    assert!(links.len() >= 2);
    assert!(std::sync::Arc::ptr_eq(links[0], links[1]));

    assert_eq!(
        model
            .hyperlink_at(TerminalPoint::new(0, 4))
            .unwrap()
            .as_deref(),
        Some("https://example.com")
    );

    model.advance(b"\x1b[?1049hALT").unwrap();
    assert!(model.snapshot().unwrap().alternate_screen);
    model.advance(b"\x1b[?1h\x1b[?2004h").unwrap();
    let modes = model.snapshot().unwrap();
    assert!(modes.application_cursor);
    assert!(modes.bracketed_paste);
}

#[test]
fn selection_and_scrollback_use_alacritty_state() {
    let model = TerminalModel::new(TerminalSize::new(2, 8));
    model.advance(b"one\r\ntwo\r\nthree").unwrap();
    model.scroll(TerminalScroll::PageUp).unwrap();
    assert!(model.snapshot().unwrap().display_offset > 0);

    model.start_selection(TerminalPoint::new(0, 0)).unwrap();
    model.update_selection(TerminalPoint::new(0, 2)).unwrap();
    assert_eq!(model.selected_text().unwrap().as_deref(), Some("one"));

    model.clear_selection().unwrap();
    assert!(model.selected_text().unwrap().is_none());
}

#[test]
fn unicode_clusters_preserve_wide_and_combining_cells() {
    let model = TerminalModel::new(TerminalSize::new(2, 12));
    model.advance("界e\u{301}".as_bytes()).unwrap();

    let snapshot = model.snapshot().unwrap();
    let wide = snapshot
        .cells
        .iter()
        .find(|cell| cell.character == '界')
        .unwrap();
    assert!(wide.wide);
    let combining = snapshot
        .cells
        .iter()
        .find(|cell| cell.character == 'e')
        .unwrap();
    assert_eq!(combining.zerowidth, vec!['\u{301}']);
}

#[test]
fn oversized_hyperlinks_are_not_copied_into_snapshots() {
    let model = TerminalModel::new(TerminalSize::new(2, 12));
    let uri = format!("https://example.com/{}", "x".repeat(2048));
    let payload = format!("\x1b]8;;{uri}\x1b\\link\x1b]8;;\x1b\\");
    model.advance(payload.as_bytes()).unwrap();

    assert!(
        model
            .snapshot()
            .unwrap()
            .cells
            .iter()
            .all(|cell| cell.hyperlink.is_none())
    );
}

/// Ridurre lo scrollback taglia la storia **e non la ridà**: è una perdita,
/// non un'ibernazione. Tornato al limite pieno, la storia ricresce da lì.
#[test]
fn ridurre_lo_scrollback_taglia_la_storia_e_non_la_restituisce() {
    let model = TerminalModel::new(TerminalSize::new(2, 20));
    let righe: String = (0..3000).map(|i| format!("riga {i}\r\n")).collect();
    model.advance(righe.as_bytes()).unwrap();
    assert!(model.history_size().unwrap() > 1000);

    model.set_scrollback(1000).unwrap();
    assert_eq!(model.history_size().unwrap(), 1000);

    // Si tagliano le righe più vecchie, non le recenti: in cima alla storia
    // rimasta c'è «riga 1999» (3.001 righe totali, 2 sullo schermo, 1.000 di
    // storia), non «riga 0».
    model.scroll(TerminalScroll::Top).unwrap();
    let snapshot = model.snapshot().unwrap();
    let mut prima_riga: Vec<_> = snapshot
        .cells
        .iter()
        .filter(|cell| cell.point.row == 0)
        .collect();
    prima_riga.sort_by_key(|cell| cell.point.col);
    let testo: String = prima_riga.iter().map(|cell| cell.character).collect();
    assert_eq!(
        testo.trim_end(),
        "riga 1999",
        "restano le righe più recenti"
    );
    model.scroll(TerminalScroll::Bottom).unwrap();

    model
        .set_scrollback(lume_terminal::SCROLLBACK_DEFAULT)
        .unwrap();
    assert_eq!(
        model.history_size().unwrap(),
        1000,
        "la storia tagliata non torna"
    );

    model.advance(b"ancora\r\nuna\r\n").unwrap();
    assert!(
        model.history_size().unwrap() > 1000,
        "col limite pieno la storia ricresce"
    );
}

#[test]
fn snapshot_cached_riusa_lo_stesso_arc_finche_nulla_cambia() {
    let model = TerminalModel::new(TerminalSize::new(3, 20));
    model.advance(b"ciao").unwrap();

    let a = model.snapshot_cached().unwrap();
    let b = model.snapshot_cached().unwrap();
    assert!(
        Arc::ptr_eq(&a, &b),
        "nessuna mutazione: nessuna nuova allocazione"
    );
    assert_eq!(
        *a,
        model.snapshot().unwrap(),
        "il frame in cache è quello vero"
    );
}

#[test]
fn ogni_mutazione_invalida_la_cache_e_il_frame_nuovo_e_quello_giusto() {
    let model = TerminalModel::new(TerminalSize::new(2, 8));
    model.advance(b"one\r\ntwo\r\nthree").unwrap();

    // scroll
    let prima = model.snapshot_cached().unwrap();
    model.scroll(TerminalScroll::PageUp).unwrap();
    let dopo = model.snapshot_cached().unwrap();
    assert!(!Arc::ptr_eq(&prima, &dopo));
    assert!(dopo.display_offset > prima.display_offset);
    assert_eq!(*dopo, model.snapshot().unwrap());

    // selezione: creata, estesa, cancellata
    model.start_selection(TerminalPoint::new(0, 0)).unwrap();
    let iniziata = model.snapshot_cached().unwrap();
    assert!(!Arc::ptr_eq(&dopo, &iniziata));
    model.update_selection(TerminalPoint::new(0, 2)).unwrap();
    let estesa = model.snapshot_cached().unwrap();
    assert!(!Arc::ptr_eq(&iniziata, &estesa));
    assert_ne!(iniziata.selection, estesa.selection);
    model.clear_selection().unwrap();
    let cancellata = model.snapshot_cached().unwrap();
    assert!(!Arc::ptr_eq(&estesa, &cancellata));
    assert!(cancellata.selection.is_none());

    // nuovo output
    model.scroll(TerminalScroll::Bottom).unwrap();
    let fermo = model.snapshot_cached().unwrap();
    model.advance(b"\r\nfour").unwrap();
    let nuovo = model.snapshot_cached().unwrap();
    assert!(!Arc::ptr_eq(&fermo, &nuovo));
    assert!(
        model
            .visible_text()
            .unwrap()
            .iter()
            .any(|riga| riga == "four")
    );
    assert_eq!(*nuovo, model.snapshot().unwrap());

    // resize
    model.resize(TerminalSize::new(5, 30)).unwrap();
    let grande = model.snapshot_cached().unwrap();
    assert!(!Arc::ptr_eq(&nuovo, &grande));
    assert_eq!((grande.rows, grande.cols), (5, 30));

    // limite della storia: il contenuto della griglia cambia anche qui
    let righe: String = (0..300).map(|i| format!("riga {i}\r\n")).collect();
    model.advance(righe.as_bytes()).unwrap();
    let piena = model.snapshot_cached().unwrap();
    model.set_scrollback(100).unwrap();
    let ridotta = model.snapshot_cached().unwrap();
    assert!(!Arc::ptr_eq(&piena, &ridotta));
    assert_eq!(model.history_size().unwrap(), 100);
}

#[test]
fn la_cache_e_condivisa_fra_i_cloni_del_modello() {
    // il thread di lettura della PTY tiene un clone del modello e chiama advance
    let model = TerminalModel::new(TerminalSize::new(2, 8));
    let lettore = model.clone();
    let a = model.snapshot_cached().unwrap();
    lettore.advance(b"xyz").unwrap();
    let b = model.snapshot_cached().unwrap();
    assert!(
        !Arc::ptr_eq(&a, &b),
        "l'output arrivato dal clone invalida la cache"
    );
    assert!(
        model
            .visible_text()
            .unwrap()
            .iter()
            .any(|riga| riga == "xyz")
    );
}

#[test]
fn release_snapshot_cache_libera_la_memoria_e_il_frame_si_ricrea() {
    let model = TerminalModel::new(TerminalSize::new(3, 20));
    assert!(!model.snapshot_cache_held(), "all'inizio nessuna cache");

    let a = model.snapshot_cached().unwrap();
    assert!(model.snapshot_cache_held());
    assert_eq!(Arc::strong_count(&a), 2, "uno è nostro, uno è della cache");

    model.release_snapshot_cache();
    assert!(!model.snapshot_cache_held());
    assert_eq!(
        Arc::strong_count(&a),
        1,
        "il modello non trattiene più il frame"
    );

    let b = model.snapshot_cached().unwrap();
    assert_eq!(*a, *b, "stesso contenuto");
    assert!(!Arc::ptr_eq(&a, &b), "ma è un'allocazione nuova");
}

/// Misura (non un controllo): quanto costa un frame da zero rispetto a un
/// frame dalla cache, su una griglia da 50 righe per 200 colonne piena di
/// testo. Si lancia a mano, prima e dopo:
/// `cargo test -p lume-terminal --test terminal_model costo_dello_snapshot -- --ignored --nocapture`
#[test]
#[ignore = "misura a mano: stampa i tempi, non è un controllo di CI"]
fn costo_dello_snapshot_da_zero_e_dalla_cache() {
    let model = TerminalModel::new(TerminalSize::new(50, 200));
    for riga in 0..50 {
        model
            .advance(format!("riga {riga:02} {}\r\n", "abcdefghij".repeat(15)).as_bytes())
            .unwrap();
    }
    let giri = 500;

    let inizio = std::time::Instant::now();
    for _ in 0..giri {
        std::hint::black_box(model.snapshot().unwrap());
    }
    let da_zero = inizio.elapsed();

    let inizio = std::time::Instant::now();
    for _ in 0..giri {
        std::hint::black_box(model.snapshot_cached().unwrap());
    }
    let dalla_cache = inizio.elapsed();

    eprintln!(
        "{giri} frame da 50x200: da zero {da_zero:?} ({:?} l'uno), dalla cache {dalla_cache:?} ({:?} l'uno)",
        da_zero / giri,
        dalla_cache / giri
    );
    assert!(dalla_cache < da_zero);
}
