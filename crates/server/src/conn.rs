use std::{sync::Arc, time::Duration};

use bytes::BytesMut;
use tokio::sync::mpsc;
use tracing::{debug, info, warn};
use whisperia_proto::{
    audio::{stamp_sender, AudioPacket},
    control::{encode_frame, ClientMsg, FrameDecoder, Password, RejectReason, ServerMsg},
    peek::Glance,
    PeerId, ALPN_MEDIA, ALPN_PEEK, PROTOCOL_VERSION,
};

use crate::{hub::Admitted, Shared};

pub(crate) const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);

pub(crate) const CLOSE_OK: u32 = 0;
pub(crate) const CLOSE_REJECTED: u32 = 1;
const CLOSE_PROTOCOL: u32 = 2;

#[derive(Debug, thiserror::Error)]
enum ConnError {
    #[error("connection failed: {0}")]
    Connection(#[from] quinn::ConnectionError),
    #[error("control stream failed: {0}")]
    Read(#[from] quinn::ReadError),
    #[error("control stream failed: {0}")]
    Write(#[from] quinn::WriteError),
    #[error(transparent)]
    Frame(#[from] whisperia_proto::control::FrameError),
    #[error("handshake timed out")]
    Timeout,
    #[error("client closed the control stream")]
    StreamClosed,
    #[error("first message was not Hello")]
    NoHello,
    #[error("no randomness for a media token: {0}")]
    Random(getrandom::Error),
}

pub(crate) fn admission(
    protocol: u16,
    given: Option<&Password>,
    wanted: Option<&Password>,
) -> Result<(), RejectReason> {
    if protocol != PROTOCOL_VERSION {
        return Err(RejectReason::ProtocolMismatch {
            server: PROTOCOL_VERSION,
        });
    }
    match wanted {
        Some(wanted) if given != Some(wanted) => Err(RejectReason::BadPassword),
        _ => Ok(()),
    }
}

pub(crate) async fn serve(incoming: quinn::Incoming, shared: Arc<Shared>) {
    let remote = incoming.remote_address();
    let conn = match incoming.await {
        Ok(c) => c,
        Err(e) => return debug!(%remote, "handshake failed: {e}"),
    };
    let protocol = conn
        .handshake_data()
        .and_then(|data| data.downcast::<quinn::crypto::rustls::HandshakeData>().ok())
        .and_then(|data| data.protocol);
    if protocol.as_deref() == Some(ALPN_MEDIA) {
        return crate::media::serve(conn, shared).await;
    }
    if protocol.as_deref() == Some(ALPN_PEEK) {
        if let Err(e) = answer_peek(&conn, &shared).await {
            debug!(%remote, "peek: {e}");
        }
        return conn.close(CLOSE_OK.into(), b"bye");
    }

    let mut joined: Option<PeerId> = None;
    let result = session(&conn, &shared, &mut joined).await;

    if let Some(id) = joined {
        if let Some(name) = shared.change(|hub| hub.disconnect(id)) {
            info!(%remote, id, %name, "left");
        }
    }
    match result {
        Ok(()) | Err(ConnError::Connection(_)) | Err(ConnError::StreamClosed) => {
            conn.close(CLOSE_OK.into(), b"bye")
        }
        Err(e) => {
            warn!(%remote, "dropping client: {e}");
            conn.close(CLOSE_PROTOCOL.into(), e.to_string().as_bytes());
        }
    }
}

async fn session(
    conn: &quinn::Connection,
    shared: &Arc<Shared>,
    joined: &mut Option<PeerId>,
) -> Result<(), ConnError> {
    let remote = conn.remote_address();
    let (mut send, mut recv) = tokio::time::timeout(HANDSHAKE_TIMEOUT, conn.accept_bi())
        .await
        .map_err(|_| ConnError::Timeout)??;

    let mut decoder = FrameDecoder::new();
    let hello = tokio::time::timeout(HANDSHAKE_TIMEOUT, read_msg(&mut recv, &mut decoder))
        .await
        .map_err(|_| ConnError::Timeout)??;
    let ClientMsg::Hello {
        protocol,
        name,
        avatar,
        password,
    } = hello
    else {
        return Err(ConnError::NoHello);
    };

    let mut token = [0u8; 8];
    getrandom::fill(&mut token).map_err(ConnError::Random)?;
    let media_token = u64::from_be_bytes(token);
    let (control_tx, mut control_rx) = mpsc::unbounded_channel();
    let answers = control_tx.clone();
    let admitted =
        admission(protocol, password.as_ref(), shared.password.as_ref()).and_then(|()| {
            shared
                .change(|hub| hub.connect(&name, avatar, conn.clone(), control_tx, media_token))
                .map_err(|_| RejectReason::ServerFull)
        });
    let Admitted { id, name, rooms } = match admitted {
        Ok(admitted) => admitted,
        Err(reason) => {
            info!(%remote, %name, "rejected: {reason}");
            send.write_all(&encode_frame(&ServerMsg::Rejected(reason))?)
                .await?;
            let _ = send.finish();
            let _ = tokio::time::timeout(Duration::from_secs(1), send.stopped()).await;
            conn.close(CLOSE_REJECTED.into(), b"rejected");
            return Ok(());
        }
    };
    *joined = Some(id);
    info!(%remote, id, %name, "connected");

    let welcome = ServerMsg::Welcome {
        id,
        name,
        server_name: shared.name.clone(),
        rooms,
        media_token,
    };
    send.write_all(&encode_frame(&welcome)?).await?;

    let writer = async {
        while let Some(msg) = control_rx.recv().await {
            send.write_all(&encode_frame(&msg)?).await?;
        }
        Ok::<(), ConnError>(())
    };

    let reader = async {
        loop {
            let msg = read_msg(&mut recv, &mut decoder).await?;
            if let ClientMsg::Hello { .. } = msg {
                return Err(ConnError::NoHello);
            }
            if let Some(answer) = handle(shared, id, msg) {
                let _ = answers.send(answer);
            }
        }
    };

    let forwarder = async {
        loop {
            let datagram = conn.read_datagram().await?;
            let Ok(packet) = AudioPacket::parse(&datagram) else {
                continue;
            };
            let kind = packet.kind;
            let mut stamped = BytesMut::from(&datagram[..]);
            if stamp_sender(&mut stamped, id).is_err() {
                continue;
            }
            let stamped = stamped.freeze();
            shared.read(|hub| {
                for target in hub.audio_targets(id, kind) {
                    let _ = target.send_datagram(stamped.clone());
                }
            });
        }
    };

    tokio::select! {
        r = writer => r,
        r = reader => r,
        r = forwarder => r,
    }
}

fn handle(shared: &Shared, id: PeerId, msg: ClientMsg) -> Option<ServerMsg> {
    match msg {
        ClientMsg::Join {
            room,
            password,
            rules,
        } => Some(
            match shared.change(|hub| hub.join(id, &room, password.as_ref(), rules)) {
                Ok((peers, state)) => {
                    info!(id, %room, "joined");
                    ServerMsg::Joined { room, peers, state }
                }
                Err(e) => ServerMsg::JoinRefused(e),
            },
        ),
        ClientMsg::Leave => {
            shared.change(|hub| hub.leave(id));
            None
        }
        ClientMsg::State(state) => {
            shared.change(|hub| hub.set_state(id, state));
            None
        }
        ClientMsg::Rename(name) => {
            shared.change(|hub| hub.rename(id, &name));
            None
        }
        ClientMsg::SetAvatar(avatar) => {
            shared.change(|hub| hub.set_avatar(id, avatar));
            None
        }
        ClientMsg::Chat(text) => {
            shared.read(|hub| hub.chat(id, text));
            None
        }
        ClientMsg::ShareStart(info) => {
            shared.change(|hub| hub.share_start(id, info));
            None
        }
        ClientMsg::ShareStop => {
            shared.change(|hub| hub.share_stop(id));
            None
        }
        ClientMsg::Watch(sharer) => {
            shared.change(|hub| hub.watch(id, sharer));
            None
        }
        ClientMsg::Unwatch(sharer) => {
            shared.change(|hub| hub.unwatch(id, sharer));
            None
        }
        ClientMsg::NeedKeyframe(sharer) => {
            shared.read(|hub| hub.need_keyframe(id, sharer));
            None
        }
        ClientMsg::SetRoomPassword(password) => {
            shared.change(|hub| hub.set_room_password(id, password));
            None
        }
        ClientMsg::SetRoomLimit(limit) => {
            shared.change(|hub| hub.set_room_limit(id, limit));
            None
        }
        ClientMsg::MakeHost(heir) => {
            shared.change(|hub| hub.make_host(id, heir));
            None
        }
        ClientMsg::Kick(target) => {
            if shared.change(|hub| hub.kick(id, target)) {
                info!(id, target, "kicked from the room");
            }
            None
        }
        ClientMsg::Hello { .. } => None,
    }
}

async fn answer_peek(conn: &quinn::Connection, shared: &Shared) -> Result<(), ConnError> {
    let occupancy = shared.read(|hub| hub.occupancy());
    let glance = Glance {
        protocol: PROTOCOL_VERSION,
        name: shared.name.to_string(),
        rooms: u32::try_from(occupancy.rooms).unwrap_or(u32::MAX),
        people: u32::try_from(occupancy.people).unwrap_or(u32::MAX),
    };
    let mut send = conn.open_uni().await?;
    send.write_all(&encode_frame(&glance)?).await?;
    let _ = send.finish();
    let _ = tokio::time::timeout(HANDSHAKE_TIMEOUT, conn.closed()).await;
    Ok(())
}

async fn read_msg(
    recv: &mut quinn::RecvStream,
    decoder: &mut FrameDecoder,
) -> Result<ClientMsg, ConnError> {
    let mut chunk = [0u8; 1024];
    loop {
        if let Some(msg) = decoder.next_message()? {
            return Ok(msg);
        }
        match recv.read(&mut chunk).await? {
            Some(n) => decoder.push(&chunk[..n]),
            None => return Err(ConnError::StreamClosed),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn password(text: &str) -> Password {
        Password::new(text).unwrap()
    }

    #[test]
    fn a_client_of_another_version_is_refused_first() {
        assert_eq!(
            admission(PROTOCOL_VERSION + 1, None, Some(&password("owl"))),
            Err(RejectReason::ProtocolMismatch {
                server: PROTOCOL_VERSION
            })
        );
    }

    #[test]
    fn a_server_with_a_password_wants_that_password() {
        let owl = password("owl");
        assert_eq!(
            admission(PROTOCOL_VERSION, None, Some(&owl)),
            Err(RejectReason::BadPassword)
        );
        assert_eq!(
            admission(PROTOCOL_VERSION, Some(&password("OWL")), Some(&owl)),
            Err(RejectReason::BadPassword)
        );
        assert_eq!(admission(PROTOCOL_VERSION, Some(&owl), Some(&owl)), Ok(()));
    }

    #[test]
    fn a_server_without_a_password_lets_anybody_in() {
        assert_eq!(
            admission(PROTOCOL_VERSION, Some(&password("owl")), None),
            Ok(())
        );
    }
}
