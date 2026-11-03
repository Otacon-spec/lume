//! Lettura del picco di memoria del processo corrente.
//!
//! Su Windows interroga `GetProcessMemoryInfo` (Psapi.dll) sul processo
//! corrente. Su altre piattaforme non c'è una misura: restituisce `None`.
//!
//! ATTENZIONE: **Non è il Job Object.** Il numero copre il solo processo Lume: i
//! language server, che vivono come figli dentro il Job Object, **non** sono
//! inclusi. È una misura onesta di quel che l'IDE occupa da sé, non del costo
//! totale dell'albero — e il pannello Estensioni la etichetta di conseguenza
//! («Picco RAM processo»). Per il totale servirebbe
//! `QueryInformationJobObject`, che e' un'altra cosa e va chiesta apposta.

/// Vero dove esiste una misura del picco di memoria del processo. Il pannello
/// Estensioni disegna la riga «Picco RAM processo» solo se è vero.
pub const HAS_BACKEND: bool = cfg!(target_os = "windows");

/// Restituisce il picco di memoria del processo in byte, o `None` dove non c'è
/// una misura (piattaforme senza backend) o la lettura è fallita.
///
/// Su Windows interroga `PROCESS_MEMORY_COUNTERS.PeakWorkingSetSize` tramite
/// `GetProcessMemoryInfo` (Psapi.dll, nessun unsafe a vista grazie a
/// `windows-sys`). Su piattaforme non-Windows restituisce `None`.
pub fn read_peak_memory_bytes() -> Option<u64> {
    #[cfg(target_os = "windows")]
    {
        use std::mem;

        // SAFETY: tutte le strutture e le funzioni vengono da windows-sys,
        // che garantisce i tipi corretti. Il solo invariante locale è che
        // `cb` sia inizializzato prima della chiamata — lo facciamo qui.
        unsafe {
            // Ottieni l'handle del processo corrente (pseudo-handle, sempre valido).
            let process = windows_sys::Win32::System::Threading::GetCurrentProcess();

            let mut counters: windows_sys::Win32::System::ProcessStatus::PROCESS_MEMORY_COUNTERS =
                mem::zeroed();
            counters.cb = mem::size_of::<
                windows_sys::Win32::System::ProcessStatus::PROCESS_MEMORY_COUNTERS,
            >() as u32;

            let ok = windows_sys::Win32::System::ProcessStatus::GetProcessMemoryInfo(
                process,
                &mut counters,
                counters.cb,
            );
            if ok != 0 {
                Some(counters.PeakWorkingSetSize as u64)
            } else {
                None
            }
        }
    }
    #[cfg(not(target_os = "windows"))]
    {
        None
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use super::*;

    /// Il backend dichiarato e la lettura non si contraddicono mai: se
    /// `HAS_BACKEND` è vero la lettura produce un numero, altrimenti «nessuna
    /// misura», mai uno zero che sembra una misura.
    #[test]
    fn il_backend_dichiarato_coincide_con_la_lettura() {
        assert_eq!(read_peak_memory_bytes().is_some(), HAS_BACKEND);
    }

    #[cfg(windows)]
    #[test]
    fn su_windows_il_picco_e_un_numero_vero() {
        let picco = read_peak_memory_bytes().expect("GetProcessMemoryInfo deve riuscire");
        assert!(picco > 0, "un processo in esecuzione ha un working set > 0");
    }

    #[cfg(not(windows))]
    #[test]
    fn senza_backend_non_si_inventa_un_numero() {
        assert_eq!(read_peak_memory_bytes(), None);
    }
}
