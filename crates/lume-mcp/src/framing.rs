//! NDJSON con tetto: una riga per messaggio, come lo stdio MCP.
//!
//! Non si riusa `lume_lsp::FrameDecoder`, che è Content-Length. Il tetto è
//! `lume_lsp::MAX_BODY_BYTES`: oltre, la connessione si chiude, perché una
//! riga senza fine sarebbe memoria senza limite.
//!
//! `next_line` è cancel-safe: la parte già letta resta in `self.pending`, non
//! in una variabile locale. Il server la usa in `tokio::select!`; perdere una
//! mezza riga lì corromperebbe il messaggio successivo.

use tokio::io::{AsyncBufReadExt, AsyncRead, BufReader};

pub const LINE_CAP: usize = lume_lsp::MAX_BODY_BYTES;

#[derive(Debug, thiserror::Error)]
pub enum FrameError {
    #[error("riga oltre il tetto di {max} byte")]
    TooLong { max: usize },
    #[error("lettura fallita: {0}")]
    Io(#[from] std::io::Error),
}

pub struct LineReader<R> {
    inner: BufReader<R>,
    pending: Vec<u8>,
    max: usize,
}

impl<R: AsyncRead + Unpin> LineReader<R> {
    pub fn new(inner: R, max: usize) -> Self {
        Self {
            inner: BufReader::new(inner),
            pending: Vec::new(),
            max,
        }
    }

    /// Il preambolo ha un tetto piccolo, i messaggi quello di framing.
    pub fn set_max(&mut self, max: usize) {
        self.max = max;
    }

    pub async fn next_line(&mut self) -> Result<Option<Vec<u8>>, FrameError> {
        loop {
            let available = self.inner.fill_buf().await?;
            if available.is_empty() {
                if self.pending.is_empty() {
                    return Ok(None);
                }
                return Ok(Some(finish(std::mem::take(&mut self.pending))));
            }
            let (chunk_len, done) = match available.iter().position(|&byte| byte == b'\n') {
                Some(position) => (position, true),
                None => (available.len(), false),
            };
            if self.pending.len() + chunk_len > self.max {
                return Err(FrameError::TooLong { max: self.max });
            }
            if let Some(chunk) = available.get(..chunk_len) {
                self.pending.extend_from_slice(chunk);
            }
            self.inner
                .consume(if done { chunk_len + 1 } else { chunk_len });
            if done {
                return Ok(Some(finish(std::mem::take(&mut self.pending))));
            }
        }
    }
}

fn finish(mut line: Vec<u8>) -> Vec<u8> {
    if line.last() == Some(&b'\r') {
        line.pop();
    }
    line
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use super::*;
    use tokio::io::AsyncWriteExt;

    #[tokio::test]
    async fn due_righe_in_un_pezzo_e_il_crlf_tolto() {
        let dati: &[u8] = b"{\"a\":1}\r\n{\"b\":2}\n";
        let mut lettore = LineReader::new(dati, 64);
        assert_eq!(lettore.next_line().await.unwrap().unwrap(), b"{\"a\":1}");
        assert_eq!(lettore.next_line().await.unwrap().unwrap(), b"{\"b\":2}");
        assert!(lettore.next_line().await.unwrap().is_none());
    }

    /// `duplex(4)` consegna al massimo quattro byte per volta: la riga arriva
    /// a pezzi, come da una pipe vera.
    #[tokio::test]
    async fn una_riga_spezzata_in_tanti_pezzi_arriva_intera() {
        let (mut scrittore, lettore) = tokio::io::duplex(4);
        let scrivi = async move {
            scrittore.write_all(b"abcdefghij\nxy").await.unwrap();
        };
        let leggi = async {
            let mut lettore = LineReader::new(lettore, 64);
            (
                lettore.next_line().await.unwrap(),
                lettore.next_line().await.unwrap(),
                lettore.next_line().await.unwrap(),
            )
        };
        let ((), (prima, seconda, fine)) = tokio::join!(scrivi, leggi);
        assert_eq!(prima.unwrap(), b"abcdefghij");
        assert_eq!(
            seconda.unwrap(),
            b"xy",
            "l'ultima riga senza a capo vale lo stesso"
        );
        assert!(fine.is_none());
    }

    #[tokio::test]
    async fn oltre_il_tetto_e_un_errore_non_una_riga_troncata() {
        let dati: &[u8] = b"0123456789\n";
        let mut lettore = LineReader::new(dati, 5);
        assert!(matches!(
            lettore.next_line().await,
            Err(FrameError::TooLong { max: 5 })
        ));
    }

    #[test]
    fn il_tetto_e_quello_di_lume_lsp() {
        assert_eq!(LINE_CAP, 16 * 1024 * 1024);
    }
}
