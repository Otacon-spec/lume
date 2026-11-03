#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
use std::collections::BTreeSet;

use lume_theme::tokens::LumeTheme;

const GUIDA: &str = include_str!("../../../docs/estendere/aggiungere-un-tema-colori.md");
const OFFICINA: &str = include_str!("../assets/themes/officina.toml");
const TOKENS_RS: &str = include_str!("../src/tokens.rs");

fn primo_blocco_toml() -> String {
    let guida = GUIDA.replace("\r\n", "\n");
    let dopo = guida
        .split("```toml")
        .nth(1)
        .expect("la guida ha un blocco ```toml");
    dopo.split("```").next().unwrap().to_owned()
}

fn chiavi(testo: &str) -> BTreeSet<String> {
    toml::from_str::<toml::Table>(testo)
        .unwrap()
        .keys()
        .cloned()
        .collect()
}

#[test]
fn l_esempio_della_guida_e_un_tema_valido_e_completo() {
    let esempio = primo_blocco_toml();
    let tema: LumeTheme = toml::from_str(&esempio).unwrap();
    assert!(
        LumeTheme::builtin().iter().all(|gia| gia.id != tema.id),
        "l'esempio deve avere un id che non esiste già fra i temi embedded"
    );
    assert_eq!(
        chiavi(&esempio),
        chiavi(OFFICINA),
        "l'esempio deve avere esattamente le chiavi dei temi embedded"
    );
}

#[test]
fn la_guida_spiega_ogni_chiave_e_ogni_tema_embedded() {
    for chiave in chiavi(OFFICINA) {
        assert!(
            GUIDA.contains(&format!("`{chiave}`")),
            "la guida non spiega la chiave `{chiave}`"
        );
    }
    for tema in LumeTheme::builtin() {
        assert!(
            GUIDA.contains(&format!("`{}`", tema.id)),
            "la guida non elenca il tema `{}`",
            tema.id
        );
    }
}

#[test]
fn la_guida_cita_i_punti_del_codice_da_toccare() {
    for simbolo in ["EMBEDDED", "i_cinque_temi_embedded_si_caricano"] {
        assert!(GUIDA.contains(simbolo), "la guida non cita {simbolo}");
        assert!(
            TOKENS_RS.contains(simbolo),
            "tokens.rs non ha più {simbolo}"
        );
    }
    assert!(
        GUIDA.contains("in fondo"),
        "la guida deve dire che il tema nuovo va in fondo a EMBEDDED"
    );
}
