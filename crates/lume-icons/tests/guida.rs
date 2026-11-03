#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
use std::fs;

use lume_icons::limits::{
    MAX_ASSETS, MAX_ASSOCIATIONS, MAX_MANIFEST_BYTES, MAX_PACK_BYTES, MAX_SVG_BYTES,
};
use lume_icons::{FileIconContext, IconThemeProvider, ManifestIconTheme};
use serde_json::Value;

const GUIDA: &str = include_str!("../../../docs/estendere/aggiungere-un-tema-icone.md");
const APPEARANCE_RS: &str = include_str!("../../lume-app/src/appearance.rs");

fn blocco(linguaggio: &str) -> String {
    let guida = GUIDA.replace("\r\n", "\n");
    let dopo = guida
        .split(&format!("```{linguaggio}\n"))
        .nth(1)
        .unwrap_or_else(|| panic!("la guida ha un blocco ```{linguaggio}"));
    dopo.split("```").next().unwrap().trim().to_owned()
}

#[test]
fn l_esempio_della_guida_si_carica_e_associa_le_icone() {
    let dir = tempfile::tempdir().unwrap();
    let manifest = blocco("json");
    let svg = blocco("svg");
    fs::write(dir.path().join("icon-theme.json"), &manifest).unwrap();
    let valore: Value = serde_json::from_str(&manifest).unwrap();
    for definizione in valore["iconDefinitions"].as_object().unwrap().values() {
        let percorso = definizione["iconPath"].as_str().unwrap();
        let destinazione = dir.path().join(percorso);
        fs::create_dir_all(destinazione.parent().unwrap()).unwrap();
        fs::write(destinazione, &svg).unwrap();
    }

    let tema = ManifestIconTheme::load(dir.path(), "icon-theme.json").unwrap();
    let rust = tema.resolve(&FileIconContext::file("Main.rs"));
    let ignoto = tema.resolve(&FileIconContext::file("Main.xyz"));
    let cartella = tema.resolve(&FileIconContext::folder("src", false));

    assert_ne!(rust, ignoto, "l'estensione .rs deve avere un'icona sua");
    assert_ne!(rust, cartella);
    for id in [&rust, &ignoto, &cartella] {
        assert!(tema.asset(id).is_ok());
    }
}

#[test]
fn la_guida_riporta_i_limiti_veri() {
    let guida = GUIDA.replace("\r\n", "\n");
    for (nome, valore) in [
        ("MAX_MANIFEST_BYTES", MAX_MANIFEST_BYTES),
        ("MAX_SVG_BYTES", MAX_SVG_BYTES),
        ("MAX_ASSETS", MAX_ASSETS),
        ("MAX_ASSOCIATIONS", MAX_ASSOCIATIONS),
        ("MAX_PACK_BYTES", MAX_PACK_BYTES),
    ] {
        let riga = guida
            .lines()
            .find(|riga| riga.contains(&format!("`{nome}`")))
            .unwrap_or_else(|| panic!("la guida non ha la riga di `{nome}`"));
        assert!(
            riga.contains(&valore.to_string()),
            "la riga di {nome} non riporta {valore}: {riga}"
        );
    }
}

#[test]
fn la_guida_descrive_dove_l_app_cerca_i_pacchetti() {
    for voce in ["MAX_EXTERNAL_PACKS", "icon-theme.json", "external:"] {
        assert!(
            APPEARANCE_RS.contains(voce),
            "appearance.rs non ha più {voce}"
        );
    }
    for voce in [
        "icon-themes",
        "icon-theme.json",
        "external:",
        "128",
        "Rileggi pacchetti esterni",
    ] {
        assert!(GUIDA.contains(voce), "la guida non cita {voce}");
    }
}
