//! Grafo commit a corsie del Log Git (prototipo): cella canvas per riga
//! (larghezza 44 per tre corsie, 26 di altezza) con linee colorate — verticali
//! o curve di Bézier tradotte dai path SVG del prototipo — e il pallino pieno
//! del commit sulla riga centrale (y = 13). `assign_lanes` calcola la
//! disposizione a partire da una lista di commit (dal più recente) e dai
//! rispettivi parent.

use iced::mouse;
use iced::widget::canvas::{self, Frame, Geometry, Path, Stroke};
use iced::{Color, Point, Rectangle, Renderer, Theme};

/// Altezza di una riga della lista commit.
pub const ROW_H: f32 = 26.0;
/// x della prima corsia e passo tra le corsie (8, 20, 32, …).
pub const LANE_X0: f32 = 8.0;
pub const LANE_STEP: f32 = 12.0;

/// Segmento di linea del grafo, tradotto dai path SVG del prototipo:
/// `M8 13V26` → `V { x: 8, y0: 13, y1: 26 }`;
/// `M8 13C8 22 32 17 32 26` → `C` con partenza (8,13), controlli (8,22) e
/// (32,17), fine (32,26).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PathSpec {
    /// Tratto verticale a x fissa, da y0 a y1.
    V { x: f32, y0: f32, y1: f32 },
    /// Cubica di Bézier: partenza (x0,y0), controlli (x1c,y1c) e (x2c,y2c),
    /// fine (x3,y3).
    C {
        x0: f32,
        y0: f32,
        x1c: f32,
        y1c: f32,
        x2c: f32,
        y2c: f32,
        x3: f32,
        y3: f32,
    },
}

/// Programma canvas per una cella della lista commit.
#[derive(Debug, Clone)]
pub struct GraphCell {
    pub lines: Vec<(PathSpec, Color)>,
    /// (x, colore) del pallino del commit; il centro è sempre a y = 13.
    pub dot: (f32, Color),
}

impl<M> canvas::Program<M> for GraphCell {
    type State = ();

    fn draw(
        &self,
        _state: &(),
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<Geometry> {
        let mut frame = Frame::new(renderer, bounds.size());

        for (spec, color) in &self.lines {
            let path = Path::new(|p| match *spec {
                PathSpec::V { x, y0, y1 } => {
                    p.move_to(Point::new(x, y0));
                    p.line_to(Point::new(x, y1));
                }
                PathSpec::C {
                    x0,
                    y0,
                    x1c,
                    y1c,
                    x2c,
                    y2c,
                    x3,
                    y3,
                } => {
                    p.move_to(Point::new(x0, y0));
                    p.bezier_curve_to(
                        Point::new(x1c, y1c),
                        Point::new(x2c, y2c),
                        Point::new(x3, y3),
                    );
                }
            });
            frame.stroke(&path, Stroke::default().with_width(1.5).with_color(*color));
        }

        let (x, color) = self.dot;
        frame.fill(&Path::circle(Point::new(x, 13.0), 3.5), color);

        vec![frame.into_geometry()]
    }
}

// ------------------------------------------------------------ disposizione

/// Un tratto del grafo nella riga di un commit, in termini di corsie.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Segment {
    /// Corsia che attraversa la riga senza toccare il commit.
    Through(usize),
    /// Dal commit (metà riga) verso il basso sulla stessa corsia (primo parent).
    Down(usize),
    /// Dall'alto fino al commit sulla stessa corsia (un figlio più recente).
    Up(usize),
    /// Curva dall'alto della corsia `from` fino al commit (figlio su altra corsia).
    In { from: usize, to: usize },
    /// Curva dal commit verso il basso della corsia `to` (parent su altra corsia).
    Out { from: usize, to: usize },
}

/// Riga del grafo: corsia del commit e segmenti da disegnare.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GraphRow {
    pub lane: usize,
    pub segments: Vec<Segment>,
}

/// Assegna le corsie ai commit (ordinati dal più recente) dati i loro parent.
/// Algoritmo classico: ogni corsia attiva «aspetta» un commit; il commit
/// prende la corsia che lo aspetta (o una nuova), passa la corsia al primo
/// parent e apre curve verso gli altri parent.
pub fn assign_lanes(commits: &[(String, Vec<String>)]) -> Vec<GraphRow> {
    let mut active: Vec<Option<String>> = Vec::new();
    let mut rows = Vec::with_capacity(commits.len());

    for (id, parents) in commits {
        let mut segments = Vec::new();
        // corsia del commit: la prima che lo aspetta, altrimenti la prima libera
        let lane = match active.iter().position(|slot| slot.as_deref() == Some(id)) {
            Some(lane) => lane,
            None => {
                let free = active.iter().position(Option::is_none);
                match free {
                    Some(lane) => lane,
                    None => {
                        active.push(None);
                        active.len() - 1
                    }
                }
            }
        };
        let expected_here = active
            .get(lane)
            .is_some_and(|slot| slot.as_deref() == Some(id));

        // altre corsie che aspettavano questo commit: curvano dentro e si liberano
        for (other, slot) in active.iter_mut().enumerate() {
            if other != lane && slot.as_deref() == Some(id) {
                segments.push(Segment::In {
                    from: other,
                    to: lane,
                });
                *slot = None;
            }
        }
        if expected_here {
            segments.push(Segment::Up(lane));
        }

        // il primo parent eredita la corsia; gli altri escono verso corsie proprie
        let mut parents = parents.iter();
        match parents.next() {
            Some(first) => {
                // se un'altra corsia aspetta già il primo parent, questa corsia
                // si chiude nel punto in cui le due si uniscono
                match active
                    .iter()
                    .position(|slot| slot.as_deref() == Some(first.as_str()))
                    .filter(|&other| other != lane)
                {
                    Some(other) => {
                        segments.push(Segment::Out {
                            from: lane,
                            to: other,
                        });
                        set_slot(&mut active, lane, None);
                    }
                    None => {
                        segments.push(Segment::Down(lane));
                        set_slot(&mut active, lane, Some(first.clone()));
                    }
                }
            }
            None => {
                set_slot(&mut active, lane, None);
            }
        }
        for parent in parents {
            let target = match active
                .iter()
                .position(|slot| slot.as_deref() == Some(parent.as_str()))
            {
                Some(other) => other,
                None => {
                    let free = active.iter().position(Option::is_none);
                    match free {
                        Some(other) => {
                            set_slot(&mut active, other, Some(parent.clone()));
                            other
                        }
                        None => {
                            active.push(Some(parent.clone()));
                            active.len() - 1
                        }
                    }
                }
            };
            segments.push(Segment::Out {
                from: lane,
                to: target,
            });
        }

        // corsie che attraversano la riga senza toccare il commit
        for (other, slot) in active.iter().enumerate() {
            let touched = other == lane
                || segments.iter().any(|segment| match segment {
                    Segment::In { from, .. } => *from == other,
                    Segment::Out { to, .. } => *to == other,
                    _ => false,
                });
            if !touched && slot.is_some() {
                segments.push(Segment::Through(other));
            }
        }
        // pulizia code: corsie libere in fondo
        while active.last().is_some_and(Option::is_none) {
            active.pop();
        }
        rows.push(GraphRow { lane, segments });
    }
    rows
}

fn set_slot(active: &mut [Option<String>], lane: usize, value: Option<String>) {
    if let Some(slot) = active.get_mut(lane) {
        *slot = value;
    }
}

/// x del centro di una corsia.
pub fn lane_x(lane: usize) -> f32 {
    LANE_X0 + LANE_STEP * lane as f32
}

/// Larghezza della cella per un grafo con `lanes` corsie (minimo 44 = 3 corsie).
pub fn cell_width(lanes: usize) -> f32 {
    (LANE_X0 * 2.0 + LANE_STEP * lanes.saturating_sub(1) as f32).max(44.0)
}

/// Traduce una riga in cella disegnabile; `palette[lane % len]` colora le corsie.
pub fn graph_cell(row: &GraphRow, palette: &[Color]) -> GraphCell {
    let color = |lane: usize| {
        palette
            .get(lane % palette.len().max(1))
            .copied()
            .unwrap_or(Color::WHITE)
    };
    let mid = ROW_H / 2.0;
    let mut lines = Vec::with_capacity(row.segments.len());
    for segment in &row.segments {
        let (spec, lane) = match *segment {
            Segment::Through(lane) => (
                PathSpec::V {
                    x: lane_x(lane),
                    y0: 0.0,
                    y1: ROW_H,
                },
                lane,
            ),
            Segment::Down(lane) => (
                PathSpec::V {
                    x: lane_x(lane),
                    y0: mid,
                    y1: ROW_H,
                },
                lane,
            ),
            Segment::Up(lane) => (
                PathSpec::V {
                    x: lane_x(lane),
                    y0: 0.0,
                    y1: mid,
                },
                lane,
            ),
            // prototipo: M20 0C20 9 8 4 8 13
            Segment::In { from, to } => (
                PathSpec::C {
                    x0: lane_x(from),
                    y0: 0.0,
                    x1c: lane_x(from),
                    y1c: 9.0,
                    x2c: lane_x(to),
                    y2c: 4.0,
                    x3: lane_x(to),
                    y3: mid,
                },
                from,
            ),
            // prototipo: M8 13C8 22 32 17 32 26
            Segment::Out { from, to } => (
                PathSpec::C {
                    x0: lane_x(from),
                    y0: mid,
                    x1c: lane_x(from),
                    y1c: 22.0,
                    x2c: lane_x(to),
                    y2c: 17.0,
                    x3: lane_x(to),
                    y3: ROW_H,
                },
                to,
            ),
        };
        lines.push((spec, color(lane)));
    }
    GraphCell {
        lines,
        dot: (lane_x(row.lane), color(row.lane)),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use super::*;

    fn c(id: &str, parents: &[&str]) -> (String, Vec<String>) {
        (id.into(), parents.iter().map(|p| p.to_string()).collect())
    }

    #[test]
    fn storia_lineare_usa_una_sola_corsia() {
        let rows = assign_lanes(&[c("c", &["b"]), c("b", &["a"]), c("a", &[])]);
        assert!(rows.iter().all(|row| row.lane == 0));
        assert_eq!(rows[0].segments, vec![Segment::Down(0)]);
        assert_eq!(rows[1].segments, vec![Segment::Up(0), Segment::Down(0)]);
        assert_eq!(rows[2].segments, vec![Segment::Up(0)]);
    }

    #[test]
    fn merge_apre_una_seconda_corsia_che_si_richiude_sulla_radice() {
        // m = merge(a, b); a e b figli di r (forma del prototipo: main + feat)
        let rows = assign_lanes(&[
            c("m", &["a", "b"]),
            c("a", &["r"]),
            c("b", &["r"]),
            c("r", &[]),
        ]);
        assert_eq!(rows[0].lane, 0);
        assert!(rows[0].segments.contains(&Segment::Out { from: 0, to: 1 }));
        assert_eq!(rows[1].lane, 0);
        assert!(rows[1].segments.contains(&Segment::Through(1)));
        assert_eq!(rows[2].lane, 1, "b vive sulla seconda corsia");
        assert!(
            rows[2].segments.contains(&Segment::Out { from: 1, to: 0 }),
            "b si richiude sulla corsia di r"
        );
        assert_eq!(rows[3].lane, 0);
        assert_eq!(rows[3].segments, vec![Segment::Up(0)]);
    }

    #[test]
    fn cella_traduce_i_segmenti_nei_path_del_prototipo() {
        let palette = [Color::WHITE, Color::BLACK];
        let row = GraphRow {
            lane: 0,
            segments: vec![Segment::Out { from: 0, to: 2 }],
        };
        let cell = graph_cell(&row, &palette);
        assert_eq!(cell.dot, (8.0, Color::WHITE));
        assert_eq!(
            cell.lines[0].0,
            PathSpec::C {
                x0: 8.0,
                y0: 13.0,
                x1c: 8.0,
                y1c: 22.0,
                x2c: 32.0,
                y2c: 17.0,
                x3: 32.0,
                y3: 26.0
            }
        );
        assert_eq!(cell_width(1), 44.0);
        assert_eq!(cell_width(5), 64.0);
    }
}
