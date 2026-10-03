use std::sync::atomic::Ordering;

use whisperia::{audio::fake::FakeBackend, net::SessionEvent};
use whisperia_e2e::{
    net::{enter, join, joining, next_event_where, send_state, Client},
    signal::{is_silent, is_tone, last, rms, tone_level},
    TestServer,
};
use whisperia_proto::control::VoiceState;

use crate::{played_for, serially, until};

fn received_from_the_first(client: &Client) -> u64 {
    client
        .engine
        .telemetry()
        .peers()
        .first()
        .map_or(0, |heard| heard.jitter.received)
}

#[test]
fn a_voice_crosses_sample_rates_intact() {
    serially(async {
        let server = TestServer::start("DESKTOP-AV83");
        let panda = joining(server.addr(), "Panda")
            .audio(FakeBackend::new(44_100, Some((440.0, 0.4))))
            .join()
            .await;
        let raccoon = join(server.addr(), "Raccoon").await;

        until("a second of voice each way", || {
            received_from_the_first(&raccoon) > 250 && received_from_the_first(&panda) > 250
        })
        .await;
        let heard = played_for(&raccoon.audio, 1.0).await;
        assert!(is_tone(&heard, 48_000, 440.0), "not the 440 Hz sent");
        let level = tone_level(&heard, 48_000, 440.0);
        assert!(
            (level - 0.4).abs() < 0.06,
            "amplitude {level} instead of 0.4"
        );

        let raccoon_hears = raccoon.engine.telemetry().peers();
        assert_eq!(raccoon_hears.len(), 1);
        assert!(raccoon_hears[0].talking);
        let jitter = raccoon_hears[0].jitter;
        assert!(
            jitter.lost * 100 <= jitter.received,
            "more than one in a hundred lost: {jitter:?}"
        );

        let panda_heard = last(&panda.audio, 44_100);
        assert!(is_silent(&panda_heard), "Raccoon's silence is heard");
    });
}

#[test]
fn muting_silences_and_tells_the_others() {
    serially(async {
        let server = TestServer::start("DESKTOP-AV83");
        let panda = joining(server.addr(), "Panda")
            .audio(FakeBackend::quiet(48_000, Some((440.0, 0.4))))
            .join()
            .await;
        let mut raccoon = join(server.addr(), "Raccoon").await;
        until("Raccoon hearing Panda", || {
            is_tone(&last(&raccoon.audio, 9_600), 48_000, 440.0)
        })
        .await;

        panda.controls.muted.store(true, Ordering::Relaxed);
        send_state(
            &panda.session,
            VoiceState {
                muted: true,
                deafened: false,
            },
        );
        let told = next_event_where(&mut raccoon.events, "news of the mute", |event| {
            matches!(event, SessionEvent::PeerState { .. })
        })
        .await;
        assert!(matches!(
            told,
            SessionEvent::PeerState {
                state: VoiceState { muted: true, .. },
                ..
            }
        ));
        until("the voice going silent", || {
            is_silent(&last(&raccoon.audio, 4_800))
        })
        .await;
        let after = played_for(&raccoon.audio, 0.5).await;
        assert!(
            is_silent(&after),
            "still audible after mute: {}",
            rms(&after)
        );
    });
}

#[test]
fn one_talker_can_be_turned_down_without_touching_the_others() {
    serially(async {
        let server = TestServer::start("DESKTOP-AV83");
        let _raccoon = joining(server.addr(), "Raccoon")
            .audio(FakeBackend::quiet(48_000, Some((440.0, 0.3))))
            .join()
            .await;
        let _fox = joining(server.addr(), "Fox")
            .audio(FakeBackend::quiet(48_000, Some((1500.0, 0.3))))
            .join()
            .await;
        let panda = join(server.addr(), "Panda").await;
        let raccoon_id = panda
            .peers
            .iter()
            .find(|p| p.name == "Raccoon")
            .expect("Raccoon is in the room")
            .id;
        assert_eq!(panda.peers.len(), 2);

        let levels = |samples: &[f32]| {
            (
                tone_level(samples, 48_000, 440.0),
                tone_level(samples, 48_000, 1500.0),
            )
        };
        until("both talkers heard", || {
            let (raccoon, fox) = levels(&last(&panda.audio, 24_000));
            raccoon > 0.2 && fox > 0.2
        })
        .await;
        let (raccoon_before, fox_before) = levels(&played_for(&panda.audio, 0.5).await);
        assert_eq!(panda.engine.telemetry().peers().len(), 2, "both are heard");

        panda.controls.set_volumes(&[(raccoon_id, -20.0)], &[]);
        until("Raccoon turned down", || {
            levels(&last(&panda.audio, 24_000)).0 < raccoon_before * 0.2
        })
        .await;
        let (raccoon_down, fox_meanwhile) = levels(&played_for(&panda.audio, 0.5).await);
        assert!(
            (raccoon_down / raccoon_before - 0.1).abs() < 0.05,
            "-20 dB is a tenth: {raccoon_before} -> {raccoon_down}"
        );
        assert!(
            (fox_meanwhile - fox_before).abs() < fox_before * 0.2,
            "Fox changed too: {fox_before} -> {fox_meanwhile}"
        );
        let heard = panda.engine.telemetry().peers();
        let fox = heard.iter().find(|p| p.id != raccoon_id).expect("Fox");
        assert!(fox.talking, "the other talker went quiet too");
        let raccoon = heard.iter().find(|p| p.id == raccoon_id).expect("Raccoon");
        assert!(
            raccoon.talking,
            "a turned-down talker must still show as talking"
        );

        panda.controls.set_volumes(&[], &[]);
        until("Raccoon back", || {
            let (raccoon, _) = levels(&last(&panda.audio, 24_000));
            (raccoon - raccoon_before).abs() < raccoon_before * 0.2
        })
        .await;
    });
}

#[test]
fn rooms_keep_voices_apart() {
    serially(async {
        let server = TestServer::start("DESKTOP-AV83");
        let _raccoon = joining(server.addr(), "Raccoon")
            .audio(FakeBackend::quiet(48_000, Some((440.0, 0.4))))
            .join()
            .await;
        let mut panda = join(server.addr(), "Panda").await;
        until("Panda hearing Raccoon", || {
            is_tone(&last(&panda.audio, 9_600), 48_000, 440.0)
        })
        .await;

        enter(&panda.session, "Room 83");
        next_event_where(&mut panda.events, "Room 83 to be joined", |event| {
            matches!(event, SessionEvent::Joined { .. })
        })
        .await;
        until("the other room going quiet", || {
            is_silent(&last(&panda.audio, 4_800))
        })
        .await;
        let next_door = played_for(&panda.audio, 0.7).await;
        assert!(is_silent(&next_door), "still hearing the other room");
    });
}
