const MAIN_SOURCE: &str = include_str!("../src/main.rs");

#[test]
fn live_media_queue_never_awaits_uncached_synthesis() {
    let start = MAIN_SOURCE
        .find("async fn play_media_queue")
        .expect("play_media_queue must exist");
    let end = MAIN_SOURCE[start..]
        .find("fn runtime_voice_cache")
        .map(|offset| start + offset)
        .expect("runtime_voice_cache must follow play_media_queue");
    let live_path = &MAIN_SOURCE[start..end];

    assert!(
        live_path.contains("voice.cached(text).await"),
        "live call playback must use a cache-only lookup"
    );
    assert!(
        !live_path.contains("voice.render(text).await"),
        "live call playback must not await Kokoro synthesis"
    );
}

#[test]
fn startup_warms_static_speech_before_starting_ari() {
    let warm = MAIN_SOURCE
        .find("warm_static_speech(")
        .expect("startup must warm static speech");
    let ari = MAIN_SOURCE
        .find("AriClient::with_config")
        .expect("ARI startup must exist");

    assert!(warm < ari, "static speech must be ready before ARI starts accepting calls");
}
