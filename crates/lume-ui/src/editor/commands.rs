//! Contratto tra il widget editor e l'applicazione: il widget NON muta mai
//! il `Document`, pubblica solo `EditorCommand` che l'app applica.

/// Movimento del cursore richiesto da tastiera.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Motion {
    Left,
    Right,
    WordLeft,
    WordRight,
    Up,
    Down,
    Home,
    End,
    DocStart,
    DocEnd,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageDirection {
    Up,
    Down,
}

/// Comando pubblicato dal widget verso l'applicazione.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EditorCommand {
    Insert(String),
    Backspace,
    Delete,
    Enter,
    Tab,
    Move(Motion, /* extend: */ bool),
    MovePage {
        direction: PageDirection,
        lines: usize,
        extend: bool,
    },
    /// Char index assoluto già risolto dal widget via hit-test.
    SetCursor {
        pos: usize,
        extend: bool,
    },
    ToggleCursor {
        pos: usize,
    },
    AddNextOccurrence,
    AddCursorLine {
        above: bool,
    },
    KeepPrimary,
    Undo,
    Redo,
    SelectAll,
    Save,
    /// Testo già letto dalla clipboard nel widget.
    Paste(String),
    /// Il widget ha già scritto la clipboard: l'app cancella la selezione.
    Cut,
    /// Il widget ha già scritto la clipboard: no-op lato app.
    Copy,
    /// Tasto destro nel testo: **posizione nel documento** già risolta con
    /// hit-test, più il punto sullo schermo dove ancorare il menù.
    ///
    /// ATTENZIONE: `mouse_area::on_right_press` esiste, ma **non dà la posizione nel
    /// documento**: darebbe un menù «Vai alla definizione» che non sa di
    /// quale simbolo. Il widget invece il hit-test lo fa già, e questo ramo
    /// riusa lo stesso `resolve_position` del tasto sinistro.
    ///
    /// Le coordinate sono **intere** di proposito: `EditorCommand` deriva
    /// `Eq` e `f32` non ce l'ha. Un menù non ha bisogno del subpixel.
    ContextMenu {
        pos: usize,
        screen_x: i32,
        screen_y: i32,
    },
    /// Ctrl+Spazio: chiedi i completamenti qui.
    TriggerComplete,
    /// Freccia su nel popup di completamento.
    PopupUp,
    /// Freccia giù nel popup di completamento.
    PopupDown,
    /// Invio o Tab nel popup: scrivi la voce selezionata.
    PopupAccept,
    /// Esc: chiudi popup e scheda hover.
    PopupDismiss,
    /// Il mouse è (`Some`) o non è più (`None`) sopra una parola: il char
    /// dove comincia. Pubblicato solo al cambio di parola.
    HoverAt(Option<usize>),
    /// Clic sulla barretta di una riga collegata a un appunto (indice 0-based).
    NoteMarkerClicked {
        line: usize,
    },
    /// Clic sulla freccia del folding nel margine della riga `line` (0-based).
    FoldClicked {
        line: usize,
    },
}
