//! Tipi condivisi per la fiducia esplicita dei workspace.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkspaceTrust {
    Unknown,
    Trusted,
    Denied,
}
