use std::{collections::HashMap, sync::Arc};

use bytes::Bytes;
use tokio::sync::mpsc;
use tracing::{debug, info};
use whisperia_proto::{
    control::FrameDecoder,
    video::{stamp_sharer, MediaHello, PictureHeader, HEADER_LEN, MAX_IN_FLIGHT, MAX_PICTURE},
    PeerId, PROTOCOL_VERSION,
};

use crate::{
    conn::{CLOSE_OK, CLOSE_REJECTED, HANDSHAKE_TIMEOUT},
    Shared,
};

const MAX_HELLO: usize = 64;

pub(crate) async fn serve(conn: quinn::Connection, shared: Arc<Shared>) {
    let remote = conn.remote_address();
    let id = match tokio::time::timeout(HANDSHAKE_TIMEOUT, hello(&conn, &shared)).await {
        Ok(Ok(id)) => id,
        Ok(Err(reason)) => {
            debug!(%remote, "media connection refused: {reason}");
            return conn.close(CLOSE_REJECTED.into(), reason.as_bytes());
        }
        Err(_) => return conn.close(CLOSE_REJECTED.into(), b"no hello"),
    };
    info!(%remote, id, "media connection attached");

    let reason = forward(&conn, id, &shared).await;
    debug!(%remote, id, "media connection ended: {reason}");
    shared.change(|hub| hub.detach_media(id, |held| held.stable_id() == conn.stable_id()));
    conn.close(CLOSE_OK.into(), b"bye");
}

async fn hello(conn: &quinn::Connection, shared: &Shared) -> Result<PeerId, String> {
    let mut stream = conn.accept_uni().await.map_err(|e| e.to_string())?;
    let bytes = stream
        .read_to_end(MAX_HELLO)
        .await
        .map_err(|e| e.to_string())?;
    let mut decoder = FrameDecoder::new();
    decoder.push(&bytes);
    let hello: MediaHello = decoder
        .next_message()
        .map_err(|e| e.to_string())?
        .ok_or("incomplete hello")?;
    if hello.protocol != PROTOCOL_VERSION {
        return Err("protocol mismatch".into());
    }
    if !shared.change(|hub| hub.attach_media(hello.peer, hello.token, conn.clone())) {
        return Err("unknown peer or wrong token".into());
    }
    Ok(hello.peer)
}

struct Watcher {
    queue: mpsc::Sender<Bytes>,
    connection: usize,

    waiting_for_keyframe: bool,
}

impl Watcher {
    fn new(media: quinn::Connection) -> Self {
        let (queue, mut pictures) = mpsc::channel::<Bytes>(MAX_IN_FLIGHT);
        let connection = media.stable_id();

        tokio::spawn(async move {
            while let Some(picture) = pictures.recv().await {
                let Ok(mut stream) = media.open_uni().await else {
                    return;
                };
                if stream.write_all(&picture).await.is_err() {
                    return;
                }
                let _ = stream.finish();
            }
        });
        Self {
            queue,
            connection,
            waiting_for_keyframe: true,
        }
    }
}

async fn forward(conn: &quinn::Connection, id: PeerId, shared: &Shared) -> String {
    let mut watchers: HashMap<PeerId, Watcher> = HashMap::new();
    loop {
        let mut stream = match conn.accept_uni().await {
            Ok(stream) => stream,
            Err(e) => return e.to_string(),
        };

        let Ok(mut picture) = stream.read_to_end(HEADER_LEN + MAX_PICTURE).await else {
            continue;
        };
        let Ok((header, _)) = PictureHeader::parse(&picture) else {
            continue;
        };
        if stamp_sharer(&mut picture, id).is_err() {
            continue;
        }
        let picture = Bytes::from(picture);

        let targets = shared.read(|hub| hub.picture_targets(id));
        watchers.retain(|watcher, way| {
            !way.queue.is_closed()
                && targets
                    .iter()
                    .any(|(id, media)| id == watcher && media.stable_id() == way.connection)
        });
        let mut keyframe_needed = false;
        for (watcher, media) in targets {
            let way = watchers
                .entry(watcher)
                .or_insert_with(|| Watcher::new(media));
            if way.waiting_for_keyframe && !header.keyframe {
                keyframe_needed = true;
                continue;
            }
            way.waiting_for_keyframe = way.queue.try_send(picture.clone()).is_err();
            keyframe_needed |= way.waiting_for_keyframe;
        }
        if keyframe_needed {
            shared.read(|hub| hub.keyframe_wanted(id));
        }
    }
}
