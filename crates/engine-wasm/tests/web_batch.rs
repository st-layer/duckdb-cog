//! 배치·캐시 계약 (#66 슬라이스 3): 씬축(1 zone × N scenes)과 zone축(N zones
//! × 1 scene) 배치가 스칼라/네이티브 골든과 동일한지, 리더 재사용 + TileCache
//! 가 관측 가능하게 동작하는지 판정한다. Range+CORS 서버(포트 18925) 경유 —
//! 골든은 전부 기존 네이티브 상수 (zonal_batch.rs), 새 수치 발명 없음.
#![cfg(target_arch = "wasm32")]

use engine_wasm::{
    configure_tile_cache, tile_cache_stats, zonal_stats, zonal_stats_batch, zonal_stats_batch_zones,
};
use wasm_bindgen::JsValue;
use wasm_bindgen_futures::JsFuture;
use wasm_bindgen_test::*;

wasm_bindgen_test_configure!(run_in_browser);

/// justfile wasm-test 레시피와의 포트 계약 (web_fetch.rs 와 동일).
const BASE: &str = "http://127.0.0.1:18925";

fn url(name: &str) -> String {
    format!("{BASE}/{name}")
}

/// zonal_batch.rs 오라클 폴리곤 3본.
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

fn str_array(items: &[String]) -> js_sys::Array {
    items.iter().map(|s| JsValue::from_str(s)).collect()
}

fn to_f64s(v: JsValue) -> Vec<Option<f64>> {
    js_sys::Array::from(&v).iter().map(|x| x.as_f64()).collect()
}

#[wasm_bindgen_test]
async fn scene_axis_batch_matches_scalar_goldens() {
    // 같은 씬 3회 — dedupe 되어도 결과는 입력 순서대로 3개
    let urls = str_array(&[
        url("basic_512x512_u16.tif"),
        url("basic_512x512_u16.tif"),
        url("basic_512x512_u16.tif"),
    ]);
    let v = JsFuture::from(zonal_stats_batch(
        urls,
        P1.to_string(),
        1,
        "count".to_string(),
    ))
    .await
    .expect("resolve");
    assert_eq!(to_f64s(v), vec![Some(62_400.0); 3]);
}

#[wasm_bindgen_test]
async fn scene_axis_batch_preserves_input_order_across_scenes() {
    // stats_64x64 (origin 950000/4000000) 는 P1 과 비교차 → count 0
    let urls = str_array(&[url("basic_512x512_u16.tif"), url("stats_64x64_u16.tif")]);
    let v = JsFuture::from(zonal_stats_batch(
        urls,
        P1.to_string(),
        1,
        "count".to_string(),
    ))
    .await
    .expect("resolve");
    assert_eq!(to_f64s(v), vec![Some(62_400.0), Some(0.0)]);
}

#[wasm_bindgen_test]
async fn scene_axis_batch_empty_input_resolves_empty() {
    let v = JsFuture::from(zonal_stats_batch(
        js_sys::Array::new(),
        P1.to_string(),
        1,
        "count".to_string(),
    ))
    .await
    .expect("resolve");
    assert_eq!(js_sys::Array::from(&v).length(), 0);
}

#[wasm_bindgen_test]
async fn zone_axis_batch_matches_native_goldens() {
    // #62 zonal_stats_polygon_batch 골든 (sum) — 타일 union 1회 fetch 경로
    let v = JsFuture::from(zonal_stats_batch_zones(
        url("basic_512x512_u16.tif"),
        str_array(&[P1.to_string(), P2.to_string(), P3.to_string()]),
        1,
        "sum".to_string(),
    ))
    .await
    .expect("resolve");
    assert_eq!(
        to_f64s(v),
        vec![
            Some(2_043_359_680.0),
            Some(165_209_622.0),
            Some(239_110_784.0)
        ]
    );
}

#[wasm_bindgen_test]
async fn batch_with_missing_url_rejects_whole_promise() {
    // IO 실패는 무음 null 이 아니라 전체 reject (#58 무음 손실 금지) — null 은
    // G11 의미로만 쓴다
    let urls = str_array(&[url("basic_512x512_u16.tif"), url("does_not_exist.tif")]);
    let err = JsFuture::from(zonal_stats_batch(
        urls,
        P1.to_string(),
        1,
        "count".to_string(),
    ))
    .await
    .expect_err("reject");
    let msg = js_sys::Error::from(err).message().as_string().unwrap();
    assert!(msg.contains("does_not_exist.tif"), "{msg}");
}

#[wasm_bindgen_test]
async fn repeat_calls_hit_tile_cache_via_reader_reuse() {
    // configureTileCache 는 캐시 교체 + 리더 레지스트리 clear — 콜드 시작 보장
    configure_tile_cache(64);
    let cold = JsFuture::from(zonal_stats(
        url("basic_512x512_u16.tif"),
        P3.to_string(),
        1,
        "count".to_string(),
    ))
    .await
    .expect("resolve");
    assert_eq!(cold.as_f64(), Some(7_267.0));
    let s1 = tile_cache_stats();
    let hits1 = js_sys::Reflect::get(&s1, &JsValue::from_str("hits"))
        .unwrap()
        .as_f64()
        .unwrap();

    // 같은 URL·같은 zone 재호출 — 리더 재사용(같은 ReaderId)이라 캐시 적중
    let warm = JsFuture::from(zonal_stats(
        url("basic_512x512_u16.tif"),
        P3.to_string(),
        1,
        "count".to_string(),
    ))
    .await
    .expect("resolve");
    assert_eq!(warm.as_f64(), Some(7_267.0));
    let s2 = tile_cache_stats();
    let hits2 = js_sys::Reflect::get(&s2, &JsValue::from_str("hits"))
        .unwrap()
        .as_f64()
        .unwrap();
    assert!(
        hits2 > hits1,
        "웜 호출이 캐시를 적중해야 함: {hits1} → {hits2}"
    );

    let max = js_sys::Reflect::get(&s2, &JsValue::from_str("maxBytes"))
        .unwrap()
        .as_f64()
        .unwrap();
    assert_eq!(max, 64.0 * 1024.0 * 1024.0);
}
