//! envelope_window (#76): 창 좌표의 단일 소스 — `band_window` 가 실제로 읽는
//! 창과 모양이 일치해야 한다. wasm 바인딩이 width/height/bbox 를 이 헬퍼로
//! 얻으므로, 픽셀 중심 반올림 규약이 두 곳에서 갈라지면 여기서 잡힌다.

use engine::{envelope_window, open_cog, MemorySource};

fn fixture_bytes(name: &str) -> Vec<u8> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../test/data/generated")
        .join(name);
    std::fs::read(&path)
        .unwrap_or_else(|_| panic!("픽스처 없음: {} — `just fixtures` 로 생성", path.display()))
}

fn block_on<F: std::future::Future>(f: F) -> F::Output {
    engine::futures::executor::block_on(f)
}

#[test]
fn envelope_window_matches_band_window_shape() {
    let (meta, reader) = block_on(open_cog(MemorySource::new(fixture_bytes(
        "basic_512x512_u16.tif",
    ))))
    .expect("valid COG");

    // P3 삼각형(zonal_batch.rs)의 envelope — 손계산: cols 20..=139, rows 20..=140
    let bbox = [300203.7, 3998593.1, 301403.7, 3999803.7];
    let (c0, c1, r0, r1) = envelope_window(&meta, bbox).expect("이미지와 교차");
    assert_eq!((c0, c1, r0, r1), (20, 139, 20, 140));

    // band_window 가 실제로 돌려주는 배열 길이 = 같은 창의 넓이
    let win = block_on(reader.band_window(&meta, Some(bbox), 1))
        .expect("io ok")
        .expect("유효 밴드");
    assert_eq!(win.len(), ((c1 - c0 + 1) * (r1 - r0 + 1)) as usize);

    // 비교차 bbox → None (band_window 의 빈 배열과 같은 결)
    assert!(envelope_window(&meta, [0.0, 0.0, 1.0, 1.0]).is_none());
}
