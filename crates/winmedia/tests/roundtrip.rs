#![cfg(windows)]

use whisperia_winmedia::selftest;

#[test]
#[ignore = "needs a desktop session and a hardware H.264 encoder"]
fn the_screen_survives_the_trip_through_encoder_decoder_and_the_shared_textures() {
    let report = selftest::roundtrip(120, 16_000_000).expect("the video path works");
    println!("{report:#?}");
    assert!(report.frames >= 120, "{report:?}");
    assert!(report.keyframes >= 100, "keyframes on request: {report:?}");
    assert!(
        report.luma_psnr > 18.0,
        "what is shown is the screen: {report:?}"
    );
    assert!(
        report.luma_psnr > report.shifted_psnr + 1.0,
        "nothing moved or got scaled on the way: {report:?}"
    );
    assert!(
        report.colour_bias < 4.0,
        "colours and brightness come back as they went in: {report:?}"
    );
    assert!(
        report.keyframe_ms.0 < 40.0 && report.decode_ms.0 < 20.0,
        "low latency mode is on: {report:?}"
    );
}
