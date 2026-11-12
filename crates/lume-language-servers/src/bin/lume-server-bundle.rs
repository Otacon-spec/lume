use std::error::Error;
use std::ffi::OsString;
use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};

use lume_language_servers::hex::hex;
use lume_language_servers::{
    CancelFlag, InstallProgress, Platform, ServerCatalog, ServerInstaller,
};
use serde::Serialize;
use sha2::{Digest, Sha256};
use zip::write::SimpleFileOptions;

#[derive(Serialize)]
struct BundleManifest<'a> {
    schema: u32,
    product: &'static str,
    os: &'a str,
    arch: &'a str,
    servers: Vec<BundleServer<'a>>,
}

#[derive(Serialize)]
struct BundleServer<'a> {
    id: &'a str,
    version: &'a str,
    repository: &'a str,
    release_asset_sha256: &'a str,
}

/// Radice del workspace, per i testi di licenza incisi nel bundler a compile
/// time: così uno zip non può uscire senza, qualunque sia la cartella da cui
/// si lancia il bundler.
macro_rules! workspace_file {
    ($path:literal) => {
        include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../", $path))
    };
}

/// Licenza di Lume, il suo NOTICE (Apache-2.0 §4(d): deve viaggiare con
/// ogni copia) e gli avvisi di terze parti, nella radice dello zip.
const ROOT_LICENSES: &[(&str, &str)] = &[
    ("LICENSE.txt", workspace_file!("LICENSE")),
    ("NOTICE.txt", workspace_file!("NOTICE")),
    (
        "THIRD_PARTY_NOTICES.md",
        workspace_file!("THIRD_PARTY_NOTICES.md"),
    ),
    (
        "THIRD_PARTY_LICENSES.md",
        workspace_file!("THIRD_PARTY_LICENSES.md"),
    ),
];

/// Testi di licenza di quel che è compilato dentro `lume.exe` (font, icone,
/// il fork di tiberius) e dei language server distribuiti in `servers\`.
/// MIT, Apache e OFL chiedono tutte che il testo viaggi con la copia.
const THIRD_PARTY_LICENSE_FILES: &[(&str, &str)] = &[
    (
        "Inter-OFL.txt",
        workspace_file!("assets/fonts/LICENSE-Inter.txt"),
    ),
    (
        "JetBrainsMono-OFL.txt",
        workspace_file!("assets/fonts/LICENSE-JetBrainsMono.txt"),
    ),
    (
        "material-icon-theme-MIT.txt",
        workspace_file!("crates/lume-icons/assets/material/LICENSE"),
    ),
    (
        "catppuccin-vscode-icons-MIT.txt",
        workspace_file!("crates/lume-icons/assets/catppuccin-mocha/LICENSE"),
    ),
    (
        "tiberius-ng-LICENSE-MIT.txt",
        workspace_file!("vendor/tiberius-ng/LICENSE-MIT.txt"),
    ),
    (
        "tiberius-ng-LICENSE-APACHE.txt",
        workspace_file!("vendor/tiberius-ng/LICENSE-APACHE.txt"),
    ),
    (
        "aws-lc-sys-LICENSE.txt",
        include_str!("../../assets/licenses/aws-lc-sys-LICENSE.txt"),
    ),
    (
        "rust-analyzer-LICENSE-MIT.txt",
        include_str!("../../assets/licenses/rust-analyzer-LICENSE-MIT.txt"),
    ),
    (
        "rust-analyzer-LICENSE-APACHE.txt",
        include_str!("../../assets/licenses/rust-analyzer-LICENSE-APACHE.txt"),
    ),
    (
        "tombi-LICENSE-MIT.txt",
        include_str!("../../assets/licenses/tombi-LICENSE.txt"),
    ),
    (
        "rumdl-LICENSE-MIT.txt",
        include_str!("../../assets/licenses/rumdl-LICENSE.txt"),
    ),
];

const THIRD_PARTY_SERVERS: &str = "\
Language server distribuiti in servers\\, scaricati dalle release ufficiali
con SHA-256 verificato (vedi server-bundle-lock.json):

- rust-analyzer  https://github.com/rust-lang/rust-analyzer
  MIT OR Apache-2.0: licenses\\rust-analyzer-LICENSE-MIT.txt,
  licenses\\rust-analyzer-LICENSE-APACHE.txt
- Tombi          https://github.com/tombi-toml/tombi
  MIT: licenses\\tombi-LICENSE-MIT.txt
- rumdl          https://github.com/rvben/rumdl
  MIT: licenses\\rumdl-LICENSE-MIT.txt

Il sorgente di ciascuno è nel rispettivo repository, al tag indicato nel lock.
";

fn write_licenses(root: &Path) -> Result<(), Box<dyn Error>> {
    for (name, text) in ROOT_LICENSES {
        fs::write(root.join(name), text)?;
    }
    let licenses = root.join("licenses");
    fs::create_dir_all(&licenses)?;
    for (name, text) in THIRD_PARTY_LICENSE_FILES {
        fs::write(licenses.join(name), text)?;
    }
    fs::write(root.join("THIRD-PARTY-SERVERS.txt"), THIRD_PARTY_SERVERS)?;
    Ok(())
}

fn main() {
    if let Err(error) = run() {
        eprintln!("bundle error: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn Error>> {
    let (lume_exe, output) = parse_args()?;
    if !lume_exe.is_file() {
        return Err(format!("Lume executable does not exist: {}", lume_exe.display()).into());
    }
    if output.exists() {
        return Err(format!(
            "refusing to overwrite existing bundle: {}",
            output.display()
        )
        .into());
    }
    let output_parent = output
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(output_parent)?;
    let staging = output_parent.join(format!(".lume-bundle-staging-{}", std::process::id()));
    if staging.exists() {
        return Err(format!("staging directory already exists: {}", staging.display()).into());
    }
    fs::create_dir(&staging)?;

    let result = build_bundle(&lume_exe, &output, &staging);
    let cleanup = fs::remove_dir_all(&staging);
    if let Err(error) = result {
        let _ = cleanup;
        return Err(error);
    }
    cleanup?;
    Ok(())
}

fn build_bundle(lume_exe: &Path, output: &Path, staging: &Path) -> Result<(), Box<dyn Error>> {
    let root = staging.join("Lume");
    let server_root = root.join("servers");
    fs::create_dir_all(&server_root)?;
    fs::copy(lume_exe, root.join("lume.exe"))?;

    let catalog = ServerCatalog::embedded()?;
    let platform = Platform {
        os: "windows".to_owned(),
        arch: "x86_64".to_owned(),
    };
    let installer = ServerInstaller::new(server_root);
    let cancel = CancelFlag::default();
    let mut manifest_servers = Vec::new();

    for server in catalog.servers() {
        let asset = server.asset_for(&platform)?;
        eprintln!("download {} {}", server.name, server.release.version);
        let mut last_percent = 101;
        installer.install(server, &platform, &cancel, |event| match event {
            InstallProgress::Downloading { received, total } => {
                let percent = received.saturating_mul(100) / total.max(1);
                if percent != last_percent && (percent % 10 == 0 || percent == 100) {
                    eprintln!("  {percent}%");
                    last_percent = percent;
                }
            }
            InstallProgress::Verifying => eprintln!("  verify SHA-256"),
            InstallProgress::Extracting => eprintln!("  extract"),
            InstallProgress::Installed => eprintln!("  installed"),
        })?;
        manifest_servers.push(BundleServer {
            id: &server.id,
            version: &server.release.version,
            repository: &server.release.repository,
            release_asset_sha256: &asset.digest,
        });
    }

    let manifest = BundleManifest {
        schema: 1,
        product: "Lume Bundle",
        os: &platform.os,
        arch: &platform.arch,
        servers: manifest_servers,
    };
    fs::write(
        root.join("server-bundle-lock.json"),
        serde_json::to_vec_pretty(&manifest)?,
    )?;
    write_licenses(&root)?;

    let mut files = Vec::new();
    collect_files(staging, staging, &mut files)?;
    files.sort_by(|left, right| left.0.cmp(&right.0));
    let temporary_archive = output.with_extension("zip.partial");
    let result = write_zip(&temporary_archive, files);
    if let Err(error) = result {
        let _ = fs::remove_file(&temporary_archive);
        return Err(error);
    }
    fs::rename(&temporary_archive, output)?;

    let digest = sha256_file(output)?;
    let mut checksum_name: OsString = output.as_os_str().to_owned();
    checksum_name.push(".sha256");
    let checksum_path = PathBuf::from(checksum_name);
    let filename = output
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or("bundle output must have a UTF-8 filename")?;
    fs::write(&checksum_path, format!("{digest}  {filename}\n"))?;
    eprintln!("bundle {}", output.display());
    eprintln!("sha256 {digest}");
    Ok(())
}

fn parse_args() -> Result<(PathBuf, PathBuf), Box<dyn Error>> {
    let mut args = std::env::args_os().skip(1);
    let mut lume_exe = None;
    let mut output = None;
    while let Some(argument) = args.next() {
        match argument.to_str() {
            Some("--lume-exe") => lume_exe = args.next().map(PathBuf::from),
            Some("--output") => output = args.next().map(PathBuf::from),
            _ => return Err(format!("unknown or invalid argument: {argument:?}").into()),
        }
    }
    Ok((
        lume_exe.ok_or("missing --lume-exe")?,
        output.ok_or("missing --output")?,
    ))
}

fn collect_files(
    base: &Path,
    directory: &Path,
    output: &mut Vec<(String, PathBuf)>,
) -> Result<(), Box<dyn Error>> {
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            collect_files(base, &path, output)?;
        } else if path.is_file() {
            let relative = path.strip_prefix(base)?;
            let zip_path = relative.to_string_lossy().replace('\\', "/");
            output.push((zip_path, path));
        }
    }
    Ok(())
}

fn write_zip(output: &Path, files: Vec<(String, PathBuf)>) -> Result<(), Box<dyn Error>> {
    let file = File::create(output)?;
    let mut archive = zip::ZipWriter::new(file);
    let options = SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated)
        .unix_permissions(0o644);
    for (name, path) in files {
        archive.start_file(name, options)?;
        let mut input = File::open(path)?;
        std::io::copy(&mut input, &mut archive)?;
    }
    archive.finish()?.sync_all()?;
    Ok(())
}

#[allow(clippy::indexing_slicing)] // count <= buffer.len() (contratto di Read)
fn sha256_file(path: &Path) -> Result<String, Box<dyn Error>> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
    }
    Ok(hex(&hasher.finalize()))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use super::*;

    /// Ogni file citato in THIRD-PARTY-SERVERS.txt deve esistere nello zip,
    /// e ogni testo deve essere una licenza vera, non un file vuoto.
    #[test]
    fn lo_zip_porta_con_se_tutti_i_testi_di_licenza() {
        let root = std::env::temp_dir().join(format!("lume-bundle-lic-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        write_licenses(&root).unwrap();

        for (name, _) in ROOT_LICENSES {
            assert!(root.join(name).is_file(), "{name}");
        }
        for (name, text) in THIRD_PARTY_LICENSE_FILES {
            assert!(root.join("licenses").join(name).is_file(), "{name}");
            assert!(text.len() > 200, "{name} sembra vuoto");
        }
        for citato in THIRD_PARTY_SERVERS
            .split(|c: char| c.is_whitespace() || c == ',')
            .filter_map(|parola| parola.strip_prefix("licenses\\"))
        {
            assert!(root.join("licenses").join(citato).is_file(), "{citato}");
        }
        let _ = fs::remove_dir_all(&root);
    }

    /// Ogni server del catalogo porta nello zip almeno un testo di licenza e
    /// THIRD-PARTY-SERVERS.txt lo cita con il suo repository. Un server
    /// aggiunto al catalogo senza la sua licenza fa fallire questo test, non
    /// la distribuzione.
    #[test]
    fn ogni_server_del_catalogo_ha_i_testi_di_licenza_nel_bundle() {
        let catalogo = ServerCatalog::embedded().unwrap();
        for server in catalogo.servers() {
            let prefisso = format!("{}-LICENSE-", server.id);
            let file: Vec<&(&str, &str)> = THIRD_PARTY_LICENSE_FILES
                .iter()
                .filter(|(nome, _)| nome.starts_with(&prefisso))
                .collect();
            assert!(
                !file.is_empty(),
                "{}: nessun file `{prefisso}<tipo>.txt` in THIRD_PARTY_LICENSE_FILES",
                server.id
            );
            let repository = format!("https://github.com/{}", server.release.repository);
            assert!(
                THIRD_PARTY_SERVERS.contains(&repository),
                "THIRD_PARTY_SERVERS non cita {repository}"
            );
            for (nome, testo) in file {
                let citato = format!("licenses\\{nome}");
                assert!(
                    THIRD_PARTY_SERVERS.contains(&citato),
                    "THIRD_PARTY_SERVERS non cita {citato}"
                );
                if nome.ends_with("-MIT.txt") {
                    assert!(
                        testo.contains("Permission is hereby granted"),
                        "{nome} non sembra un testo MIT"
                    );
                }
                if nome.ends_with("-APACHE.txt") {
                    assert!(
                        testo.contains("Apache License"),
                        "{nome} non sembra un testo Apache"
                    );
                }
            }
        }
    }

    /// Un testo di licenza col titolare lasciato come segnaposto non
    /// attribuisce niente a nessuno. È il file che viaggia nello zip.
    #[test]
    fn le_licenze_di_terze_parti_hanno_i_titolari_veri() {
        let (_, testo) = ROOT_LICENSES
            .iter()
            .find(|(nome, _)| *nome == "THIRD_PARTY_LICENSES.md")
            .expect("THIRD_PARTY_LICENSES.md nello zip");
        assert!(
            !testo.contains("<year> <owner>"),
            "«<year> <owner>» in THIRD_PARTY_LICENSES.md"
        );
        // Il MIT generico resta per i crate senza una fonte licenza verificabile
        // nella configurazione. I casi risolti hanno invece testo e titolare reali.
        assert!(
            testo.matches("<copyright holders>").count() <= 1,
            "più di un «<copyright holders>» in THIRD_PARTY_LICENSES.md"
        );
        for titolare in ["Derek Stride", "zune-image developers"] {
            assert!(testo.contains(titolare), "{titolare}");
        }
        // `windows-collections` era solo nella catena iced → mundy → zbus 5:
        // uscita quella, l'attribuzione si verifica su un crate windows-rs
        // che resta nel grafo (`windows-core`, dipendenza di `windows`).
        let windows_mit = testo
            .split("## MIT License")
            .find(|sezione| sezione.contains("[windows-core 0.62.2]"))
            .expect("windows-core nella sezione MIT");
        assert!(
            windows_mit.contains("Copyright (c) Microsoft Corporation."),
            "windows-rs deve usare il MIT Microsoft incluso nel crate"
        );
        let objc_mit = testo
            .split("## MIT License")
            .find(|sezione| sezione.contains("[objc-sys 0.3.5]"))
            .expect("objc-sys nella sezione MIT");
        assert!(
            objc_mit.contains("Copyright (c) Steven Sheldon"),
            "objc-sys deve usare il MIT del commit pubblicato"
        );
    }

    /// Il LICENSE composito di aws-lc-sys nomina sei titolari, e Apache,
    /// ISC e BSD chiedono che il testo viaggi con la copia.
    #[test]
    fn il_license_composito_di_aws_lc_sys_viaggia_nello_zip() {
        let (_, testo) = THIRD_PARTY_LICENSE_FILES
            .iter()
            .find(|(nome, _)| *nome == "aws-lc-sys-LICENSE.txt")
            .expect("aws-lc-sys-LICENSE.txt fra i testi dello zip");
        for titolare in [
            "Amazon.com",
            "Google Inc.",
            "The OpenSSL Project",
            "Eric Young",
            "fiat-crypto",
            "The Go Authors",
        ] {
            assert!(testo.contains(titolare), "{titolare}");
        }
    }
}
