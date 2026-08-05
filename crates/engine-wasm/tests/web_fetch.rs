//! fetch 경로 패리티 (#66 슬라이스 2): 원격(HTTP Range) 경로가 메모리 경로와
//! 같은 골든을 내는지 판정한다. justfile `wasm-test` 가 띄우는 Range+CORS
//! 서버(scripts/range_cors_server.py) 를 경유 — 실제 preflight(OPTIONS) 와
//! 206 Partial Content 를 브라우저에서 통과해야 green 이다.
#![cfg(target_arch = "wasm32")]

use engine_wasm::{cog_meta, zonal_stats};
use wasm_bindgen::JsValue;
use wasm_bindgen_futures::JsFuture;
use wasm_bindgen_test::*;

wasm_bindgen_test_configure!(run_in_browser);

/// justfile wasm-test 레시피와의 포트 계약.
const BASE: &str = "http://127.0.0.1:18925";

fn url(name: &str) -> String {
    format!("{BASE}/{name}")
}

/// zonal_batch.rs 오라클 폴리곤 (P1 오목+구멍 / P3 삼각형).
const P1: &str = "POLYGON ((301203.7 3995803.7, 304003.7 3995803.7, 304003.7 3998803.7, \
     303203.7 3998803.7, 303203.7 3996803.7, 302203.7 3996803.7, \
     302203.7 3998803.7, 301203.7 3998803.7, 301203.7 3995803.7), \
     (303403.7 3996003.7, 303803.7 3996003.7, 303803.7 3996403.7, \
     303403.7 3996403.7, 303403.7 3996003.7))";
const P3: &str = "POLYGON ((300203.7 3999803.7, 301403.7 3999803.7, 300203.7 3998593.1, \
     300203.7 3999803.7))";

async fn zonal(wkt: &str, band: u32, stat: &str) -> JsValue {
    JsFuture::from(zonal_stats(
        url("basic_512x512_u16.tif"),
        wkt.to_string(),
        band,
        stat.to_string(),
    ))
    .await
    .expect("resolve")
}

#[wasm_bindgen_test]
async fn remote_zonal_matches_native_goldens() {
    // 메모리 경로(web.rs)·네이티브(zonal_batch.rs)와 동일 상수 — 전송 계층만 다르다
    for (wkt, count, sum) in [
        (P1, 62_400.0, 2_043_359_680.0),
        (P3, 7_267.0, 239_110_784.0),
    ] {
        assert_eq!(zonal(wkt, 1, "count").await.as_f64(), Some(count));
        assert_eq!(zonal(wkt, 1, "sum").await.as_f64(), Some(sum));
        assert_eq!(zonal(wkt, 1, "mean").await.as_f64(), Some(sum / count));
    }
}

#[wasm_bindgen_test]
async fn remote_out_of_range_band_yields_count_zero_and_nulls() {
    // G11: 범위 밖 밴드 → 빈 집계 (count → 0, 나머지 → null)
    assert_eq!(zonal(P3, 99, "count").await.as_f64(), Some(0.0));
    assert!(zonal(P3, 99, "mean").await.is_null());
}

#[wasm_bindgen_test]
async fn remote_cog_meta_matches_fixture() {
    let meta = JsFuture::from(cog_meta(url("basic_512x512_u16.tif")))
        .await
        .expect("resolve");
    let get = |k: &str| js_sys::Reflect::get(&meta, &JsValue::from_str(k)).unwrap();
    assert_eq!(get("width").as_f64(), Some(512.0));
    assert_eq!(get("height").as_f64(), Some(512.0));
    assert_eq!(get("srid").as_f64(), Some(32652.0));
}

#[wasm_bindgen_test]
async fn missing_url_rejects_with_source_error() {
    let err = JsFuture::from(zonal_stats(
        url("does_not_exist.tif"),
        P3.to_string(),
        1,
        "count".to_string(),
    ))
    .await
    .expect_err("404 는 reject");
    // 에러 메시지에 URL 이 남아야 디버깅 가능 (SourceError 계약)
    let msg = js_sys::Error::from(err).message().as_string().unwrap();
    assert!(msg.contains("does_not_exist.tif"), "{msg}");
}
