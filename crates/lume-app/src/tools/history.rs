//! Tool «Cronologia»: aperture, salvataggi e commit reali (più recenti in
//! alto), persistiti nella sessione (ultimi 100). La lista appartiene
//! all'app; il tool la mostra e chiede l'apertura del file al click.

use std::cell::Cell;

use iced::widget::{button, column, container, row, scrollable, text};
use iced::{Alignment, Element, Length, Padding};
use lume_store::{HistoryAction, HistoryRecord};
use lume_ui::icons::Icon;
use lume_ui::styles;
use lume_ui::tool::{Tool, ToolCtx, ToolEvent, ToolId, ToolRequest};

use super::gitlog::civil_from_days;

pub const ID: ToolId = ToolId("history");
/// Voci tenute in memoria (lo snapshot ne salva 100).
pub const MAX_ENTRIES: usize = 200;
/// Voci salvate nello snapshot di sessione.
pub const PERSISTED_ENTRIES: usize = 100;

#[derive(Clone, Debug)]
pub enum Event {
    Replace(Vec<HistoryRecord>),
    Open(usize),
}

pub struct HistoryTool {
    items: Vec<HistoryRecord>,
    /// Istante di lettura (secondi Unix) e fuso letto; vedi `cached_offset`.
    offset_cache: Cell<Option<(u64, i32)>>,
}

impl HistoryTool {
    pub fn new() -> Self {
        Self {
            items: Vec::new(),
            offset_cache: Cell::new(None),
        }
    }
}

impl Default for HistoryTool {
    fn default() -> Self {
        Self::new()
    }
}

/// Inserisce una voce in testa; una voce identica (azione, etichetta, file)
/// già presente viene spostata in testa con il nuovo istante. Massimo
/// `MAX_ENTRIES`.
pub fn push(items: &mut Vec<HistoryRecord>, entry: HistoryRecord) {
    items.retain(|e| !(e.action == entry.action && e.label == entry.label && e.path == entry.path));
    items.insert(0, entry);
    items.truncate(MAX_ENTRIES);
}

pub fn action_label(action: HistoryAction) -> &'static str {
    match action {
        HistoryAction::Opened => "Apertura",
        HistoryAction::Saved => "Salvataggio",
        HistoryAction::Committed => "Commit",
        HistoryAction::AgentWrote => "Agente: scrittura",
        HistoryAction::AgentRan => "Agente: comando",
    }
}

/// Etichetta temporale: «HH:MM» se oggi, «ieri», altrimenti «dd/MM»
/// (giorni civili nel fuso indicato da `offset` in secondi).
pub fn time_label(at: u64, now: u64, offset: i32) -> String {
    let local = |t: u64| t as i64 + i64::from(offset);
    let day = local(at).div_euclid(86_400);
    let today = local(now).div_euclid(86_400);
    if day == today {
        let secs = local(at).rem_euclid(86_400);
        format!("{:02}:{:02}", secs / 3600, (secs % 3600) / 60)
    } else if day + 1 == today {
        "ieri".into()
    } else {
        let (_, month, d) = civil_from_days(day);
        format!("{d:02}/{month:02}")
    }
}

/// Offset del fuso di sistema in secondi (jiff, già nel grafo delle
/// dipendenze via gix).
pub fn local_offset_seconds() -> i32 {
    jiff::tz::TimeZone::system()
        .to_offset(jiff::Timestamp::now())
        .seconds()
}

/// Ogni quanto si rilegge il fuso di sistema. L'ora legale cambia due volte
/// l'anno: rileggerlo a ogni frame è lavoro a ogni messaggio.
pub const OFFSET_TTL_SECONDS: u64 = 60;

/// Il fuso, riletto solo se la lettura in cache ha almeno `OFFSET_TTL_SECONDS`
/// o se l'orologio è andato indietro. La scadenza si calcola **qui**, alla
/// lettura: nessun timer.
pub fn cached_offset(
    cache: &Cell<Option<(u64, i32)>>,
    now: u64,
    read: impl FnOnce() -> i32,
) -> i32 {
    if let Some((at, offset)) = cache.get()
        && now >= at
        && now - at < OFFSET_TTL_SECONDS
    {
        return offset;
    }
    let offset = read();
    cache.set(Some((now, offset)));
    offset
}

pub fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or_default()
}

impl Tool for HistoryTool {
    fn id(&self) -> ToolId {
        ID
    }

    fn title(&self) -> &'static str {
        "Cronologia"
    }

    fn icon(&self) -> Icon {
        Icon::History
    }

    fn feature(&self) -> Option<&'static str> {
        Some("cronologia")
    }

    fn view<'a>(&'a self, ctx: &ToolCtx<'a>) -> Element<'a, ToolEvent> {
        let t = ctx.theme;
        if self.items.is_empty() {
            return container(text("Nessuna attività recente.").size(12).color(t.muted))
                .padding(Padding::from([2.0, 12.0]))
                .into();
        }
        let now = now_unix();
        let offset = cached_offset(&self.offset_cache, now, local_offset_seconds);
        let mut list = column![].spacing(2);
        for (index, entry) in self.items.iter().enumerate() {
            let content = row![
                text(time_label(entry.at, now, offset))
                    .size(12)
                    .color(t.muted)
                    .width(44),
                container(
                    text(format!("{} · {}", action_label(entry.action), entry.label))
                        .size(12)
                        .color(t.ui)
                        .wrapping(text::Wrapping::None)
                )
                .clip(true)
                .width(Length::Fill),
            ]
            .spacing(4)
            .align_y(Alignment::Center);
            let mut row_button = button(content)
                .width(Length::Fill)
                .padding(Padding::from([3, 6]))
                .style(styles::hover_row(t.ui, t.hover, None, 4.0));
            if entry.path.is_some() {
                row_button = row_button.on_press(ToolEvent::new(Event::Open(index)));
            }
            list = list.push(row_button);
        }
        scrollable(container(list).width(Length::Fill).padding(Padding {
            top: 2.0,
            right: 12.0,
            bottom: 12.0,
            left: 12.0,
        }))
        .height(Length::Fill)
        .into()
    }

    fn update(&mut self, event: &ToolEvent) -> Vec<ToolRequest> {
        match event.downcast::<Event>() {
            Some(Event::Replace(items)) => {
                self.items = items.clone();
                Vec::new()
            }
            Some(Event::Open(index)) => self
                .items
                .get(*index)
                .and_then(|entry| entry.path.clone())
                .map(ToolRequest::OpenFile)
                .into_iter()
                .collect(),
            None => Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use std::path::PathBuf;

    use super::*;

    fn entry(at: u64, action: HistoryAction, label: &str) -> HistoryRecord {
        HistoryRecord {
            at,
            action,
            label: label.into(),
            path: Some(PathBuf::from("C:/repo/src/main.rs")),
        }
    }

    #[test]
    fn push_dedup_e_limite() {
        let mut items = Vec::new();
        push(&mut items, entry(1, HistoryAction::Opened, "main.rs"));
        push(&mut items, entry(2, HistoryAction::Saved, "main.rs"));
        push(&mut items, entry(3, HistoryAction::Opened, "main.rs"));
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].at, 3, "la voce identica risale in testa");
        assert_eq!(items[0].action, HistoryAction::Opened);
        for i in 0..(MAX_ENTRIES + 10) {
            push(
                &mut items,
                entry(100 + i as u64, HistoryAction::Saved, &format!("f{i}")),
            );
        }
        assert_eq!(items.len(), MAX_ENTRIES);
    }

    #[test]
    fn etichette_temporali_relative_al_giorno_locale() {
        // 1700000000 = 2023-11-14 22:13:20 UTC
        let now = 1_700_000_000;
        assert_eq!(time_label(now - 600, now, 0), "22:03");
        assert_eq!(time_label(now - 600, now, 3_600), "23:03");
        // stesso istante UTC ma a +2h è già il 15/11 → «ieri» diventa relativo
        assert_eq!(time_label(now - 3 * 3_600, now, 7_200), "ieri");
        assert_eq!(time_label(now - 86_400, now, 0), "ieri");
        assert_eq!(time_label(now - 10 * 86_400, now, 0), "04/11");
        assert_eq!(action_label(HistoryAction::Committed), "Commit");
    }

    #[test]
    fn le_azioni_agente_hanno_etichette_dedicate() {
        assert_eq!(action_label(HistoryAction::AgentWrote), "Agente: scrittura");
        assert_eq!(action_label(HistoryAction::AgentRan), "Agente: comando");
    }

    #[test]
    fn scrittura_agente_e_salvataggio_restano_voci_distinte() {
        let mut items = Vec::new();
        push(&mut items, entry(1, HistoryAction::AgentWrote, "main.rs"));
        push(&mut items, entry(2, HistoryAction::Saved, "main.rs"));
        push(&mut items, entry(3, HistoryAction::AgentWrote, "main.rs"));
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].action, HistoryAction::AgentWrote);
        assert_eq!(items[0].at, 3);
    }

    #[test]
    fn click_apre_solo_voci_con_file() {
        let mut tool = HistoryTool::new();
        let mut no_path = entry(1, HistoryAction::Committed, "«msg»");
        no_path.path = None;
        tool.update(&ToolEvent::new(Event::Replace(vec![
            entry(2, HistoryAction::Opened, "main.rs"),
            no_path,
        ])));
        assert!(matches!(
            &tool.update(&ToolEvent::new(Event::Open(0)))[..],
            [ToolRequest::OpenFile(_)]
        ));
        assert!(tool.update(&ToolEvent::new(Event::Open(1))).is_empty());
    }

    #[test]
    fn il_fuso_si_legge_una_volta_e_poi_si_riusa_per_un_minuto() {
        let cache = std::cell::Cell::new(None);
        let letture = std::cell::Cell::new(0);
        let leggi = |valore: i32| {
            letture.set(letture.get() + 1);
            valore
        };

        assert_eq!(cached_offset(&cache, 1_000, || leggi(3_600)), 3_600);
        assert_eq!(letture.get(), 1);
        // dentro il minuto: niente lettura, anche se il sistema ora direbbe altro
        assert_eq!(cached_offset(&cache, 1_000, || leggi(7_200)), 3_600);
        assert_eq!(
            cached_offset(&cache, 1_000 + OFFSET_TTL_SECONDS - 1, || leggi(7_200)),
            3_600
        );
        assert_eq!(letture.get(), 1, "nessuna lettura dentro il minuto");
        // scaduto: si rilegge
        assert_eq!(
            cached_offset(&cache, 1_000 + OFFSET_TTL_SECONDS, || leggi(7_200)),
            7_200
        );
        assert_eq!(letture.get(), 2);
    }

    /// Se l'orologio va indietro (correzione NTP, cambio manuale) la cache
    /// non resta «nel futuro» per ore: si rilegge.
    #[test]
    fn se_l_orologio_va_indietro_il_fuso_si_rilegge() {
        let cache = std::cell::Cell::new(Some((10_000, 3_600)));
        assert_eq!(cached_offset(&cache, 9_000, || 0), 0);
        assert_eq!(cache.get(), Some((9_000, 0)));
    }

    /// La `view` legge il fuso una volta per disegno passando da `cached_offset`:
    /// qui si prova la cache del tool con il lettore **vero** di sistema (l'altro
    /// test copre già i tempi con lettori finti). La `view` stessa lo chiama una
    /// volta per frame, mai per riga.
    #[test]
    fn il_fuso_vero_del_sistema_si_legge_e_resta_in_cache() {
        let tool = HistoryTool::new();
        assert!(
            tool.offset_cache.get().is_none(),
            "nessuna lettura prima del primo disegno"
        );
        let a = cached_offset(&tool.offset_cache, 500, local_offset_seconds);
        let b = cached_offset(&tool.offset_cache, 510, || i32::MAX);
        assert_eq!(
            a, b,
            "con il fuso vero del sistema, la seconda chiamata non rilegge"
        );
        assert!((-14 * 3600..=14 * 3600).contains(&a));
    }
}
