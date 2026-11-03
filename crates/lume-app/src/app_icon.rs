//! L'icona della finestra, quella che Windows mostra nella barra delle
//! applicazioni e nell'Alt-Tab.
//!
//! **Disegnata in codice, non caricata da un file.** La regola del progetto è
//! «Nessuna immagine: tutte le icone sono SVG inline»; un PNG incorporato
//! sarebbe l'unico asset raster del progetto, da tenere allineato ai temi a
//! mano. Qui la forma è geometria e i colori sono i token di «Officina»,
//! quindi non può sfasarsi dal resto.
//!
//! Il segno: una L azzurra illuminata da una lampada da scrivania su una tessera scura.
//! La geometria corrisponde all'icona vettoriale `assets/app/lume.svg`,
//! usata dal launcher Linux. Il rendering sovracampionato conserva bordi
//! morbidi nelle icone della finestra e nelle risorse Windows.

/// Lato dell'icona prodotta. 64 è la misura da cui Windows scala bene sia a
/// 32 (barra) sia a 16 (titolo).
const LATO: u32 = 64;
/// Sovracampionamento per i bordi morbidi.
const SCALA: u32 = 4;

/// Colori del tema «Officina».
const FONDO: [u8; 3] = [0x23, 0x26, 0x2c]; // superfici
const BORDO: [u8; 3] = [0x33, 0x37, 0x3f]; // bordo popup/input
const ACCENTO: [u8; 3] = [0x61, 0xa8, 0xf2]; // accento blu
const LUCE: [u8; 3] = [0xff, 0xe7, 0xa3]; // fascio caldo
const METALLO: [u8; 3] = [0x80, 0x96, 0xb3]; // supporto della lampada

/// L'icona in RGBA, pronta per `iced::window::icon::from_rgba`.
pub fn rgba() -> (Vec<u8>, u32, u32) {
    let grande = LATO * SCALA;
    let mut campioni = vec![[0_u8; 4]; (grande * grande) as usize];

    let lato = grande as f32;
    let raggio_tessera = 15.0 * SCALA as f32;
    let spessore_bordo = 2.0 * SCALA as f32;

    for y in 0..grande {
        for x in 0..grande {
            let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
            let indice = (y * grande + x) as usize;

            if !dentro_tessera(fx, fy, lato, raggio_tessera) {
                continue; // fuori dalla tessera: resta trasparente
            }

            // Geometria in coordinate SVG a 64px, condivisa con lume.svg.
            let (sx, sy) = (fx / SCALA as f32, fy / SCALA as f32);
            let verticale = segmento(sx, sy, (21.0, 18.0), (21.0, 46.0), 4.0);
            let orizzontale = segmento(sx, sy, (21.0, 46.0), (46.0, 46.0), 4.0);
            let mut colore = if verticale || orizzontale {
                ACCENTO
            } else if bordo_interno(fx, fy, lato, raggio_tessera, spessore_bordo) {
                FONDO
            } else {
                BORDO
            };
            // La luce colora anche la L, attenuandosi verso il fondo.
            if poligono(
                sx,
                sy,
                &[(38.0, 16.0), (46.0, 24.0), (42.0, 48.0), (17.0, 27.0)],
            ) {
                let distanza = ((sx - 42.0) * -16.0 + (sy - 20.0) * 22.0) / 740.0;
                colore = miscela(colore, LUCE, 0.28 * (1.0 - distanza.clamp(0.0, 1.0)));
            }
            if segmento(sx, sy, (48.0, 31.0), (54.0, 23.0), 1.5)
                || segmento(sx, sy, (54.0, 23.0), (48.0, 15.0), 1.5)
                || ((sx - 48.0) / 7.0).powi(2) + ((sy - 33.0) / 2.0).powi(2) <= 1.0
            {
                colore = METALLO;
            }
            if (sx - 54.0).powi(2) + (sy - 23.0).powi(2) <= 4.0 {
                colore = [0xa1, 0xb4, 0xcf];
            }
            if poligono(
                sx,
                sy,
                &[(38.0, 16.0), (44.0, 10.0), (50.0, 16.0), (46.0, 24.0)],
            ) {
                colore = miscela(
                    [0xa1, 0xb4, 0xcf],
                    [0x59, 0x70, 0x90],
                    ((sy - 10.0) / 14.0).clamp(0.0, 1.0),
                );
            }
            // Bocca ellittica del paralume, inclinata verso la lettera.
            let u = ((sx - 42.0) + (sy - 20.0)) * std::f32::consts::FRAC_1_SQRT_2;
            let v = (-(sx - 42.0) + (sy - 20.0)) * std::f32::consts::FRAC_1_SQRT_2;
            if (u / 5.7).powi(2) + (v / 1.7).powi(2) <= 1.0 {
                colore = LUCE;
            }
            if let Some(pixel) = campioni.get_mut(indice) {
                *pixel = [colore[0], colore[1], colore[2], 0xff];
            }
        }
    }

    (media(&campioni, grande), LATO, LATO)
}

/// Distanza da un segmento con estremità arrotondate.
fn segmento(x: f32, y: f32, a: (f32, f32), b: (f32, f32), raggio: f32) -> bool {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let t = (((x - a.0) * dx + (y - a.1) * dy) / (dx * dx + dy * dy)).clamp(0.0, 1.0);
    (x - a.0 - t * dx).powi(2) + (y - a.1 - t * dy).powi(2) <= raggio * raggio
}

fn poligono(x: f32, y: f32, punti: &[(f32, f32)]) -> bool {
    let mut dentro = false;
    for (a, b) in punti.iter().zip(punti.iter().cycle().skip(1)) {
        if (a.1 > y) != (b.1 > y) && x < (b.0 - a.0) * (y - a.1) / (b.1 - a.1) + a.0 {
            dentro = !dentro;
        }
    }
    dentro
}

fn miscela(fondo: [u8; 3], sopra: [u8; 3], alfa: f32) -> [u8; 3] {
    let canale = |a: u8, b: u8| (f32::from(a) * (1.0 - alfa) + f32::from(b) * alfa).round() as u8;
    let [fr, fg, fb] = fondo;
    let [sr, sg, sb] = sopra;
    [canale(fr, sr), canale(fg, sg), canale(fb, sb)]
}

/// L'icona in formato `.ico` per le risorse dell'eseguibile: una sola
/// immagine BMP a 32 bit, alfa compreso. La usa `build.rs`, che include
/// questo file con `#[path]`; nel binario è codice morto, da qui l'`allow`.
#[allow(dead_code)]
pub fn ico(rgba: &[u8], lato: u32) -> Vec<u8> {
    let lato_px = lato as usize;
    let xor = lato_px * lato_px * 4;
    // maschera AND: 1 bit per pixel, righe allineate a 4 byte, tutta a zero
    // perché la trasparenza la porta già il canale alfa
    let and = lato_px.div_ceil(32) * 4 * lato_px;
    let immagine = 40 + xor + and;
    let mut out = Vec::with_capacity(22 + immagine);
    // ICONDIR: riservato, tipo 1 (icona), una immagine
    out.extend_from_slice(&[0, 0, 1, 0, 1, 0]);
    // ICONDIRENTRY: 0 vale 256
    let byte_lato = if lato >= 256 { 0 } else { lato as u8 };
    out.extend_from_slice(&[byte_lato, byte_lato, 0, 0]);
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&32u16.to_le_bytes());
    out.extend_from_slice(&(immagine as u32).to_le_bytes());
    out.extend_from_slice(&22u32.to_le_bytes());
    // BITMAPINFOHEADER: altezza doppia, XOR più AND
    out.extend_from_slice(&40u32.to_le_bytes());
    out.extend_from_slice(&(lato as i32).to_le_bytes());
    out.extend_from_slice(&(2 * lato as i32).to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&32u16.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&((xor + and) as u32).to_le_bytes());
    out.extend_from_slice(&[0; 16]);
    // pixel dal basso verso l'alto, in BGRA
    for riga in rgba.chunks_exact(lato_px * 4).rev() {
        for pixel in riga.chunks_exact(4) {
            if let [r, g, b, a] = *pixel {
                out.extend_from_slice(&[b, g, r, a]);
            }
        }
    }
    out.resize(out.len() + and, 0);
    out
}

/// Un punto sta nella tessera arrotondata? Rettangolo pieno meno i quattro
/// angoli, ciascuno tagliato da un quarto di cerchio.
fn dentro_tessera(x: f32, y: f32, lato: f32, raggio: f32) -> bool {
    if x < 0.0 || y < 0.0 || x > lato || y > lato {
        return false;
    }
    let cx = x.max(raggio).min(lato - raggio);
    let cy = y.max(raggio).min(lato - raggio);
    let (dx, dy) = (x - cx, y - cy);
    dx * dx + dy * dy <= raggio * raggio
}

/// Vero se il punto è **oltre** l'anello di bordo, cioè nella parte piena.
fn bordo_interno(x: f32, y: f32, lato: f32, raggio: f32, spessore: f32) -> bool {
    let margine = spessore;
    x >= margine
        && y >= margine
        && x <= lato - margine
        && y <= lato - margine
        && dentro_tessera(
            x - margine,
            y - margine,
            lato - 2.0 * margine,
            raggio - margine,
        )
}

/// Media dei blocchi `SCALA`×`SCALA`: è l'antialiasing.
fn media(campioni: &[[u8; 4]], grande: u32) -> Vec<u8> {
    let mut fuori = Vec::with_capacity((LATO * LATO * 4) as usize);
    let per_blocco = SCALA * SCALA;
    for y in 0..LATO {
        for x in 0..LATO {
            let mut somma = [0_u32; 4];
            for sy in 0..SCALA {
                for sx in 0..SCALA {
                    let px = x * SCALA + sx;
                    let py = y * SCALA + sy;
                    let campione = campioni
                        .get((py * grande + px) as usize)
                        .copied()
                        .unwrap_or([0; 4]);
                    for (accumulatore, valore) in somma.iter_mut().zip(campione) {
                        *accumulatore += u32::from(valore);
                    }
                }
            }
            for canale in somma {
                // divisione esatta: `per_blocco` non è mai zero
                fuori.push((canale / per_blocco) as u8);
            }
        }
    }
    fuori
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use super::*;

    fn pixel(dati: &[u8], x: u32, y: u32) -> [u8; 4] {
        let i = ((y * LATO + x) * 4) as usize;
        [dati[i], dati[i + 1], dati[i + 2], dati[i + 3]]
    }

    /// La forma che `iced::window::icon::from_rgba` si aspetta: 4 byte per
    /// pixel, esattamente `lato × lato`. Se questo sbaglia, l'icona non si
    /// carica e la finestra resta con quella di sistema.
    #[test]
    fn le_dimensioni_sono_quelle_dichiarate() {
        let (dati, larghezza, altezza) = rgba();
        assert_eq!((larghezza, altezza), (LATO, LATO));
        assert_eq!(dati.len(), (LATO * LATO * 4) as usize);
    }

    /// Gli angoli restano trasparenti: è quel che rende la tessera
    /// arrotondata invece di un quadrato.
    #[test]
    fn gli_angoli_sono_trasparenti() {
        let (dati, ..) = rgba();
        for (x, y) in [(0, 0), (LATO - 1, 0), (0, LATO - 1), (LATO - 1, LATO - 1)] {
            assert_eq!(
                pixel(&dati, x, y)[3],
                0,
                "l'angolo ({x},{y}) deve essere trasparente"
            );
        }
    }

    #[test]
    fn la_l_e_la_lampada_sono_distinte() {
        let (dati, ..) = rgba();
        assert_eq!(
            pixel(&dati, 21, 46),
            [ACCENTO[0], ACCENTO[1], ACCENTO[2], 255]
        );
        assert_eq!(pixel(&dati, 42, 20), [LUCE[0], LUCE[1], LUCE[2], 255]);
        assert_eq!(
            pixel(&dati, 48, 33),
            [METALLO[0], METALLO[1], METALLO[2], 255]
        );
    }

    #[test]
    fn il_fascio_illumina_la_l_senza_cambiare_il_fondo_esterno() {
        let (dati, ..) = rgba();
        assert!(pixel(&dati, 21, 28)[0] > ACCENTO[0]);
        assert_eq!(pixel(&dati, 55, 45), [FONDO[0], FONDO[1], FONDO[2], 255]);
    }

    #[test]
    fn l_ico_ha_intestazione_una_voce_e_la_lunghezza_dichiarata() {
        let (dati, lato, _) = rgba();
        let ico = ico(&dati, lato);
        assert_eq!(&ico[..6], &[0, 0, 1, 0, 1, 0], "icona, una immagine");
        assert_eq!((ico[6], ico[7]), (64, 64));
        let dimensione = u32::from_le_bytes(ico[14..18].try_into().unwrap()) as usize;
        let inizio = u32::from_le_bytes(ico[18..22].try_into().unwrap()) as usize;
        assert_eq!(inizio, 22);
        assert_eq!(ico.len(), inizio + dimensione);
        let altezza = i32::from_le_bytes(ico[30..34].try_into().unwrap());
        assert_eq!(altezza, 128, "XOR più maschera AND");
    }

    #[test]
    fn l_ico_capovolge_le_righe_e_scrive_bgra() {
        // 2×2: rossa la riga in alto, blu quella in basso
        let rgba = [
            255, 0, 0, 255, 255, 0, 0, 255, //
            0, 0, 255, 255, 0, 0, 255, 255,
        ];
        let ico = ico(&rgba, 2);
        // i pixel partono a 22 + 40; il BMP va dal basso verso l'alto
        assert_eq!(
            &ico[62..66],
            &[255, 0, 0, 255],
            "prima la riga blu, in BGRA"
        );
        assert_eq!(&ico[70..74], &[0, 0, 255, 255], "poi la rossa, in BGRA");
    }
}
