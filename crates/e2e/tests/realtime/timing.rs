use std::{
    sync::atomic::Ordering,
    time::{Duration, Instant},
};

use whisperia::{audio::fake::FakeBackend, dsp::denoise::DenoiseMode};
use whisperia_e2e::{
    net::{join, joining},
    signal::{is_silent, played, rms, since},
    TestServer,
};

use crate::{played_for, serially, until};

#[test]
fn software_latency_fits_the_budget_with_noise_suppression_on() {
    serially(async {
        let server = TestServer::start("DESKTOP-AV83");
        let mic = FakeBackend::quiet(48_000, Some((300.0, 0.5)));
        mic.tone_on.store(false, Ordering::Relaxed);
        let panda = joining(server.addr(), "Panda")
            .audio(mic)
            .denoise(DenoiseMode::Strong)
            .join()
            .await;
        let raccoon = join(server.addr(), "Raccoon").await;
        until("half a second of voice arriving", || {
            raccoon
                .engine
                .telemetry()
                .peers()
                .first()
                .is_some_and(|heard| heard.jitter.received > 50)
        })
        .await;
        let settling = played_for(&raccoon.audio, 3.0).await;
        assert!(is_silent(&settling), "the microphone is not quiet yet");

        let mut measured = Vec::new();
        for _ in 0..7 {
            let already = played(&raccoon.audio);
            let onset_at = Instant::now();
            panda.audio.tone_on.store(true, Ordering::Relaxed);
            tokio::time::sleep(Duration::from_millis(400)).await;
            panda.audio.tone_on.store(false, Ordering::Relaxed);

            let heard = since(&raccoon.audio, already);
            let started = raccoon
                .audio
                .playback_started
                .lock()
                .unwrap()
                .expect("the speaker runs");
            if let Some(i) = heard.iter().position(|s| s.abs() > 0.02) {
                let due = started + Duration::from_secs_f64((already + i) as f64 / 48_000.0);
                measured.push(due.saturating_duration_since(onset_at).as_secs_f64() * 1000.0);
            }
            tokio::time::sleep(Duration::from_millis(400)).await;
        }

        assert!(measured.len() >= 5, "tone onsets got lost: {measured:?}");
        measured.sort_by(f64::total_cmp);
        let median = measured[measured.len() / 2];
        println!("microphone ring -> speaker ring, ms: {measured:.1?}, median {median:.1}");
        assert!(
            median < 75.0,
            "median software latency {median:.1} ms leaves no room for hardware"
        );
    });
}

#[test]
fn drifting_clocks_do_not_accumulate_latency_or_glitches() {
    serially(async {
        let server = TestServer::start("DESKTOP-AV83");
        let mut fast = FakeBackend::quiet(48_000, Some((440.0, 0.4)));
        fast.clock_skew_ppm = 3000.0;
        let _panda = joining(server.addr(), "Panda").audio(fast).join().await;
        let raccoon = join(server.addr(), "Raccoon").await;

        let heard = || raccoon.engine.telemetry().peers().first().copied();
        until("surplus audio being dropped", || {
            heard().is_some_and(|t| t.jitter.trimmed >= 1)
        })
        .await;
        let tail = played_for(&raccoon.audio, 5.0).await;

        let t = heard().expect("Panda is heard");
        assert!(
            t.depth_frames <= t.target_frames + 3,
            "latency crept up: {t:?}"
        );
        assert!(
            t.jitter.underruns <= 1 && t.jitter.lost <= 3,
            "{:?}",
            t.jitter
        );
        let quiet = tail.chunks(480).filter(|c| rms(c) < 0.1).count();
        assert!(quiet <= 2, "{quiet} silent frames");
    });
}

#[test]
fn the_delay_does_not_creep_up_while_muted() {
    serially(async {
        let server = TestServer::start("DESKTOP-AV83");
        let panda = joining(server.addr(), "Panda")
            .audio(FakeBackend::quiet(48_000, None))
            .join()
            .await;
        let worst_over = |seconds: u64| {
            let session = &panda.session;
            async move {
                let mut worst = Duration::ZERO;
                for _ in 0..seconds * 4 {
                    tokio::time::sleep(Duration::from_millis(250)).await;
                    worst = worst.max(session.rtt());
                }
                worst
            }
        };
        let talking = worst_over(4).await;
        panda.controls.muted.store(true, Ordering::Relaxed);
        let muted = worst_over(4).await;
        assert!(
            muted < talking + Duration::from_millis(5),
            "muted {muted:?} against {talking:?} while talking"
        );
    });
}
