#[cfg(any(
    feature = "rustls",
    feature = "native-tls",
    feature = "vendored-openssl"
))]
use crate::client::{tls::TlsPreloginWrapper, tls_stream::create_tls_stream};
use crate::{
    client::{tls::MaybeTlsStream, AuthMethod, Config},
    tds::{
        codec::{
            self, Encode, LoginMessage, Packet, PacketCodec, PacketHeader, PacketStatus,
            DoneStatus, PreloginMessage, TokenDone,
        },
        stream::TokenStream,
        Context, HEADER_BYTES,
    },
    EncryptionLevel, SqlReadBytes, TokenType,
};
use asynchronous_codec::Framed;
use bytes::BytesMut;
#[cfg(any(windows, feature = "integrated-auth-gssapi", feature = "sspi-rs"))]
use codec::TokenSspi;
use futures_util::io::{AsyncRead, AsyncWrite, AsyncWriteExt};
use futures_util::ready;
use futures_util::sink::SinkExt;
use futures_util::stream::{Stream, TryStream, TryStreamExt};
#[cfg(all(unix, feature = "integrated-auth-gssapi"))]
use libgssapi::{
    context::{ClientCtx, CtxFlags},
    credential::{Cred, CredUsage},
    name::Name,
    oid::{OidSet, GSS_MECH_KRB5, GSS_NT_KRB5_PRINCIPAL},
};
use pretty_hex::*;
#[cfg(all(unix, feature = "sspi-rs"))]
use sspi::{
    AuthIdentity, BufferType, ClientRequestFlags, CredentialUse, DataRepresentation, Ntlm,
    SecurityBuffer, Sspi, SspiImpl, Username,
};
#[cfg(all(unix, feature = "integrated-auth-gssapi"))]
use std::ops::Deref;
use std::{cmp, fmt::Debug, io, pin::Pin, task};
use task::Poll;
use tracing::{event, Level};
#[cfg(all(windows, feature = "winauth"))]
use winauth::{windows::NtlmSspiBuilder, NextBytes};
use zeroize::{Zeroize, Zeroizing};

/// A `Connection` is an abstraction between the [`Client`] and the server. It
/// can be used as a `Stream` to fetch [`Packet`]s from and to `send` packets
/// splitting them to the negotiated limit automatically.
///
/// `Connection` is not meant to use directly, but as an abstraction layer for
/// the numerous `Stream`s for easy packet handling.
///
/// [`Client`]: struct.Encode.html
/// [`Packet`]: ../protocol/codec/struct.Packet.html
pub(crate) struct Connection<S>
where
    S: AsyncRead + AsyncWrite + Unpin + Send,
{
    transport: Framed<MaybeTlsStream<S>, PacketCodec>,
    flushed: bool,
    context: Context,
    buf: BytesMut,
    /// Set for the duration of a multi-packet write. A message is only partly
    /// on the wire while this is `true`; if the writing future is dropped
    /// (a cancelled `query`/`execute`, a `select!` losing the race, a
    /// `tokio::time::timeout` firing) the flag stays set, so the next write on
    /// the same connection fails cleanly instead of appending a second message
    /// after a half-sent one and silently desyncing the server.
    poisoned: bool,
}

impl<S: AsyncRead + AsyncWrite + Unpin + Send> Debug for Connection<S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Connection")
            .field("transport", &"Framed<..>")
            .field("flushed", &self.flushed)
            .field("context", &self.context)
            .field("buf", &self.buf.as_ref().hex_dump())
            .finish()
    }
}

impl<S: AsyncRead + AsyncWrite + Unpin + Send> Connection<S> {
    /// Creates a new connection
    ///
    /// Note: `tcp_stream` is a connected stream, so some parts of the
    /// [`Config`] need to be handled outside of this method.
    pub(crate) async fn connect(config: Config, tcp_stream: S) -> crate::Result<Connection<S>> {
        let context = {
            let mut context = Context::new();
            context.set_spn(config.get_host(), config.get_port());
            context
        };

        // In TDS 8.0 "strict" mode the TLS handshake happens *before* the
        // prelogin, so we wrap the stream in TLS up front. In every other mode
        // the connection starts in the clear and TLS (if any) is negotiated
        // during the prelogin.
        #[cfg(any(
            feature = "rustls",
            feature = "native-tls",
            feature = "vendored-openssl"
        ))]
        let transport = match config.encryption {
            EncryptionLevel::Strict => {
                event!(Level::DEBUG, "Performing a TLS handshake (TDS 8.0 strict)");
                let mut pre_login_stream = TlsPreloginWrapper::new(tcp_stream);
                // No prelogin framing is used for the strict handshake; pass the
                // raw TLS bytes straight through.
                pre_login_stream.handshake_complete();
                let stream = create_tls_stream(&config, pre_login_stream).await?;
                event!(Level::DEBUG, "TLS handshake successful");
                Framed::new(MaybeTlsStream::Tls(stream), PacketCodec)
            }
            _ => Framed::new(MaybeTlsStream::Raw(tcp_stream), PacketCodec),
        };

        #[cfg(not(any(
            feature = "rustls",
            feature = "native-tls",
            feature = "vendored-openssl"
        )))]
        let transport = Framed::new(MaybeTlsStream::Raw(tcp_stream), PacketCodec);

        let mut connection = Self {
            transport,
            context,
            flushed: false,
            buf: BytesMut::new(),
            poisoned: false,
        };

        let fed_auth_required = matches!(config.auth, AuthMethod::AADToken(_));

        let prelogin = connection
            .prelogin(
                config.encryption,
                fed_auth_required,
                config.instance_name.clone(),
            )
            .await?;

        let encryption = prelogin.negotiated_encryption(config.encryption)?;

        let connection = connection.tls_handshake(&config, encryption).await?;

        let mut connection = connection
            .login(
                config.auth,
                encryption,
                config.database,
                config.host,
                config.application_name,
                config.client_name,
                config.readonly,
                config.packet_size,
                prelogin,
            )
            .await?;

        connection.flush_done().await?;

        Ok(connection)
    }

    /// Flush the incoming token stream until receiving `DONE` token.
    async fn flush_done(&mut self) -> crate::Result<TokenDone> {
        TokenStream::new(self).flush_done().await
    }

    #[cfg(any(windows, feature = "integrated-auth-gssapi", feature = "sspi-rs"))]
    /// Flush the incoming token stream until receiving `SSPI` token.
    async fn flush_sspi(&mut self) -> crate::Result<TokenSspi> {
        TokenStream::new(self).flush_sspi().await
    }

    #[cfg(any(
        feature = "rustls",
        feature = "native-tls",
        feature = "vendored-openssl"
    ))]
    fn post_login_encryption(mut self, encryption: EncryptionLevel) -> Self {
        if let EncryptionLevel::Off = encryption {
            event!(
                Level::WARN,
                "Turning TLS off after a login. All traffic from here on is not encrypted.",
            );

            let Self { transport, .. } = self;
            let tcp = transport.into_inner().into_inner();
            self.transport = Framed::new(MaybeTlsStream::Raw(tcp), PacketCodec);
        }

        self
    }

    #[cfg(not(any(
        feature = "rustls",
        feature = "native-tls",
        feature = "vendored-openssl"
    )))]
    fn post_login_encryption(self, _: EncryptionLevel) -> Self {
        self
    }

    /// Send an item to the wire. Header should define the item type and item should implement
    /// [`Encode`], defining the byte structure for the wire.
    ///
    /// The `send` will split the packet into multiple packets if bigger than
    /// the negotiated packet size, and handle flushing to the wire in an optimal way.
    ///
    /// [`Encode`]: ../protocol/codec/trait.Encode.html
    pub async fn send<E>(&mut self, mut header: PacketHeader, item: E) -> crate::Result<()>
    where
        E: Sized + Encode<BytesMut>,
    {
        self.ensure_not_poisoned()?;
        self.flushed = false;
        let packet_size = (self.context.packet_size() as usize) - HEADER_BYTES;

        let mut payload = BytesMut::new();
        item.encode(&mut payload)?;

        // Mark the connection poisoned across the multi-packet write; a clean
        // completion clears it below. A future dropped mid-loop leaves it set.
        self.poisoned = true;

        while !payload.is_empty() {
            let writable = cmp::min(payload.len(), packet_size);
            let split_payload = payload.split_to(writable);

            if payload.is_empty() {
                header.set_status(PacketStatus::EndOfMessage);
            } else {
                header.set_status(PacketStatus::NormalMessage);
            }

            event!(
                Level::TRACE,
                "Sending a packet ({} bytes)",
                split_payload.len() + HEADER_BYTES,
            );

            self.write_to_wire(header, split_payload).await?;
        }

        self.flush_sink().await?;
        self.poisoned = false;

        Ok(())
    }

    /// Returns an error if a previous multi-packet write on this connection was
    /// interrupted (e.g. the query/execute future was cancelled), which would
    /// have left a partial message on the wire. The connection cannot be safely
    /// reused in that state and should be dropped.
    fn ensure_not_poisoned(&self) -> crate::Result<()> {
        if self.poisoned {
            return Err(crate::Error::Protocol(
                "connection was left in an inconsistent state by a cancelled write or cancellation and can no longer be used; open a new connection"
                    .into(),
            ));
        }
        Ok(())
    }

    async fn send_sensitive_login(
        &mut self,
        mut header: PacketHeader,
        mut payload: Zeroizing<Vec<u8>>,
    ) -> crate::Result<()> {
        self.ensure_not_poisoned()?;
        self.flushed = false;
        let packet_size = (self.context.packet_size() as usize) - HEADER_BYTES;
        let mut offset = 0;

        self.poisoned = true;

        while offset < payload.len() {
            let end = cmp::min(payload.len(), offset + packet_size);

            if end == payload.len() {
                header.set_status(PacketStatus::EndOfMessage);
            } else {
                header.set_status(PacketStatus::NormalMessage);
            }

            let mut frame = Zeroizing::new(Vec::with_capacity(HEADER_BYTES + end - offset));
            header.encode(&mut *frame)?;
            frame.extend_from_slice(&payload[offset..end]);

            let size = (frame.len() as u16).to_be_bytes();
            frame[2] = size[0];
            frame[3] = size[1];

            event!(Level::TRACE, "Sending a packet ({} bytes)", frame.len(),);

            self.transport.write_all(frame.as_slice()).await?;
            frame.zeroize();
            payload[offset..end].zeroize();
            offset = end;
        }

        (&mut *self.transport).flush().await?;
        self.poisoned = false;

        Ok(())
    }

    /// Sends a packet of data to the database.
    ///
    /// # Warning
    ///
    /// Please be sure the packet size doesn't exceed the largest allowed size
    /// dictaded by the server.
    pub(crate) async fn write_to_wire(
        &mut self,
        header: PacketHeader,
        data: BytesMut,
    ) -> crate::Result<()> {
        self.flushed = false;

        let packet = Packet::new(header, data);
        self.transport.send(packet).await?;

        Ok(())
    }

    /// Sends all pending packages to the wire.
    pub(crate) async fn flush_sink(&mut self) -> crate::Result<()> {
        self.transport.flush().await
    }

    /// Sends a TDS Attention signal (packet type `0x06`, MS-TDS section
    /// 2.2.1.6) to request cancellation of the request currently in flight on
    /// this connection, then drains the token stream until the acknowledging
    /// DONE token (with the `DONE_ATTN` status bit set) is received.
    ///
    /// The Attention message carries no payload, so it is written to the wire
    /// as a single end-of-message packet. Draining the acknowledgement leaves
    /// the connection clean and ready to be reused for further queries.
    ///
    /// The request being cancelled may have been dropped half-way through a
    /// token (a `select!` that lost the race while a row was being decoded).
    /// Two things are therefore NOT safe: parsing what is left in `buf`, and
    /// parsing the packets that follow, because a token can straddle a packet
    /// boundary and the next packet may start in the middle of one. So the
    /// leftovers are thrown away and the rest is skipped at packet level, see
    /// [`Self::drain_until_attention_ack`].
    pub(crate) async fn cancel_request(&mut self) -> crate::Result<()> {
        // Bytes of a half-read token left by a dropped decoder.
        self.buf.truncate(0);

        // Same scheme as `send`: the connection is unusable until the
        // acknowledgement has been read. If this future is dropped half-way
        // (a timeout around `cancel_query`) or the drain fails, the flag stays
        // set and the next operation fails fast instead of reading noise. A
        // connection that was already poisoned stays so: an Attention does not
        // repair a half-written request.
        let already_poisoned = self.poisoned;
        self.poisoned = true;

        let id = self.context.next_packet_id();
        let header = PacketHeader::attention(id);

        // Attention has an empty payload; send just the 8-byte header.
        self.write_to_wire(header, BytesMut::new()).await?;
        self.flush_sink().await?;

        self.drain_until_attention_ack().await?;
        self.poisoned = already_poisoned;
        Ok(())
    }

    /// Discards whole packets until the end of a message whose last token is a
    /// DONE/DONEPROC/DONEINPROC with the `DONE_ATTN` bit (MS-TDS 2.2.1.6).
    ///
    /// The last token of every response message is a DONE-family token of a
    /// fixed size, so it sits exactly in the last bytes of the message
    /// payload: that is all that is looked at, without decoding what comes
    /// before it (which may start mid-token). A response that ended normally
    /// before the server saw the Attention is followed by a second message
    /// that carries only the acknowledgement: hence the loop.
    async fn drain_until_attention_ack(&mut self) -> crate::Result<()> {
        // token type (1) + status (2) + current command (2) + row count.
        let done_len = 5 + usize::from(self.context.version().done_row_count_bytes());
        let mut tail = BytesMut::new();

        loop {
            let Some(packet) = self.try_next().await? else {
                return Err(crate::Error::Protocol(
                    "Never got a DONE token acknowledging the Attention signal.".into(),
                ));
            };
            let last = packet.is_last();
            let (_, payload) = packet.into_parts();

            tail.extend_from_slice(&payload);
            if tail.len() > done_len {
                let excess = tail.len() - done_len;
                let _ = tail.split_to(excess);
            }

            if !last {
                continue;
            }
            let is_done = matches!(
                tail.first().copied(),
                Some(t) if t == TokenType::Done as u8
                    || t == TokenType::DoneProc as u8
                    || t == TokenType::DoneInProc as u8
            );
            let status = match (tail.get(1), tail.get(2)) {
                (Some(&lo), Some(&hi)) => u16::from_le_bytes([lo, hi]),
                _ => 0,
            };
            if tail.len() == done_len && is_done && status & DoneStatus::Attention as u16 != 0 {
                return Ok(());
            }
            // A message that ended without the acknowledgement: the next one
            // carries it.
            tail.clear();
        }
    }

    /// Cleans the packet stream from previous use. It is important to use the
    /// whole stream before using the connection again. Flushing the stream
    /// makes sure we don't have any old data causing undefined behaviour after
    /// previous queries.
    ///
    /// Calling this will slow down the queries if stream is still dirty if all
    /// results are not handled.
    pub async fn flush_stream(&mut self) -> crate::Result<()> {
        // If a previous write was cancelled mid-message the connection is
        // already known-bad; fail fast rather than layering a new request on
        // top of it.
        self.ensure_not_poisoned()?;

        // Discard any partially-consumed packet payload, then drain whole
        // packets up to the end-of-message marker. Truncating `buf` and
        // re-reading on packet boundaries resynchronises the token stream even
        // if a previous result stream was dropped part-way through a value
        // (the lost bytes belonged to a packet we are discarding anyway).
        self.buf.truncate(0);

        if self.flushed {
            return Ok(());
        }

        loop {
            match self.try_next().await {
                Ok(Some(packet)) => {
                    event!(
                        Level::WARN,
                        "Flushing unhandled packet from the wire. Please consume your streams!",
                    );

                    if packet.is_last() {
                        break;
                    }
                }
                Ok(None) => break,
                // The stream could not be drained cleanly (e.g. it was
                // abandoned at an unrecoverable offset). Poison the connection
                // so it is not silently reused in an inconsistent state.
                Err(e) => {
                    self.poisoned = true;
                    return Err(e);
                }
            }
        }

        Ok(())
    }

    /// True if the underlying stream has no more data and is consumed
    /// completely.
    pub fn is_eof(&self) -> bool {
        self.flushed && self.buf.is_empty()
    }

    /// A message sent by the client to set up context for login. The server
    /// responds to a client PRELOGIN message with a message of packet header
    /// type 0x04 and with the packet data containing a PRELOGIN structure.
    ///
    /// This message stream is also used to wrap the TLS handshake payload if
    /// encryption is needed. In this scenario, where PRELOGIN message is
    /// transporting the TLS handshake payload, the packet data is simply the
    /// raw bytes of the TLS handshake payload.
    async fn prelogin(
        &mut self,
        encryption: EncryptionLevel,
        fed_auth_required: bool,
        instance_name: Option<String>,
    ) -> crate::Result<PreloginMessage> {
        let mut msg = PreloginMessage::new();
        msg.encryption = encryption;
        msg.fed_auth_required = fed_auth_required;
        msg.instance_name = instance_name.clone();

        let id = self.context.next_packet_id();
        self.send(PacketHeader::pre_login(id), msg).await?;

        let response: PreloginMessage = codec::collect_from(self).await?;
        // threadid (should be empty when sent from server to client)
        debug_assert_eq!(response.thread_id, 0);
        // ensure the server accepted the instance we asked it to validate
        response.validate_instance(instance_name.as_deref())?;
        Ok(response)
    }

    /// Defines the login record rules with SQL Server. Authentication with
    /// connection options.
    #[allow(clippy::too_many_arguments)]
    async fn login(
        mut self,
        auth: AuthMethod,
        encryption: EncryptionLevel,
        db: Option<String>,
        server_name: Option<String>,
        application_name: Option<String>,
        client_name: Option<String>,
        readonly: bool,
        packet_size: Option<u32>,
        prelogin: PreloginMessage,
    ) -> crate::Result<Self> {
        let mut login_message = LoginMessage::new();

        if let Some(db) = db {
            login_message.db_name(db);
        }

        if let Some(server_name) = server_name {
            login_message.server_name(server_name);
        }

        if let Some(app_name) = application_name {
            login_message.app_name(app_name);
        }

        if let Some(client_name) = client_name {
            login_message.hostname(client_name);
        }

        login_message.readonly(readonly);

        if let Some(size) = packet_size {
            login_message.packet_size(size);
        }

        match auth {
            #[cfg(all(windows, feature = "winauth"))]
            AuthMethod::Integrated => {
                let mut client = NtlmSspiBuilder::new()
                    .target_spn(self.context.spn())
                    .build()?;

                login_message.integrated_security(client.next_bytes(None)?);

                let id = self.context.next_packet_id();
                self.send(PacketHeader::login(id), login_message).await?;

                self = self.post_login_encryption(encryption);

                let sspi_bytes = self.flush_sspi().await?;

                match client.next_bytes(Some(sspi_bytes.as_ref()))? {
                    Some(sspi_response) => {
                        event!(Level::TRACE, sspi_response_len = sspi_response.len());

                        let id = self.context.next_packet_id();
                        let header = PacketHeader::sspi(id);

                        let token = TokenSspi::new(sspi_response);
                        self.send(header, token).await?;
                    }
                    None => {
                        return Err(crate::Error::Protocol(
                            "NTLM handshake produced no response to the server challenge".into(),
                        ))
                    }
                }
            }
            #[cfg(all(unix, feature = "integrated-auth-gssapi"))]
            AuthMethod::Integrated => {
                let mut s = OidSet::new();
                s.add(GSS_MECH_KRB5)?;

                let client_cred = Cred::acquire(None, None, CredUsage::Initiate, Some(&s))?;

                let mut ctx = ClientCtx::new(
                    Some(client_cred),
                    Name::new(self.context.spn().as_bytes(), Some(GSS_NT_KRB5_PRINCIPAL))?,
                    CtxFlags::GSS_C_MUTUAL_FLAG | CtxFlags::GSS_C_SEQUENCE_FLAG,
                    None,
                );

                let init_token = ctx.step(None, None)?;

                login_message.integrated_security(Some(Vec::from(init_token.unwrap().deref())));

                let id = self.context.next_packet_id();
                self.send(PacketHeader::login(id), login_message).await?;

                self = self.post_login_encryption(encryption);

                let auth_bytes = self.flush_sspi().await?;

                let next_token = match ctx.step(Some(auth_bytes.as_ref()), None)? {
                    Some(response) => {
                        event!(Level::TRACE, response_len = response.len());
                        TokenSspi::new(Vec::from(response.deref()))
                    }
                    None => {
                        event!(Level::TRACE, response_len = 0);
                        TokenSspi::new(Vec::new())
                    }
                };

                let id = self.context.next_packet_id();
                let header = PacketHeader::login(id);

                self.send(header, next_token).await?;
            }
            #[cfg(all(unix, feature = "sspi-rs"))]
            AuthMethod::Windows(auth) => {
                let mut ntlm = Ntlm::new();

                let username =
                    Username::new(&auth.user, auth.domain.as_deref()).map_err(sspi::Error::from)?;

                let identity = AuthIdentity {
                    username,
                    password: auth.password.to_string().into(),
                };

                let mut creds = ntlm
                    .acquire_credentials_handle()
                    .with_credential_use(CredentialUse::Outbound)
                    .with_auth_data(&identity)
                    .execute(&mut ntlm)?;

                let spn = self.context.spn().to_string();

                // First leg of the NTLM handshake: produce the NEGOTIATE token
                // and ship it in the login packet as integrated security data.
                let mut input = vec![SecurityBuffer::new(Vec::new(), BufferType::Token)];
                let mut output = vec![SecurityBuffer::new(Vec::new(), BufferType::Token)];

                let mut builder = ntlm
                    .initialize_security_context()
                    .with_credentials_handle(&mut creds.credentials_handle)
                    .with_context_requirements(
                        ClientRequestFlags::CONFIDENTIALITY | ClientRequestFlags::ALLOCATE_MEMORY,
                    )
                    .with_target_data_representation(DataRepresentation::Native)
                    .with_target_name(&spn)
                    .with_input(&mut input)
                    .with_output(&mut output);

                ntlm.initialize_security_context_impl(&mut builder)?
                    .resolve_to_result()?;

                login_message.integrated_security(Some(output[0].buffer.clone()));

                let id = self.context.next_packet_id();
                self.send(PacketHeader::login(id), login_message).await?;
                self = self.post_login_encryption(encryption);

                // Second leg: consume the server's CHALLENGE token and reply
                // with the AUTHENTICATE token.
                let sspi_bytes = self.flush_sspi().await?;

                let mut input = vec![SecurityBuffer::new(
                    sspi_bytes.as_ref().to_vec(),
                    BufferType::Token,
                )];
                let mut output = vec![SecurityBuffer::new(Vec::new(), BufferType::Token)];

                let mut builder = ntlm
                    .initialize_security_context()
                    .with_credentials_handle(&mut creds.credentials_handle)
                    .with_context_requirements(
                        ClientRequestFlags::CONFIDENTIALITY | ClientRequestFlags::ALLOCATE_MEMORY,
                    )
                    .with_target_data_representation(DataRepresentation::Native)
                    .with_target_name(&spn)
                    .with_input(&mut input)
                    .with_output(&mut output);

                ntlm.initialize_security_context_impl(&mut builder)?
                    .resolve_to_result()?;

                event!(Level::TRACE, authenticate_len = output[0].buffer.len());

                let id = self.context.next_packet_id();
                self.send(
                    PacketHeader::login(id),
                    TokenSspi::new(output[0].buffer.clone()),
                )
                .await?;
            }
            #[cfg(all(windows, feature = "winauth"))]
            AuthMethod::Windows(auth) => {
                let spn = self.context.spn().to_string();
                let builder = winauth::NtlmV2ClientBuilder::new().target_spn(spn);
                let mut client = builder.build(auth.domain, auth.user, auth.password.to_string());

                login_message.integrated_security(client.next_bytes(None)?);

                let id = self.context.next_packet_id();
                self.send(PacketHeader::login(id), login_message).await?;

                self = self.post_login_encryption(encryption);

                let sspi_bytes = self.flush_sspi().await?;

                match client.next_bytes(Some(sspi_bytes.as_ref()))? {
                    Some(sspi_response) => {
                        event!(Level::TRACE, sspi_response_len = sspi_response.len());

                        let id = self.context.next_packet_id();
                        let header = PacketHeader::login(id);

                        let token = TokenSspi::new(sspi_response);
                        self.send(header, token).await?;
                    }
                    None => {
                        return Err(crate::Error::Protocol(
                            "NTLM handshake produced no response to the server challenge".into(),
                        ))
                    }
                }
            }
            AuthMethod::None => {
                let id = self.context.next_packet_id();
                self.send(PacketHeader::login(id), login_message).await?;
                self = self.post_login_encryption(encryption);
            }
            AuthMethod::SqlServer(auth) => {
                let (user, mut password) = auth.into_credentials();

                login_message.user_name(user);
                login_message.password(password.as_str());
                let payload = login_message.encode_to_vec()?;
                password.zeroize();

                let id = self.context.next_packet_id();
                self.send_sensitive_login(PacketHeader::login(id), payload)
                    .await?;
                self = self.post_login_encryption(encryption);
            }
            AuthMethod::AADToken(token) => {
                login_message.aad_token(token, prelogin.fed_auth_required, prelogin.nonce);
                // Encode into a zeroizing buffer and use the sensitive-login
                // path so the bearer token does not linger in freed heap memory.
                let payload = login_message.encode_to_vec()?;
                let id = self.context.next_packet_id();
                self.send_sensitive_login(PacketHeader::login(id), payload)
                    .await?;
                self = self.post_login_encryption(encryption);
            }
        }

        Ok(self)
    }

    /// Implements the TLS handshake with the SQL Server.
    #[cfg(any(
        feature = "rustls",
        feature = "native-tls",
        feature = "vendored-openssl"
    ))]
    async fn tls_handshake(
        self,
        config: &Config,
        encryption: EncryptionLevel,
    ) -> crate::Result<Self> {
        match encryption {
            EncryptionLevel::NotSupported => {
                event!(
                    Level::WARN,
                    "TLS encryption is not enabled. All traffic including the login credentials are not encrypted."
                );

                Ok(self)
            }
            // In strict mode the handshake already happened before the prelogin,
            // so the transport is already a TLS stream. Nothing to do here.
            EncryptionLevel::Strict => {
                event!(
                    Level::TRACE,
                    "Already in a TLS stream (TDS 8.0 strict), skipping handshake."
                );

                Ok(self)
            }
            EncryptionLevel::Off | EncryptionLevel::On | EncryptionLevel::Required => {
                event!(Level::DEBUG, "Performing a TLS handshake");

                let Self {
                    transport, context, ..
                } = self;
                let mut stream = match transport.into_inner() {
                    MaybeTlsStream::Raw(tcp) => {
                        create_tls_stream(config, TlsPreloginWrapper::new(tcp)).await?
                    }
                    _ => unreachable!(),
                };

                stream.get_mut().handshake_complete();
                event!(Level::DEBUG, "TLS handshake successful");

                let transport = Framed::new(MaybeTlsStream::Tls(stream), PacketCodec);

                Ok(Self {
                    transport,
                    context,
                    flushed: false,
                    buf: BytesMut::new(),
                    poisoned: false,
                })
            }
        }
    }

    /// Implements the TLS handshake with the SQL Server.
    #[cfg(not(any(
        feature = "rustls",
        feature = "native-tls",
        feature = "vendored-openssl"
    )))]
    async fn tls_handshake(self, config: &Config, _: EncryptionLevel) -> crate::Result<Self> {
        // Without a TLS backend compiled in, we cannot encrypt anything. If the
        // user asked for encryption, fail loudly instead of silently sending
        // traffic (including login credentials) in the clear.
        check_tls_backend_available(config.encryption)?;

        event!(
            Level::WARN,
            "TLS encryption is not enabled. All traffic including the login credentials are not encrypted."
        );

        Ok(self)
    }

    pub(crate) async fn close(mut self) -> crate::Result<()> {
        self.transport.close().await
    }
}

/// Returns an error when the user requested encryption but no TLS backend was
/// compiled in. Without this check, a `Required`/`On` encryption request would
/// silently fall back to an unencrypted connection.
#[cfg(not(any(
    feature = "rustls",
    feature = "native-tls",
    feature = "vendored-openssl"
)))]
fn check_tls_backend_available(encryption: EncryptionLevel) -> crate::Result<()> {
    if let EncryptionLevel::On | EncryptionLevel::Required | EncryptionLevel::Strict = encryption {
        return Err(crate::Error::Tls(
            "TLS encryption was requested but the crate was compiled without a TLS backend. \
             Enable one of the `native-tls`, `rustls` or `vendored-openssl` features."
                .to_string(),
        ));
    }

    Ok(())
}

#[cfg(all(
    test,
    not(any(
        feature = "rustls",
        feature = "native-tls",
        feature = "vendored-openssl"
    ))
))]
mod tests {
    use super::check_tls_backend_available;
    use crate::EncryptionLevel;

    #[test]
    fn requested_encryption_without_tls_backend_errors() {
        assert!(check_tls_backend_available(EncryptionLevel::Required).is_err());
        assert!(check_tls_backend_available(EncryptionLevel::On).is_err());
    }

    #[test]
    fn no_encryption_without_tls_backend_is_ok() {
        assert!(check_tls_backend_available(EncryptionLevel::Off).is_ok());
        assert!(check_tls_backend_available(EncryptionLevel::NotSupported).is_ok());
    }
}

impl<S: AsyncRead + AsyncWrite + Unpin + Send> Stream for Connection<S> {
    type Item = crate::Result<Packet>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut task::Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();

        match ready!(this.transport.try_poll_next_unpin(cx)) {
            Some(Ok(packet)) => {
                this.flushed = packet.is_last();
                Poll::Ready(Some(Ok(packet)))
            }
            Some(Err(e)) => Poll::Ready(Some(Err(e))),
            None => Poll::Ready(None),
        }
    }
}

impl<S: AsyncRead + AsyncWrite + Unpin + Send> futures_util::io::AsyncRead for Connection<S> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut task::Context<'_>,
        buf: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        let mut this = self.get_mut();
        let size = buf.len();

        if this.buf.len() < size {
            while let Some(item) = ready!(Pin::new(&mut this).try_poll_next(cx)) {
                match item {
                    Ok(packet) => {
                        let (_, payload) = packet.into_parts();
                        this.buf.extend(payload);

                        if this.buf.len() >= size {
                            break;
                        }
                    }
                    Err(e) => {
                        return Poll::Ready(Err(io::Error::new(
                            io::ErrorKind::BrokenPipe,
                            e.to_string(),
                        )))
                    }
                }
            }

            // Got EOF before having all the data.
            if this.buf.len() < size {
                return Poll::Ready(Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "No more packets in the wire",
                )));
            }
        }

        buf.copy_from_slice(this.buf.split_to(size).as_ref());
        Poll::Ready(Ok(size))
    }
}

impl<S: AsyncRead + AsyncWrite + Unpin + Send> SqlReadBytes for Connection<S> {
    /// Hex dump of the current buffer.
    fn debug_buffer(&self) {
        dbg!(self.buf.as_ref().hex_dump());
    }

    /// The current execution context.
    fn context(&self) -> &Context {
        &self.context
    }

    /// A mutable reference to the current execution context.
    fn context_mut(&mut self) -> &mut Context {
        &mut self.context
    }
}

/// Prove di `cancel_request` su una `Connection` vera, con byte veri su un
/// canale in memoria (tokio::io::duplex): il lato «server» è un semplice
/// scrittore di pacchetti TDS, non un server finto. Serve a provare la
/// risincronizzazione dopo un decoder lasciato cadere a metà token.
#[cfg(test)]
mod cancel_tests {
    use super::*;
    use crate::tds::codec::PacketHeader;
    use tokio::io::{AsyncReadExt, AsyncWriteExt, DuplexStream};
    use tokio_util::compat::{Compat, TokioAsyncReadCompatExt};

    fn connessione_da(stream: DuplexStream) -> Connection<Compat<DuplexStream>> {
        Connection {
            transport: Framed::new(MaybeTlsStream::Raw(stream.compat()), PacketCodec),
            flushed: false,
            context: Context::new(),
            buf: BytesMut::new(),
            poisoned: false,
        }
    }

    /// Un pacchetto di risposta (tipo 0x04) con il payload dato.
    fn pacchetto(payload: &[u8], ultimo: bool) -> Vec<u8> {
        let mut header = PacketHeader::new(0, 1);
        header.set_type(codec::PacketType::TabularResult);
        header.set_status(if ultimo {
            PacketStatus::EndOfMessage
        } else {
            PacketStatus::NormalMessage
        });
        let mut out = BytesMut::new();
        Packet::new(header, BytesMut::from(payload))
            .encode(&mut out)
            .unwrap();
        out.to_vec()
    }

    /// Token DONE (0xFD) di 13 byte, con lo stato dato.
    fn done(stato: u16) -> Vec<u8> {
        let mut out = vec![TokenType::Done as u8];
        out.extend_from_slice(&stato.to_le_bytes());
        out.extend_from_slice(&0_u16.to_le_bytes());
        out.extend_from_slice(&0_u64.to_le_bytes());
        out
    }

    const ATTN: u16 = DoneStatus::Attention as u16;

    /// Il caso del SELECT grosso: il decoder aspetta il resto di un valore e
    /// viene lasciato cadere; in `buf` restano i byte già letti. La conferma
    /// arriva nell'ultimo pacchetto, dopo altri pacchetti che partono a metà
    /// token. `cancel_request` non deve leggere nulla di questo come token.
    #[tokio::test]
    async fn cancel_a_meta_token_scarta_il_buffer_e_salta_fino_alla_conferma() {
        let (client, mut server) = tokio::io::duplex(64 * 1024);
        let mut connessione = connessione_da(client);

        // Un valore da 100 byte di cui ne arrivano 10: `read_exact` resta in
        // attesa, poi il futuro viene lasciato cadere (è ciò che fa il select!).
        server.write_all(&pacchetto(&[0xAB; 10], false)).await.unwrap();
        let mut valore = [0_u8; 100];
        let attesa = tokio::time::timeout(
            std::time::Duration::from_millis(100),
            futures_util::io::AsyncReadExt::read_exact(&mut connessione, &mut valore),
        )
        .await;
        assert!(attesa.is_err(), "il valore incompleto doveva restare in attesa");
        assert_eq!(connessione.buf.len(), 10, "restano i byte del token a metà");

        // Il server continua: un pacchetto che finisce con byte identici a un
        // DONE di conferma ma non è l'ultimo, poi l'ultimo con la conferma vera.
        let mut centrale = vec![0x01; 30];
        centrale.extend_from_slice(&done(ATTN));
        server.write_all(&pacchetto(&centrale, false)).await.unwrap();
        let mut finale = vec![0xD1, 0x02, 0x03];
        finale.extend_from_slice(&done(ATTN));
        server.write_all(&pacchetto(&finale, true)).await.unwrap();

        tokio::time::timeout(
            std::time::Duration::from_secs(5),
            connessione.cancel_request(),
        )
        .await
        .expect("la conferma è arrivata: non doveva restare appesa")
        .expect("la conferma doveva essere riconosciuta");

        assert!(connessione.buf.is_empty());
        assert!(connessione.flushed, "la connessione è pronta a riuso");

        // E l'Attention è uscita sul filo: 8 byte, tipo 0x06, fine messaggio.
        let mut uscita = [0_u8; 8];
        server.read_exact(&mut uscita).await.unwrap();
        assert_eq!(uscita[0], 0x06);
        assert_eq!(uscita[1], 0x01);
    }

    /// La risposta era già finita (DONE senza Attention) quando il server ha
    /// visto l'Attention: la conferma arriva in un secondo messaggio.
    #[tokio::test]
    async fn cancel_dopo_una_risposta_finita_aspetta_il_messaggio_di_conferma() {
        let (client, mut server) = tokio::io::duplex(64 * 1024);
        let mut connessione = connessione_da(client);

        let mut risposta = vec![0x55; 7];
        risposta.extend_from_slice(&done(0));
        server.write_all(&pacchetto(&risposta, true)).await.unwrap();
        server
            .write_all(&pacchetto(&done(ATTN), true))
            .await
            .unwrap();

        tokio::time::timeout(
            std::time::Duration::from_secs(5),
            connessione.cancel_request(),
        )
        .await
        .expect("non doveva restare appesa")
        .expect("la conferma del secondo messaggio doveva essere riconosciuta");
    }

    /// Il DONE di conferma è spezzato fra due pacchetti (6 + 7 byte, l'ultimo
    /// con fine messaggio): la coda scorrevole deve ricomporlo.
    #[tokio::test]
    async fn cancel_riconosce_la_conferma_spezzata_in_due_pacchetti() {
        let (client, mut server) = tokio::io::duplex(64 * 1024);
        let mut connessione = connessione_da(client);

        let conferma = done(ATTN);
        assert_eq!(conferma.len(), 13);
        let mut primo = vec![0x42; 9];
        primo.extend_from_slice(&conferma[..6]);
        server.write_all(&pacchetto(&primo, false)).await.unwrap();
        server
            .write_all(&pacchetto(&conferma[6..], true))
            .await
            .unwrap();

        tokio::time::timeout(
            std::time::Duration::from_secs(5),
            connessione.cancel_request(),
        )
        .await
        .expect("non doveva restare appesa")
        .expect("la conferma spezzata doveva essere riconosciuta");
        assert!(!connessione.poisoned);
    }

    /// Un `cancel_request` interrotto (il timeout di chi lo chiama) lascia la
    /// connessione segnata: la prossima operazione fallisce subito invece di
    /// leggere il rumore della risposta non ancora scartata. Dopo una
    /// conferma valida invece torna usabile.
    #[tokio::test]
    async fn cancel_interrotto_avvelena_la_connessione_e_dopo_la_conferma_torna_usabile() {
        let (client, mut server) = tokio::io::duplex(64 * 1024);
        let mut connessione = connessione_da(client);

        // Il server manda rumore senza mai confermare: il drain resta in attesa.
        server.write_all(&pacchetto(&[0x11; 20], false)).await.unwrap();
        let esito = tokio::time::timeout(
            std::time::Duration::from_millis(200),
            connessione.cancel_request(),
        )
        .await;
        assert!(esito.is_err(), "senza conferma doveva restare in attesa");

        // Con un timeout: se la connessione non fosse segnata, `flush_stream`
        // resterebbe ad aspettare i pacchetti del server invece di fallire.
        let errore = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            connessione.flush_stream(),
        )
        .await
        .expect("la connessione doveva essere segnata: fallire subito")
        .unwrap_err();
        assert!(errore.to_string().contains("inconsistent"), "{errore}");

        // Conferma valida su una connessione nuova: torna usabile.
        let (client, mut server) = tokio::io::duplex(64 * 1024);
        let mut connessione = connessione_da(client);
        server.write_all(&pacchetto(&done(ATTN), true)).await.unwrap();
        connessione.cancel_request().await.unwrap();
        connessione.flush_stream().await.unwrap();

        // Un errore durante il drain lascia il segno (flusso chiuso).
        let (client, mut server) = tokio::io::duplex(64 * 1024);
        let mut connessione = connessione_da(client);
        server.shutdown().await.unwrap();
        assert!(connessione.cancel_request().await.is_err());
        assert!(connessione.flush_stream().await.is_err());
    }

    /// Se il server chiude senza confermare l'errore è esplicito, non un
    /// ciclo infinito.
    #[tokio::test]
    async fn cancel_senza_conferma_e_un_errore_di_protocollo() {
        let (client, mut server) = tokio::io::duplex(64 * 1024);
        let mut connessione = connessione_da(client);
        server.write_all(&pacchetto(&done(0), true)).await.unwrap();
        // Il server smette di scrivere ma tiene aperta la lettura.
        server.shutdown().await.unwrap();

        let errore = connessione.cancel_request().await.unwrap_err();
        assert!(errore.to_string().contains("Attention"), "{errore}");
    }
}
