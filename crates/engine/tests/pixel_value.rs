//! RS_Value 의 엔진 재료 계약: CogReader::read_pixel — 좌표 변환·타일 인덱싱·
//! 밴드 선택·NULL 규약. 기대 수치는 rasterio 오라클과 동일 (3중 대조의 엔진 축).

use engine::{open_cog, MemorySource};

fn fixture(name: &str) -> MemorySource {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../test/data/generated")
        .join(name);
    let raw = std::fs::read(&path)
        .unwrap_or_else(|_| panic!("픽스처 없음: {} — `just fixtures` 로 생성", path.display()));
    MemorySource::new(raw)
}

fn block_on<F: std::future::Future>(f: F) -> F::Output {
    engine::futures::executor::block_on(f)
}

#[test]
fn reads_known_pixels_matching_rasterio() {
    let (meta, reader) = block_on(open_cog(fixture("basic_512x512_u16.tif"))).expect("valid COG");
    let px = |x: f64, y: f64| block_on(reader.read_pixel(&meta, x, y, 1)).expect("io ok");
    assert_eq!(px(300005.0, 3999995.0), Some(9864.0)); // 픽셀 (0,0)
    assert_eq!(px(302565.0, 3997435.0), Some(23907.0)); // 타일 경계 넘은 (256,256)
    assert_eq!(px(305115.0, 3994885.0), Some(59212.0)); // 마지막 픽셀 (511,511)
                                                        // 원점 코너는 픽셀 (0,0), 우하단 경계 좌표는 밖 (floor 격자)
    assert_eq!(px(300000.0, 4000000.0), Some(9864.0));
    assert_eq!(px(305120.0, 3994880.0), None);
}

#[test]
fn multiband_bands_are_one_based() {
    let (meta, reader) = block_on(open_cog(fixture("multiband_64x64_u8.tif"))).expect("valid COG");
    let px = |b: u32| block_on(reader.read_pixel(&meta, 600325.0, 3899675.0, b)).expect("io ok");
    assert_eq!(px(1), Some(191.0));
    assert_eq!(px(2), Some(110.0));
    assert_eq!(px(3), Some(51.0));
    assert_eq!(px(0), None, "0 은 범위 밖 (1-based)");
    assert_eq!(px(4), None);
}

#[test]
fn outside_extent_is_none() {
    let (meta, reader) = block_on(open_cog(fixture("edge_400x300_u16.tif"))).expect("valid COG");
    let px = |x: f64, y: f64| block_on(reader.read_pixel(&meta, x, y, 1)).expect("io ok");
    assert_eq!(px(499999.9, 3799995.0), None);
    assert_eq!(px(504000.1, 3799995.0), None);
    assert_eq!(px(500005.0, 3800000.1), None);
    assert_eq!(px(500005.0, 3796999.9), None);
    // 클립된 마지막 픽셀은 안쪽
    assert_eq!(px(503995.0, 3797005.0), Some(22749.0));
}

#[test]
fn tiny_cog_pixel_read_via_readahead_clamp() {
    let (meta, reader) = block_on(open_cog(fixture("tiny_16x16_u8.tif"))).expect("valid COG");
    assert_eq!(
        block_on(reader.read_pixel(&meta, 700155.0, 3949845.0, 1)).expect("io ok"),
        Some(247.0)
    );
}

#[test]
fn nodata_maps_to_none() {
    // basic 은 nodata=0 이지만 데이터에 0 이 없다 (생성기가 1.. 로 뽑음) —
    // nodata 매핑은 순수 함수 계약으로 고정한다.
    assert_eq!(engine::apply_nodata(0.0, Some(0.0)), None);
    assert_eq!(engine::apply_nodata(5.0, Some(0.0)), Some(5.0));
    assert_eq!(engine::apply_nodata(0.0, None), Some(0.0));
    // NaN nodata: NaN 픽셀 → None
    assert_eq!(engine::apply_nodata(f64::NAN, Some(f64::NAN)), None);
    assert!(engine::apply_nodata(1.5, Some(f64::NAN)).is_some());
}
