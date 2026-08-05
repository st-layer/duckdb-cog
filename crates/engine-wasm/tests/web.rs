//! wasm 사이드카 패리티 판정 (#66): 같은 engine 코드가 같은 입력에 같은 출력을
//! 내는지 헤드리스 브라우저에서 확인한다. zonal 골든 상수는 네이티브
//! `crates/engine/tests/zonal_batch.rs` 와 동일해야 한다 (bit-exact).
#![cfg(target_arch = "wasm32")]

use engine_wasm::{cog_meta_from_bytes, zonal_stats_from_bytes};
use wasm_bindgen::JsValue;
use wasm_bindgen_futures::JsFuture;
use wasm_bindgen_test::*;

wasm_bindgen_test_configure!(run_in_browser);

/// 컴파일 타임 임베드 — 파일이 없으면 컴파일 에러: `just fixtures` 로 생성.
const FIXTURE: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../test/data/generated/basic_512x512_u16.tif"
));
/// STATISTICS_* 태그 재료 (§6.7) — band_stats 패리티 판정용.
const STATS_FIXTURE: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../test/data/generated/stats_64x64_u16.tif"
));

/// zonal_batch.rs 오라클 폴리곤 3본 (P1 오목+구멍 / P2 MULTIPOLYGON / P3 삼각형).
const P1: &str = "POLYGON ((301203.7 3995803.7, 304003.7 3995803.7, 304003.7 3998803.7, \
     303203.7 3998803.7, 303203.7 3996803.7, 302203.7 3996803.7, \
     302203.7 3998803.7, 301203.7 3998803.7, 301203.7 3995803.7), \
     (303403.7 3996003.7, 303803.7 3996003.7, 303803.7 3996403.7, \
     303403.7 3996403.7, 303403.7 3996003.7))";
const P2: &str = "MULTIPOLYGON (((301003.7 3999003.7, 301503.7 3999003.7, 301503.7 3999503.7, \
     301003.7 3999503.7, 301003.7 3999003.7)), \
     ((303003.7 3999003.7, 303503.7 3999003.7, 303503.7 3999503.7, \
     303003.7 3999503.7, 303003.7 3999003.7)))";
const P3: &str = "POLYGON ((300203.7 3999803.7, 301403.7 3999803.7, 300203.7 3998593.1, \
     300203.7 3999803.7))";

fn fixture() -> js_sys::Uint8Array {
    js_sys::Uint8Array::from(FIXTURE)
}

async fn zonal(wkt: &str, band: u32, stat: &str) -> JsValue {
    JsFuture::from(zonal_stats_from_bytes(
        fixture(),
        wkt.to_string(),
        band,
        stat.to_string(),
    ))
    .await
    .expect("resolve")
}

#[wasm_bindgen_test]
async fn zonal_stats_match_native_goldens() {
    // (count, sum) 골든: zonal_batch.rs 와 동일 상수 — 패리티의 핵심 판정
    for (wkt, count, sum) in [
        (P1, 62_400.0, 2_043_359_680.0),
        (P2, 5_000.0, 165_209_622.0),
        (P3, 7_267.0, 239_110_784.0),
    ] {
        assert_eq!(zonal(wkt, 1, "count").await.as_f64(), Some(count));
        assert_eq!(zonal(wkt, 1, "sum").await.as_f64(), Some(sum));
        assert_eq!(zonal(wkt, 1, "mean").await.as_f64(), Some(sum / count));
    }
}

#[wasm_bindgen_test]
async fn out_of_range_band_yields_count_zero_and_nulls() {
    // G11: 범위 밖 밴드 → 빈 집계 (count → 0, 나머지 → null)
    assert_eq!(zonal(P3, 99, "count").await.as_f64(), Some(0.0));
    assert!(zonal(P3, 99, "sum").await.is_null());
    assert!(zonal(P3, 99, "mean").await.is_null());
}

#[wasm_bindgen_test]
async fn unknown_stat_and_bad_wkt_reject() {
    let err = JsFuture::from(zonal_stats_from_bytes(
        fixture(),
        P3.to_string(),
        1,
        "median".to_string(),
    ))
    .await
    .expect_err("reject");
    let msg = js_sys::Error::from(err).message().as_string().unwrap();
    assert!(msg.contains("unknown stat 'median'"), "{msg}");

    JsFuture::from(zonal_stats_from_bytes(
        fixture(),
        "POINT (1 2)".to_string(),
        1,
        "mean".to_string(),
    ))
    .await
    .expect_err("zone 은 POLYGON/MULTIPOLYGON 만");
}

#[wasm_bindgen_test]
async fn cog_meta_matches_fixture() {
    // gen_fixtures.py basic_512x512_u16: EPSG:32652, 10m, origin (300000, 4000000),
    // nodata 0, 타일 256, 1밴드
    let meta = JsFuture::from(cog_meta_from_bytes(fixture()))
        .await
        .expect("resolve");
    let get = |k: &str| js_sys::Reflect::get(&meta, &JsValue::from_str(k)).unwrap();

    assert_eq!(get("width").as_f64(), Some(512.0));
    assert_eq!(get("height").as_f64(), Some(512.0));
    assert_eq!(get("numBands").as_f64(), Some(1.0));
    assert_eq!(get("srid").as_f64(), Some(32652.0));
    assert_eq!(get("nodata").as_f64(), Some(0.0));

    // GDAL 순서 (RFC §6.8): scaleX, skewY, skewX, scaleY, upperLeftX, upperLeftY
    let gt = js_sys::Array::from(&get("geotransform"));
    let gtv: Vec<f64> = gt.iter().map(|v| v.as_f64().unwrap()).collect();
    assert_eq!(gtv, [10.0, 0.0, 0.0, -10.0, 300_000.0, 4_000_000.0]);

    let levels = js_sys::Array::from(&get("levels"));
    assert!(levels.length() >= 1, "본체 레벨은 항상 존재");
    let l0 = levels.get(0);
    let lget = |k: &str| js_sys::Reflect::get(&l0, &JsValue::from_str(k)).unwrap();
    assert_eq!(lget("width").as_f64(), Some(512.0));
    assert_eq!(lget("tileWidth").as_f64(), Some(256.0));
    assert_eq!(lget("tileHeight").as_f64(), Some(256.0));

    // basic 픽스처는 STATISTICS_* 태그가 없다 → null (pixel_value.rs 계약과 동일)
    assert!(get("bandStats").is_null());
}

#[wasm_bindgen_test]
async fn band_stats_match_native_goldens() {
    // pixel_value.rs::gdal_metadata_statistics_are_mapped 와 동일 상수 (bit-exact)
    let meta = JsFuture::from(cog_meta_from_bytes(js_sys::Uint8Array::from(STATS_FIXTURE)))
        .await
        .expect("resolve");
    let bands =
        js_sys::Array::from(&js_sys::Reflect::get(&meta, &JsValue::from_str("bandStats")).unwrap());
    assert_eq!(bands.length(), 1);
    let b0 = bands.get(0);
    let get = |k: &str| {
        js_sys::Reflect::get(&b0, &JsValue::from_str(k))
            .unwrap()
            .as_f64()
    };
    assert_eq!(get("min"), Some(33.0));
    assert_eq!(get("max"), Some(65_477.0));
    assert_eq!(get("mean"), Some(32_939.121_338));
    assert_eq!(get("stddev"), Some(18_924.488_017));
}
