mod cues;
mod mic_test;
mod share_sound;
mod timing;
mod voice;

use std::{
    future::Future,
    sync::{Mutex, MutexGuard},
    time::{Duration, Instant},
};

use whisperia::audio::fake::FakeBackend;
use whisperia_e2e::signal;

static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());

const PATIENCE: Duration = Duration::from_secs(15);

fn one_at_a_time() -> MutexGuard<'static, ()> {
    ONE_AT_A_TIME.lock().unwrap_or_else(|p| p.into_inner())
}

fn serially(test: impl Future<Output = ()>) {
    let _one_at_a_time = one_at_a_time();
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("a runtime")
        .block_on(test);
}

async fn until(what: &str, mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + PATIENCE;
    while !done() {
        assert!(Instant::now() < deadline, "{what} never happened");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

fn blocking_until(what: &str, mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + PATIENCE;
    while !done() {
        assert!(Instant::now() < deadline, "{what} never happened");
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn blocking_played_for(speaker: &FakeBackend, seconds: f32) -> Vec<f32> {
    let from = signal::played(speaker);
    let samples = (seconds * speaker.sample_rate as f32) as usize;
    blocking_until(&format!("{seconds} s of playback"), || {
        signal::played(speaker) >= from + samples
    });
    signal::since(speaker, from)
}

async fn played_for(speaker: &FakeBackend, seconds: f32) -> Vec<f32> {
    let from = signal::played(speaker);
    let samples = (seconds * speaker.sample_rate as f32) as usize;
    until(&format!("{seconds} s of playback"), || {
        signal::played(speaker) >= from + samples
    })
    .await;
    signal::since(speaker, from)
}
