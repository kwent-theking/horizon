// Offline regression: a captured 767 chunk body must pass through the
// translator and decode natively as a 777 LevelChunkWithLight packet.
// The fixture lives in %TEMP%\horizon-chunk-767-0.bin (written by the
// debug capture during a live session); the test is skipped when the
// file is absent so CI without the fixture still passes.
// / RU: офлайн-регрессия: захваченное тело чанка 767 должно пройти
// транслятор и декодироваться нативно как пакет 777 LevelChunkWithLight.
// Фикстура лежит в %TEMP%\horizon-chunk-767-0.bin (снимок отладочного
// захвата живой сессии); без файла тест пропускается, чтобы CI не падал.

use horizon_protocol::ids::play::clientbound::LEVEL_CHUNK_WITH_LIGHT;
use horizon_protocol::packets::play::Clientbound;
use horizon_protocol::translation::p767;

#[test]
fn captured_767_chunk_decodes_as_777() {
    let path = std::env::temp_dir().join("horizon-chunk-767-0.bin");
    let Ok(body) = std::fs::read(&path) else {
        eprintln!("skipping: {} not present", path.display());
        return;
    };
    let t = p767::translation();
    eprintln!("body 767: {} bytes", body.len());
    let Some(translated) = (t.ingress_body)(LEVEL_CHUNK_WITH_LIGHT, &body) else {
        panic!("ingress_body returned None for the captured chunk");
    };
    eprintln!("translated 777: {} bytes", translated.len());
    let _ = std::fs::write(
        std::env::temp_dir().join("horizon-chunk-777-translated.bin"),
        &translated,
    );
    let packet = Clientbound::decode(LEVEL_CHUNK_WITH_LIGHT, &translated)
        .expect("native 777 decode");
    // The captured fixture is chunk (0, 0).
    let Clientbound::LevelChunkWithLight(p) = packet else {
        panic!("wrong packet variant");
    };
    assert_eq!((p.x, p.z), (0, 0));
}
