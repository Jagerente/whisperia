use std::sync::{atomic::Ordering, Arc};

use whisperia::{
    audio::fake::FakeBackend,
    dsp::denoise::DenoiseMode,
    engine::{Controls, Engine},
    wake::Waker,
};
use whisperia_e2e::{
    net::{joining, Client},
    signal::{is_silent, is_tone, last, rms, tone_level},
    TestServer,
};

use crate::{blocking_played_for, blocking_until, one_at_a_time, played_for, serially, until};

#[test]
fn the_microphone_test_plays_the_processed_microphone_back_without_a_server() {
    let _one_at_a_time = one_at_a_time();
    let audio = FakeBackend::new(44_100, Some((440.0, 0.3)));
    let controls = Arc::new(Controls::new(DenoiseMode::Off, 0.0, 0.0, false));
    let mut engine =
        Engine::new(Arc::new(audio.clone()), controls.clone(), &Waker::default()).unwrap();
    engine.test_microphone(true);

    blocking_until("the microphone coming back", || {
        rms(&last(&audio, 22_050)) > 0.15
    });
    let heard = blocking_played_for(&audio, 0.5);
    assert!(is_tone(&heard, 44_100, 440.0), "not the 440 Hz sung");
    let level = tone_level(&heard, 44_100, 440.0);
    assert!(
        (level - 0.3).abs() < 0.05,
        "amplitude {level} instead of 0.3"
    );
    assert!(engine.telemetry().mic_talking.load(Ordering::Relaxed));
    assert_eq!(engine.telemetry().frames_sent.load(Ordering::Relaxed), 0);

    controls.agc.store(true, Ordering::Relaxed);
    let level_db = || 20.0 * rms(&last(&audio, 22_050)).log10();
    blocking_until("automatic gain reaching -24 dBFS", || {
        (level_db() + 24.0).abs() < 1.0
    });
}

#[test]
fn a_microphone_test_in_a_room_is_for_ones_own_ears_alone() {
    serially(async {
        let server = TestServer::start("DESKTOP-AV83");
        let tone = |hz| FakeBackend::new(48_000, Some((hz, 0.4)));
        let raccoon = joining(server.addr(), "Raccoon")
            .audio(tone(440.0))
            .join()
            .await;
        let mut panda = joining(server.addr(), "Panda")
            .audio(tone(880.0))
            .join()
            .await;
        let hears = |client: &Client, hz: f32| is_tone(&last(&client.audio, 9_600), 48_000, hz);
        let hears_nothing = |client: &Client| is_silent(&last(&client.audio, 9_600));
        until("both hearing each other", || {
            hears(&raccoon, 880.0) && hears(&panda, 440.0)
        })
        .await;

        panda.engine.test_microphone(true);
        until("the room going quiet for the test", || {
            hears_nothing(&raccoon) && hears(&panda, 880.0)
        })
        .await;
        assert!(
            is_silent(&played_for(&raccoon.audio, 0.5).await),
            "Raccoon hears Panda's test"
        );
        assert_eq!(
            panda.audio.microphones_opened.load(Ordering::Relaxed),
            1,
            "the test uses the microphone already open"
        );

        panda.engine.test_microphone(false);
        until("the room coming back", || {
            hears(&raccoon, 880.0) && hears(&panda, 440.0)
        })
        .await;
    });
}
