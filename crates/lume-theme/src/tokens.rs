//! I 31 token per tema, stessa struttura del metodo `themes()` del prototipo.

use iced::Color;
use serde::Deserialize;

/// Token di un tema Lume. Nomi identici alle CSS custom properties del
/// prototipo (senza `--`); `fn`/`str` sono rinominati perché keyword Rust.
#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
pub struct LumeTheme {
    pub id: String,
    pub name: String,

    #[serde(with = "hex")]
    pub deep: Color,
    #[serde(with = "hex")]
    pub bg: Color,
    #[serde(with = "hex")]
    pub surface: Color,
    #[serde(with = "hex")]
    pub inset: Color,
    #[serde(with = "hex")]
    pub border: Color,
    #[serde(with = "hex")]
    pub border2: Color,
    #[serde(with = "hex")]
    pub text: Color,
    #[serde(with = "hex")]
    pub ui: Color,
    #[serde(with = "hex")]
    pub sec: Color,
    #[serde(with = "hex")]
    pub muted: Color,
    #[serde(with = "hex")]
    pub icon: Color,
    #[serde(with = "hex")]
    pub faint: Color,
    #[serde(with = "hex")]
    pub tree: Color,
    #[serde(with = "hex")]
    pub hover: Color,
    #[serde(with = "hex")]
    pub hover2: Color,
    #[serde(with = "hex")]
    pub accent: Color,
    #[serde(rename = "accent-on", with = "hex")]
    pub accent_on: Color,
    #[serde(rename = "accent-dim", with = "hex")]
    pub accent_dim: Color,
    #[serde(with = "hex")]
    pub sel: Color,
    #[serde(with = "hex")]
    pub good: Color,
    #[serde(rename = "good-bg", with = "hex")]
    pub good_bg: Color,
    #[serde(rename = "good-br", with = "hex")]
    pub good_br: Color,
    #[serde(with = "hex")]
    pub warn: Color,
    #[serde(rename = "warn-bg", with = "hex")]
    pub warn_bg: Color,
    #[serde(rename = "warn-br", with = "hex")]
    pub warn_br: Color,
    #[serde(with = "hex")]
    pub bad: Color,
    #[serde(rename = "fn", with = "hex")]
    pub func: Color,
    #[serde(rename = "str", with = "hex")]
    pub string: Color,
    #[serde(with = "hex")]
    pub num: Color,
    #[serde(with = "hex")]
    pub comment: Color,
    #[serde(with = "hex")]
    pub hl: Color,
}

const EMBEDDED: [&str; 5] = [
    include_str!("../assets/themes/officina.toml"),
    include_str!("../assets/themes/grafite.toml"),
    include_str!("../assets/themes/ossido.toml"),
    include_str!("../assets/themes/inchiostro.toml"),
    include_str!("../assets/themes/terracotta.toml"),
];

impl LumeTheme {
    /// I 5 temi embedded, in ordine di presentazione. Officina per primo
    /// (default). Un tema non parsabile viene saltato (con log su stderr)
    /// invece di far crashare l'avvio.
    pub fn builtin() -> Vec<LumeTheme> {
        EMBEDDED
            .iter()
            .filter_map(|src| match toml::from_str(src) {
                Ok(t) => Some(t),
                Err(e) => {
                    eprintln!("lume-theme: tema embedded non valido: {e}");
                    None
                }
            })
            .collect()
    }

    /// Tema iced per i widget builtin (input, scrollable, checkbox…);
    /// tutto il chrome usa i token direttamente via style-closure.
    pub fn iced_theme(&self) -> iced::Theme {
        iced::Theme::custom(
            self.name.clone(),
            iced::theme::Palette {
                background: self.bg,
                text: self.ui,
                primary: self.accent,
                success: self.good,
                warning: self.warn,
                danger: self.bad,
            },
        )
    }
}

mod hex {
    use iced::Color;
    use serde::{Deserialize, Deserializer, de::Error};

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Color, D::Error> {
        let s = String::deserialize(d)?;
        parse(&s).ok_or_else(|| D::Error::custom(format!("colore hex non valido: {s}")))
    }

    pub fn parse(s: &str) -> Option<Color> {
        let s = s.strip_prefix('#')?;
        let b = |i: usize| u8::from_str_radix(s.get(i..i + 2)?, 16).ok();
        match s.len() {
            6 => Some(Color::from_rgb8(b(0)?, b(2)?, b(4)?)),
            8 => Some(Color::from_rgba8(b(0)?, b(2)?, b(4)?, b(6)? as f32 / 255.0)),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use super::*;

    #[test]
    fn i_cinque_temi_embedded_si_caricano() {
        let themes = LumeTheme::builtin();
        let ids: Vec<&str> = themes.iter().map(|t| t.id.as_str()).collect();
        assert_eq!(
            ids,
            ["officina", "grafite", "ossido", "inchiostro", "terracotta"]
        );
    }

    #[test]
    fn officina_ha_i_valori_del_prototipo() {
        let officina = &LumeTheme::builtin()[0];
        assert_eq!(officina.accent, hex::parse("#61a8f2").unwrap());
        assert_eq!(officina.bg, hex::parse("#1e2126").unwrap());
        assert_eq!(officina.sel, hex::parse("#31445c").unwrap());
        assert_eq!(officina.func, hex::parse("#79c0ff").unwrap());
        assert_eq!(officina.hl, hex::parse("#4a3d20").unwrap());
    }

    #[test]
    fn hex_parse_gestisce_alpha_e_input_rotti() {
        assert!(hex::parse("#12345").is_none());
        assert!(hex::parse("zzzzzz").is_none());
        let c = hex::parse("#ff000080").unwrap();
        assert!((c.a - 0.5).abs() < 0.01);
    }
}
