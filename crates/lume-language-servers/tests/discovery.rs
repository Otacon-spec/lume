#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use lume_language_servers::hex::hex;
use lume_language_servers::{
    AttemptFailure, DiscoveryAttempt, DiscoveryError, Platform, ServerCatalog, ServerLocator,
    describe_unavailable,
};
use lume_lsp::CommandSource;
use serde_json::json;
use sha2::{Digest, Sha256};

fn rustc_path() -> PathBuf {
    let name = if cfg!(windows) { "rustc.exe" } else { "rustc" };
    let output = std::process::Command::new("rustc")
        .args(["--print", "sysroot"])
        .output()
        .expect("test requires rustc in PATH");
    assert!(output.status.success());
    PathBuf::from(String::from_utf8(output.stdout).unwrap().trim())
        .join("bin")
        .join(name)
}

fn platform_key(platform: &Platform) -> String {
    format!("{}-{}", platform.os, platform.arch)
}

fn write_version_fixture(binary: &Path) {
    let source_path = binary.with_extension("rs");
    fs::write(
        &source_path,
        "fn main(){println!(\"rust-analyzer test fixture 1.0\");}",
    )
    .unwrap();
    let status = std::process::Command::new(rustc_path())
        .arg(&source_path)
        .arg("-o")
        .arg(binary)
        .status()
        .unwrap();
    assert!(status.success());
}

fn install_fixture(root: &Path, corrupt_receipt: bool) -> PathBuf {
    let server = ServerCatalog::embedded()
        .unwrap()
        .get("rust-analyzer")
        .unwrap();
    let platform = Platform::current();
    let directory = root
        .join(&server.id)
        .join(&server.release.version)
        .join(platform_key(&platform));
    fs::create_dir_all(&directory).unwrap();
    let binary = directory.join(if cfg!(windows) {
        "rust-analyzer.exe"
    } else {
        "rust-analyzer"
    });
    write_version_fixture(&binary);
    let digest = hex(&Sha256::digest(fs::read(&binary).unwrap()));
    let receipt = json!({
        "server_id": server.id,
        "version": server.release.version,
        "os": platform.os,
        "arch": platform.arch,
        "binary": binary.file_name().unwrap().to_string_lossy(),
        "sha256": if corrupt_receipt { "0".repeat(64) } else { digest }
    });
    fs::write(
        directory.join("receipt.json"),
        serde_json::to_vec(&receipt).unwrap(),
    )
    .unwrap();
    binary
}

fn install_probe_fixture(root: &Path, source: &str) -> PathBuf {
    let server = ServerCatalog::embedded()
        .unwrap()
        .get("rust-analyzer")
        .unwrap();
    let platform = Platform::current();
    let directory = root
        .join(&server.id)
        .join(&server.release.version)
        .join(platform_key(&platform));
    fs::create_dir_all(&directory).unwrap();
    let binary = directory.join(if cfg!(windows) {
        "rust-analyzer.exe"
    } else {
        "rust-analyzer"
    });
    let source_path = root.join("probe.rs");
    fs::write(&source_path, source).unwrap();
    let status = std::process::Command::new(rustc_path())
        .arg(&source_path)
        .arg("-o")
        .arg(&binary)
        .status()
        .unwrap();
    assert!(status.success());
    let digest = hex(&Sha256::digest(fs::read(&binary).unwrap()));
    let receipt = json!({
        "server_id": server.id,
        "version": server.release.version,
        "os": platform.os,
        "arch": platform.arch,
        "binary": binary.file_name().unwrap().to_string_lossy(),
        "sha256": digest
    });
    fs::write(
        directory.join("receipt.json"),
        serde_json::to_vec(&receipt).unwrap(),
    )
    .unwrap();
    binary
}

#[test]
fn bundle_precedes_managed_and_system_with_verified_receipts() {
    lume_testkit::richiedi!(lume_testkit::Strumento::Rustc);
    let temp = tempfile::tempdir().unwrap();
    let bundle = temp.path().join("bundle");
    let managed = temp.path().join("managed");
    let system = temp.path().join("system");
    install_fixture(&bundle, false);
    install_fixture(&managed, false);
    fs::create_dir_all(&system).unwrap();
    write_version_fixture(&system.join(if cfg!(windows) {
        "rust-analyzer.exe"
    } else {
        "rust-analyzer"
    }));
    let locator = ServerLocator::new(Some(bundle), managed, vec![system]);
    let server = ServerCatalog::embedded()
        .unwrap()
        .get("rust-analyzer")
        .unwrap();
    let resolved = locator.resolve(server, &Platform::current()).unwrap();
    assert_eq!(resolved.command.source, CommandSource::Bundled);
    assert_eq!(resolved.version, "rust-analyzer test fixture 1.0");
    assert_eq!(resolved.attempts.len(), 1);
}

#[test]
fn corrupt_bundle_is_rejected_and_real_managed_candidate_is_used() {
    lume_testkit::richiedi!(lume_testkit::Strumento::Rustc);
    let temp = tempfile::tempdir().unwrap();
    let bundle = temp.path().join("bundle");
    let managed = temp.path().join("managed");
    install_fixture(&bundle, true);
    install_fixture(&managed, false);
    let locator = ServerLocator::new(Some(bundle), managed, Vec::new());
    let server = ServerCatalog::embedded()
        .unwrap()
        .get("rust-analyzer")
        .unwrap();
    let resolved = locator.resolve(server, &Platform::current()).unwrap();
    assert_eq!(resolved.command.source, CommandSource::Managed);
    assert_eq!(resolved.attempts.len(), 2);
    assert!(
        resolved.attempts[0]
            .error
            .as_deref()
            .unwrap()
            .contains("hash mismatch")
    );
    assert_eq!(
        resolved.attempts[0].failure,
        Some(AttemptFailure::HashMismatch)
    );
    assert_eq!(
        resolved.attempts[1].failure, None,
        "il candidato riuscito non ha un motivo"
    );
}

#[test]
fn current_directory_is_never_an_implicit_system_candidate() {
    let temp = tempfile::tempdir().unwrap();
    let locator = ServerLocator::new(None, temp.path().join("managed"), vec![PathBuf::new()]);
    let server = ServerCatalog::embedded()
        .unwrap()
        .get("rust-analyzer")
        .unwrap();
    let error = locator.resolve(server, &Platform::current()).unwrap_err();
    assert!(matches!(error, DiscoveryError::Unavailable { attempts, .. } if attempts.len() == 1));
}

#[test]
fn version_probe_timeout_kills_the_candidate() {
    lume_testkit::richiedi!(lume_testkit::Strumento::Rustc);
    let temp = tempfile::tempdir().unwrap();
    let managed = temp.path().join("managed");
    install_probe_fixture(
        &managed,
        "fn main(){std::thread::sleep(std::time::Duration::from_secs(30));println!(\"late\");}",
    );
    let locator =
        ServerLocator::new(None, managed, Vec::new()).with_probe_timeout(Duration::from_millis(50));
    let server = ServerCatalog::embedded()
        .unwrap()
        .get("rust-analyzer")
        .unwrap();
    let error = locator.resolve(server, &Platform::current()).unwrap_err();
    assert!(
        matches!(&error, DiscoveryError::Unavailable { attempts, .. }
        if attempts.iter().any(|attempt| attempt.error.as_deref().is_some_and(|error| error.contains("timed out"))))
    );
    assert!(
        matches!(&error, DiscoveryError::Unavailable { attempts, .. }
        if attempts.iter().any(|attempt| attempt.failure == Some(AttemptFailure::ProbeTimeout)))
    );
}

#[test]
fn version_probe_output_is_bounded() {
    lume_testkit::richiedi!(lume_testkit::Strumento::Rustc);
    let temp = tempfile::tempdir().unwrap();
    let managed = temp.path().join("managed");
    install_probe_fixture(&managed, "fn main(){print!(\"{}\", \"x\".repeat(70_000));}");
    let locator = ServerLocator::new(None, managed, Vec::new());
    let server = ServerCatalog::embedded()
        .unwrap()
        .get("rust-analyzer")
        .unwrap();
    let error = locator.resolve(server, &Platform::current()).unwrap_err();
    assert!(
        matches!(&error, DiscoveryError::Unavailable { attempts, .. }
        if attempts.iter().any(|attempt| attempt.error.as_deref().is_some_and(|error| error.contains("64 KiB"))))
    );
    assert!(
        matches!(&error, DiscoveryError::Unavailable { attempts, .. }
        if attempts.iter().any(|attempt| attempt.failure == Some(AttemptFailure::ProbeOutputTooLarge)))
    );
}

fn rust_analyzer_spec() -> &'static lume_language_servers::ServerSpec {
    ServerCatalog::embedded()
        .unwrap()
        .get("rust-analyzer")
        .unwrap()
}

fn attempts_of(error: DiscoveryError) -> Vec<DiscoveryAttempt> {
    match error {
        DiscoveryError::Unavailable { attempts, .. } => attempts,
        altro => panic!("doveva essere Unavailable: {altro:?}"),
    }
}

#[test]
fn senza_nessun_binario_il_motivo_nomina_ogni_origine() {
    let temp = tempfile::tempdir().unwrap();
    let system = temp.path().join("bin");
    fs::create_dir_all(&system).unwrap();
    let locator = ServerLocator::new(None, temp.path().join("managed"), vec![system]);

    let error = locator
        .resolve(rust_analyzer_spec(), &Platform::current())
        .unwrap_err();
    let attempts = attempts_of(error);

    assert_eq!(attempts.len(), 2, "gestito + una cartella del PATH");
    assert!(
        attempts
            .iter()
            .all(|attempt| attempt.failure == Some(AttemptFailure::NotFound)),
        "{attempts:?}"
    );
    assert_eq!(
        describe_unavailable(&attempts),
        "Scaricato da Lume: non trovato\nPATH: non trovato"
    );
}

#[test]
fn le_cartelle_del_path_si_contano_senza_elencarle() {
    let temp = tempfile::tempdir().unwrap();
    let cartelle: Vec<PathBuf> = (0..3)
        .map(|indice| {
            let cartella = temp.path().join(format!("bin{indice}"));
            fs::create_dir_all(&cartella).unwrap();
            cartella
        })
        .collect();
    let locator = ServerLocator::new(None, temp.path().join("managed"), cartelle);

    let attempts = attempts_of(
        locator
            .resolve(rust_analyzer_spec(), &Platform::current())
            .unwrap_err(),
    );

    let testo = describe_unavailable(&attempts);
    assert_eq!(
        testo,
        "Scaricato da Lume: non trovato\nPATH: non trovato in 3 cartelle"
    );
    assert!(
        !testo.contains("bin0"),
        "nessun percorso nel testo: {testo}"
    );
}

#[test]
fn una_directory_al_posto_del_binario_non_e_un_non_trovato() {
    let temp = tempfile::tempdir().unwrap();
    let managed = temp.path().join("managed");
    let server = rust_analyzer_spec();
    let platform = Platform::current();
    let al_posto_del_binario = managed
        .join(&server.id)
        .join(&server.release.version)
        .join(platform_key(&platform))
        .join(if cfg!(windows) {
            "rust-analyzer.exe"
        } else {
            "rust-analyzer"
        });
    fs::create_dir_all(&al_posto_del_binario).unwrap();
    let locator = ServerLocator::new(None, managed, Vec::new());

    let attempts = attempts_of(locator.resolve(server, &platform).unwrap_err());

    assert_eq!(attempts.len(), 1);
    assert_eq!(attempts[0].failure, Some(AttemptFailure::NotAFile));
    assert_eq!(
        describe_unavailable(&attempts),
        "Scaricato da Lume: non è un file"
    );
}

/// Dati puri, non un finto locator: la funzione descrive una lista di
/// tentativi e non deve mai ripetere né i percorsi né il testo grezzo degli
/// errori (contengono il profilo dell'utente e, a volte, righe ostili).
#[test]
fn la_descrizione_non_ripete_percorsi_ne_testo_grezzo() {
    let segreto = r"C:\Users\mario.rossi\AppData\Local\Lume";
    let attempts = vec![
        DiscoveryAttempt {
            source: CommandSource::Bundled,
            path: PathBuf::from(segreto),
            error: Some(format!("binary hash mismatch: {segreto}\nriga ostile")),
            failure: Some(AttemptFailure::HashMismatch),
        },
        DiscoveryAttempt {
            source: CommandSource::Managed,
            path: PathBuf::from(segreto),
            error: Some("candidate not found".to_owned()),
            failure: Some(AttemptFailure::NotFound),
        },
        DiscoveryAttempt {
            source: CommandSource::SystemPath,
            path: PathBuf::from(segreto),
            error: Some("version probe timed out".to_owned()),
            failure: Some(AttemptFailure::ProbeTimeout),
        },
    ];

    let testo = describe_unavailable(&attempts);

    assert_eq!(
        testo,
        "Incluso con Lume: il file non corrisponde alla ricevuta\n\
         Scaricato da Lume: non trovato\n\
         PATH: non risponde a --version entro il tempo limite"
    );
    assert!(!testo.contains("mario"), "{testo}");
    assert!(!testo.contains("ostile"), "{testo}");
}

#[test]
fn con_un_motivo_strano_vince_su_non_trovato() {
    let attempts = vec![
        DiscoveryAttempt {
            source: CommandSource::SystemPath,
            path: PathBuf::from("/usr/bin/rust-analyzer"),
            error: Some("candidate not found".to_owned()),
            failure: Some(AttemptFailure::NotFound),
        },
        DiscoveryAttempt {
            source: CommandSource::SystemPath,
            path: PathBuf::from("/opt/bin/rust-analyzer"),
            error: Some("version probe failed".to_owned()),
            failure: Some(AttemptFailure::ProbeFailed),
        },
    ];
    assert_eq!(
        describe_unavailable(&attempts),
        "PATH: non parte (--version fallisce)"
    );
}

#[test]
fn senza_tentativi_la_descrizione_lo_dice() {
    assert_eq!(
        describe_unavailable(&[]),
        "nessun percorso di ricerca disponibile"
    );
}

/// Un binario c'è ma la ricevuta no: è il caso «file manomesso o copiato a
/// mano» che il pannello deve saper dire, non un generico «non leggibile».
/// Un file finto basta: la verifica della ricevuta precede la sonda.
#[test]
fn un_binario_senza_ricevuta_e_una_ricevuta_mancante_non_un_non_leggibile() {
    let temp = tempfile::tempdir().unwrap();
    let managed = temp.path().join("managed");
    let server = rust_analyzer_spec();
    let platform = Platform::current();
    let directory = managed
        .join(&server.id)
        .join(&server.release.version)
        .join(platform_key(&platform));
    fs::create_dir_all(&directory).unwrap();
    fs::write(
        directory.join(if cfg!(windows) {
            "rust-analyzer.exe"
        } else {
            "rust-analyzer"
        }),
        b"non un vero eseguibile",
    )
    .unwrap();
    let locator = ServerLocator::new(None, managed, Vec::new());

    let attempts = attempts_of(locator.resolve(server, &platform).unwrap_err());

    assert_eq!(attempts.len(), 1);
    assert_eq!(attempts[0].failure, Some(AttemptFailure::InvalidReceipt));
    let testo = describe_unavailable(&attempts);
    assert_eq!(
        testo,
        "Scaricato da Lume: ricevuta di installazione mancante o non valida"
    );
    assert!(!testo.contains("managed"), "nessun percorso: {testo}");
}
