#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
use std::io::{Cursor, Write};

use lume_language_servers::hex::hex;
use lume_language_servers::{
    ArchiveFormat, CancelFlag, InstallError, Platform, ServerCatalog, ServerInstaller,
};
use sha2::{Digest, Sha256};
use zip::write::SimpleFileOptions;

fn zip_bytes(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let mut output = Cursor::new(Vec::new());
    {
        let mut zip = zip::ZipWriter::new(&mut output);
        for (name, bytes) in entries {
            zip.start_file(*name, SimpleFileOptions::default()).unwrap();
            zip.write_all(bytes).unwrap();
        }
        zip.finish().unwrap();
    }
    output.into_inner()
}

#[test]
fn verified_zip_installs_atomically_with_receipt() {
    let temp = tempfile::tempdir().unwrap();
    let platform = Platform::current();
    let server = ServerCatalog::embedded()
        .unwrap()
        .get("rust-analyzer")
        .unwrap();
    let mut asset = server.asset_for(&platform).unwrap().clone();
    // La fixture è ZIP anche dove il catalogo distribuisce un gzip.
    asset.archive = ArchiveFormat::Zip;
    let payload = b"real-server-binary";
    let archive = zip_bytes(&[(asset.binary_path.as_str(), payload)]);
    asset.size = archive.len() as u64;
    asset.digest = format!("sha256:{}", hex(&Sha256::digest(&archive)));
    let installer = ServerInstaller::new(temp.path().join("managed"));
    let mut progress = Vec::new();
    let receipt = installer
        .install_bytes(
            server,
            &platform,
            &asset,
            &archive,
            &CancelFlag::default(),
            |event| progress.push(event),
        )
        .unwrap();
    assert_eq!(receipt.sha256, hex(&Sha256::digest(payload)));
    assert_eq!(
        progress.last(),
        Some(&lume_language_servers::InstallProgress::Installed)
    );
    assert!(
        !temp
            .path()
            .join("managed")
            .join(format!(".install-{}-{}", server.id, std::process::id()))
            .exists()
    );
}

#[test]
fn hash_mismatch_and_pre_cancel_write_nothing() {
    let temp = tempfile::tempdir().unwrap();
    let platform = Platform::current();
    let server = ServerCatalog::embedded()
        .unwrap()
        .get("rust-analyzer")
        .unwrap();
    let mut asset = server.asset_for(&platform).unwrap().clone();
    asset.archive = ArchiveFormat::Zip;
    let archive = zip_bytes(&[(asset.binary_path.as_str(), b"x")]);
    asset.size = archive.len() as u64;
    asset.digest = format!("sha256:{}", "0".repeat(64));
    let root = temp.path().join("managed");
    let installer = ServerInstaller::new(root.clone());
    assert_eq!(
        installer.install_bytes(
            server,
            &platform,
            &asset,
            &archive,
            &CancelFlag::default(),
            |_| {}
        ),
        Err(InstallError::HashMismatch)
    );
    let cancel = CancelFlag::default();
    cancel.cancel();
    assert_eq!(
        installer.install_bytes(server, &platform, &asset, &archive, &cancel, |_| {}),
        Err(InstallError::Cancelled)
    );
    assert!(!root.exists());
}

#[test]
fn traversal_entry_is_rejected_even_when_expected_binary_exists() {
    let temp = tempfile::tempdir().unwrap();
    let platform = Platform::current();
    let server = ServerCatalog::embedded()
        .unwrap()
        .get("rust-analyzer")
        .unwrap();
    let mut asset = server.asset_for(&platform).unwrap().clone();
    asset.archive = ArchiveFormat::Zip;
    let archive = zip_bytes(&[
        ("../escape.exe", b"bad"),
        (asset.binary_path.as_str(), b"good"),
    ]);
    asset.size = archive.len() as u64;
    asset.digest = format!("sha256:{}", hex(&Sha256::digest(&archive)));
    let installer = ServerInstaller::new(temp.path().join("managed"));
    assert!(matches!(
        installer.install_bytes(
            server,
            &platform,
            &asset,
            &archive,
            &CancelFlag::default(),
            |_| {}
        ),
        Err(InstallError::UnsafeEntry(_))
    ));
    assert!(!temp.path().join("escape.exe").exists());
}
