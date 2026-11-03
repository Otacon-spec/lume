use sha2::{Digest, Sha256};

use crate::{CommandSource, ServerCommand};

pub use lume_trust::WorkspaceTrust;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ConfigurationFingerprint([u8; 32]);

impl ConfigurationFingerprint {
    pub fn from_bytes(configuration: &[u8]) -> Self {
        Self(Sha256::digest(configuration).into())
    }

    #[allow(clippy::indexing_slicing)] // nibble 0..16 su tabella di 16
    pub fn as_hex(&self) -> String {
        const HEX: &[u8; 16] = b"0123456789abcdef";
        let mut encoded = String::with_capacity(64);
        for byte in self.0 {
            encoded.push(HEX[(byte >> 4) as usize] as char);
            encoded.push(HEX[(byte & 0x0f) as usize] as char);
        }
        encoded
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("workspace trust is required ({decision:?})")]
pub struct TrustRequired {
    pub decision: WorkspaceTrust,
}

impl ServerCommand {
    pub fn validate(&self, decision: WorkspaceTrust) -> Result<(), TrustRequired> {
        if self.source != CommandSource::Workspace || decision == WorkspaceTrust::Trusted {
            return Ok(());
        }
        Err(TrustRequired { decision })
    }
}
