use std::{
    sync::{atomic::Ordering, Arc},
    time::{Duration, Instant},
};

use whisperia::{
    audio::fake::FakeBackend,
    dsp::{cue::Cue, denoise::DenoiseMode},
    engine::{Controls, Engine},
    wake::Waker,
};
use whisperia_e2e::{
    net::{join, joining},
    signal::{is_silent, peak_after, played},
    TestServer,
};

use crate::{blocking_until, one_at_a_time, played_for, serially};

const CUE_PEAK: std::ops::Range<f32> = 0.03..0.2;

#[test]
fn cues_play_while_deafened() {
    serially(async {
        let server = TestServer::start("DESKTOP-AV83");
        let _raccoon = joining(server.addr(), "Raccoon")
            .audio(FakeBackend::quiet(48_000, None))
            .join()
            .await;
        let mut panda = join(server.addr(), "Panda").await;

        panda.controls.deafened.store(true, Ordering::Relaxed);
        let deafened = played_for(&panda.audio, 0.5).await;
        assert!(is_silent(&deafened));
        let before = played(&panda.audio);
        panda.engine.play(Cue::Join);
        let peak = peak_after(&panda.audio, before, Duration::from_secs(5));
        assert!(CUE_PEAK.contains(&peak), "cue peak {peak}");
    });
}

#[test]
fn a_sound_is_played_without_a_room_or_a_microphone_and_the_speakers_are_let_go() {
    let _one_at_a_time = one_at_a_time();
    let audio = FakeBackend::new(48_000, Some((440.0, 0.3)));
    let controls = Arc::new(Controls::new(DenoiseMode::Off, 0.0, 0.0, false));
    let mut engine = Engine::new(Arc::new(audio.clone()), controls, &Waker::default()).unwrap();
    assert_eq!(audio.speakers_playing.load(Ordering::Relaxed), 0);

    engine.play(Cue::Join);
    let peak = peak_after(&audio, 0, Duration::from_secs(5));
    assert!(CUE_PEAK.contains(&peak), "cue peak {peak}");
    assert_eq!(audio.microphones_opened.load(Ordering::Relaxed), 0);
    assert_eq!(engine.telemetry().frames_sent.load(Ordering::Relaxed), 0);

    engine.tick(Instant::now() + Duration::from_secs(10));
    blocking_until("the speakers let go", || {
        audio.speakers_playing.load(Ordering::Relaxed) == 0
    });
}
