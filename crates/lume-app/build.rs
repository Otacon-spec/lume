//! Risorse Windows di `lume.exe`: icona e informazioni di versione (#5).
//! L'icona è la stessa della finestra: la disegna `src/app_icon.rs`, incluso
//! qui con `#[path]`, così le due non possono divergere.

#[cfg(windows)]
#[path = "src/app_icon.rs"]
mod app_icon;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=src/app_icon.rs");
    #[cfg(windows)]
    risorse_windows();
}

#[cfg(windows)]
fn risorse_windows() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let Ok(out_dir) = std::env::var("OUT_DIR") else {
        panic!("OUT_DIR assente: cargo lo imposta sempre per i build script");
    };
    let icona = std::path::Path::new(&out_dir).join("lume.ico");
    let (dati, lato, _) = app_icon::rgba();
    if let Err(errore) = std::fs::write(&icona, app_icon::ico(&dati, lato)) {
        panic!("scrittura di {}: {errore}", icona.display());
    }
    let mut risorse = winresource::WindowsResource::new();
    risorse
        .set_icon(&icona.to_string_lossy())
        .set("ProductName", "Lume")
        .set("FileDescription", "Lume")
        .set("CompanyName", "mak")
        .set(
            "LegalCopyright",
            "Apache-2.0 — https://github.com/Otacon-spec/lume",
        )
        .set("OriginalFilename", "lume.exe");
    if let Err(errore) = risorse.compile() {
        panic!("risorse Windows di lume.exe: {errore}");
    }
}
