//! Codifica esadecimale minuscola, condivisa entre el código nativo de
//! sesión y los tests de integración.

/// Restituisce i byte in esadecimale minuscola. sha2 0.11 restituisce
/// `hybrid_array::Array`, che non implementa `LowerHex`.
pub fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(out, "{byte:02x}");
    }
    out
}
