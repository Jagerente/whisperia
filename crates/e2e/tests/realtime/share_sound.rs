use whisperia::net::SessionEvent;
use whisperia_e2e::{
    net::{
        answer_keyframes, joining, member_of, next_event_where, send_pictures_and_sound,
        send_state, share_start, share_stop, unwatch, wait_for_pictures, watch, Client, Member,
        Pictures, SharerHears, SHARE_TONE,
    },
    signal::{is_silent, is_tone, last, rms, tone_level},
    TestServer,
};
use whisperia_proto::{control::VoiceState, PeerId};

use crate::{played_for, serially, until};

struct Share {
    panda: Member,
    pictures: Pictures,
    sharer: SharerHears,
}

async fn share(server: &TestServer, sound: bool) -> Share {
    let mut panda = member_of(server.addr(), "Panda", "Backroom").await;
    share_start(&panda.session, sound);
    let pictures = send_pictures_and_sound(&panda.session);
    let events = std::mem::replace(&mut panda.events, tokio::sync::mpsc::unbounded_channel().1);
    let sharer = answer_keyframes(events, &pictures);
    Share {
        panda,
        pictures,
        sharer,
    }
}

async fn in_backroom(server: &TestServer, name: &str, sharer: PeerId) -> Client {
    let mut client = joining(server.addr(), name).room("Backroom").join().await;
    let already = client
        .peers
        .iter()
        .any(|p| p.id == sharer && p.share.is_some());
    if !already {
        next_event_where(&mut client.events, "news of the share", |event| {
            matches!(event, SessionEvent::PeerShare { id, share: Some(_) } if *id == sharer)
        })
        .await;
    }
    client
}

fn hears_the_share(client: &Client) -> bool {
    is_tone(&last(&client.audio, 24_000), 48_000, SHARE_TONE)
}

#[test]
fn share_sound_reaches_only_those_who_watch() {
    serially(async {
        let server = TestServer::start("DESKTOP-AV83");
        let share = share(&server, true).await;
        let id = share.panda.id;
        let raccoon = in_backroom(&server, "Raccoon", id).await;
        let fox = in_backroom(&server, "Fox", id).await;
        let penguin = joining(server.addr(), "Penguin")
            .room("Room 83")
            .join()
            .await;

        let _seen = watch(&raccoon.session, id);
        until("Raccoon hearing the share", || hears_the_share(&raccoon)).await;
        let heard = played_for(&raccoon.audio, 1.0).await;
        assert!(rms(&heard) > 0.1, "level {}", rms(&heard));
        let shares = raccoon.engine.telemetry().shares();
        assert_eq!(shares.len(), 1);
        assert_eq!(shares[0].id, id);
        assert!(is_silent(&last(&fox.audio, 48_000)), "Fox does not watch");
        assert!(
            is_silent(&last(&penguin.audio, 48_000)),
            "Penguin is next door"
        );
        assert!(fox.engine.telemetry().shares().is_empty());

        unwatch(&raccoon.session, id);
        share.sharer.wait_for_viewers(&[]).await;
        until("the share going quiet", || {
            is_silent(&last(&raccoon.audio, 4_800))
        })
        .await;
        drop(share.pictures);
    });
}

#[test]
fn the_screen_volume_turns_the_share_sound_down() {
    serially(async {
        let server = TestServer::start("DESKTOP-AV83");
        let share = share(&server, true).await;
        let id = share.panda.id;
        let raccoon = in_backroom(&server, "Raccoon", id).await;
        let _seen = watch(&raccoon.session, id);
        until("Raccoon hearing the share", || hears_the_share(&raccoon)).await;
        let full = tone_level(&played_for(&raccoon.audio, 1.0).await, 48_000, SHARE_TONE);

        raccoon.controls.set_volumes(&[], &[(id, -20.0)]);
        let level = || tone_level(&last(&raccoon.audio, 24_000), 48_000, SHARE_TONE);
        until("the share turned down", || level() < full * 0.2).await;
        let quieter = tone_level(&played_for(&raccoon.audio, 1.0).await, 48_000, SHARE_TONE);
        assert!(
            (quieter / full - 0.1).abs() < 0.03,
            "-20 dB is a tenth: {quieter} after {full}"
        );

        raccoon.controls.set_volumes(&[], &[(id, -30.0)]);
        until("the bottom of the volume being silence", || {
            rms(&last(&raccoon.audio, 24_000)) < 1e-4
        })
        .await;
        drop(share.pictures);
    });
}

#[test]
fn muting_the_voice_leaves_the_share_sound() {
    serially(async {
        let server = TestServer::start("DESKTOP-AV83");
        let share = share(&server, true).await;
        let id = share.panda.id;
        let mut raccoon = in_backroom(&server, "Raccoon", id).await;
        let _seen = watch(&raccoon.session, id);
        until("Raccoon hearing the share", || hears_the_share(&raccoon)).await;

        send_state(
            &share.panda.session,
            VoiceState {
                muted: true,
                deafened: false,
            },
        );
        next_event_where(
            &mut raccoon.events,
            "news of the mute",
            |event| matches!(event, SessionEvent::PeerState { id: who, .. } if *who == id),
        )
        .await;
        let after = played_for(&raccoon.audio, 1.0).await;
        assert!(
            is_tone(&after, 48_000, SHARE_TONE),
            "the share went quiet with the voice"
        );
        drop(share.pictures);
    });
}

#[test]
fn a_stopped_share_leaves_the_mix() {
    serially(async {
        let server = TestServer::start("DESKTOP-AV83");
        let share = share(&server, true).await;
        let id = share.panda.id;
        let mut raccoon = in_backroom(&server, "Raccoon", id).await;
        let _seen = watch(&raccoon.session, id);
        until("Raccoon hearing the share", || hears_the_share(&raccoon)).await;
        assert_eq!(raccoon.engine.telemetry().shares().len(), 1);

        drop(share.pictures);
        share_stop(&share.panda.session);
        next_event_where(
            &mut raccoon.events,
            "the end of the share",
            |event| matches!(event, SessionEvent::PeerShare { id: who, share: None } if *who == id),
        )
        .await;
        raccoon.engine.forget_share(id);
        until("the share leaving the mix", || {
            raccoon.engine.telemetry().shares().is_empty()
        })
        .await;
    });
}

#[test]
fn a_share_without_sound_is_silent() {
    serially(async {
        let server = TestServer::start("DESKTOP-AV83");
        let share = share(&server, false).await;
        let id = share.panda.id;
        let raccoon = in_backroom(&server, "Raccoon", id).await;
        let seen = watch(&raccoon.session, id);
        wait_for_pictures(&seen, 10).await;
        let watched = played_for(&raccoon.audio, 1.2).await;
        assert!(is_silent(&watched), "level {}", rms(&watched));
        drop(share.pictures);
    });
}
