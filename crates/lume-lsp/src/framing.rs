use crate::rpc::RpcMessage;

pub const MAX_HEADER_BYTES: usize = 8 * 1024;
pub const MAX_BODY_BYTES: usize = 16 * 1024 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum ProtocolError {
    #[error("header JSON-RPC oltre il limite: {size} > {limit}")]
    HeaderTooLarge { size: usize, limit: usize },
    #[error("body JSON-RPC oltre il limite: {size} > {limit}")]
    BodyTooLarge { size: usize, limit: usize },
    #[error("header JSON-RPC non valido")]
    InvalidHeader,
    #[error("Content-Length mancante")]
    MissingContentLength,
    #[error("Content-Length duplicato")]
    DuplicateContentLength,
    #[error("Content-Length non valido")]
    InvalidContentLength,
    #[error("JSON-RPC non valido: {0}")]
    Json(#[from] serde_json::Error),
}

#[derive(Debug, Default)]
pub struct FrameDecoder {
    buffer: Vec<u8>,
    body_length: Option<usize>,
}

impl FrameDecoder {
    #[allow(clippy::expect_used, clippy::indexing_slicing)] // header_end trovato nel buffer; body_length impostata dal ramo precedente dello stesso ciclo
    pub fn push(&mut self, bytes: &[u8]) -> Result<Vec<Vec<u8>>, ProtocolError> {
        self.buffer.extend_from_slice(bytes);
        let mut frames = Vec::new();
        loop {
            if self.body_length.is_none() {
                let Some(header_end) = find_header_end(&self.buffer) else {
                    if self.buffer.len() > MAX_HEADER_BYTES {
                        return Err(ProtocolError::HeaderTooLarge {
                            size: self.buffer.len(),
                            limit: MAX_HEADER_BYTES,
                        });
                    }
                    break;
                };
                if header_end > MAX_HEADER_BYTES {
                    return Err(ProtocolError::HeaderTooLarge {
                        size: header_end,
                        limit: MAX_HEADER_BYTES,
                    });
                }
                let length = parse_content_length(&self.buffer[..header_end])?;
                if length > MAX_BODY_BYTES {
                    return Err(ProtocolError::BodyTooLarge {
                        size: length,
                        limit: MAX_BODY_BYTES,
                    });
                }
                self.buffer.drain(..header_end + 4);
                self.body_length = Some(length);
            }

            let length = self
                .body_length
                .expect("length impostata dal parser header");
            if self.buffer.len() < length {
                break;
            }
            frames.push(self.buffer.drain(..length).collect());
            self.body_length = None;
        }
        Ok(frames)
    }
}

fn find_header_end(buffer: &[u8]) -> Option<usize> {
    buffer.windows(4).position(|window| window == b"\r\n\r\n")
}

fn parse_content_length(header: &[u8]) -> Result<usize, ProtocolError> {
    let header = std::str::from_utf8(header).map_err(|_| ProtocolError::InvalidHeader)?;
    let mut content_length = None;
    for line in header.split("\r\n") {
        let Some((name, value)) = line.split_once(':') else {
            return Err(ProtocolError::InvalidHeader);
        };
        if name.trim().eq_ignore_ascii_case("content-length") {
            if content_length.is_some() {
                return Err(ProtocolError::DuplicateContentLength);
            }
            content_length = Some(
                value
                    .trim()
                    .parse::<usize>()
                    .map_err(|_| ProtocolError::InvalidContentLength)?,
            );
        }
    }
    content_length.ok_or(ProtocolError::MissingContentLength)
}

pub fn encode_message(message: &RpcMessage) -> Result<Vec<u8>, ProtocolError> {
    let body = serde_json::to_vec(message)?;
    if body.len() > MAX_BODY_BYTES {
        return Err(ProtocolError::BodyTooLarge {
            size: body.len(),
            limit: MAX_BODY_BYTES,
        });
    }
    let mut encoded = format!("Content-Length: {}\r\n\r\n", body.len()).into_bytes();
    encoded.extend_from_slice(&body);
    Ok(encoded)
}
