//! 오버뷰 fast mode 바인딩 계약 (#71): maxPixels 생략 = exact (기본 불변),
//! 예산 지정 시 exact 대비 불변식 (엔진 zonal_level.rs · rs_zonal_fast.test 와
//! 동일 판정 — 네이티브/브라우저가 같은 select_level/value_scaled 경로).
#![cfg(target_arch = "wasm32")]

use engine_wasm::{zonal_stats, zonal_stats_batch};
use wasm_bindgen::JsValue;
use wasm_bindgen_futures::JsFuture;
use wasm_bindgen_test::*;

wasm_bindgen_test_configure!(run_in_browser);

/// justfile wasm-test 레시피와의 포트 계약.
const BASE: &str = "http://127.0.0.1:18925";

fn url(name: &str) -> String {
    format!("{BASE}/{name}")
}

/// zonal_batch.rs 오라클 P3 (exact: count 7267, sum 239110784).
const P3: &str = "POLYGON ((300203.7 3999803.7, 301403.7 3999803.7, 300203.7 3998593.1, \
     300203.7 3999803.7))";
const EXACT_COUNT: f64 = 7_267.0;
const EXACT_SUM: f64 = 239_110_784.0;

async fn zonal(stat: &str, max_pixels: Option<u32>) -> f64 {
    JsFuture::from(zonal_stats(
        url("basic_512x512_u16.tif"),
        P3.to_string(),
        1,
        stat.to_string(),
        max_pixels,
    ))
    .await
    .expect("resolve")
    .as_f64()
    .expect("number")
}

#[wasm_bindgen_test]
async fn omitted_and_zero_budget_stay_exact() {
    // 생략(None)과 0 은 기존 exact 골든 그대로 — 기본 동작 불변
    assert_eq!(zonal("count", None).await, EXACT_COUNT);
    assert_eq!(zonal("sum", None).await, EXACT_SUM);
    assert_eq!(zonal("count", Some(0)).await, EXACT_COUNT);
}

#[wasm_bindgen_test]
async fn budgeted_zonal_holds_tolerance_invariants() {
    let exact_mean = EXACT_SUM / EXACT_COUNT;
    let fast_mean = zonal("mean", Some(5_000)).await;
    assert!(
        (fast_mean - exact_mean).abs() / exact_mean < 0.05,
        "mean 드리프트: {exact_mean} vs {fast_mean}"
    );
    let fast_count = zonal("count", Some(5_000)).await;
    assert!(
        (fast_count - EXACT_COUNT).abs() / EXACT_COUNT < 0.05,
        "환산 count: {fast_count}"
    );
}

#[wasm_bindgen_test]
async fn batch_accepts_the_budget_too() {
    let urls: js_sys::Array = [url("basic_512x512_u16.tif"), url("basic_512x512_u16.tif")]
        .iter()
        .map(|s| JsValue::from_str(s))
        .collect();
    let v = JsFuture::from(zonal_stats_batch(
        urls,
        P3.to_string(),
        1,
        "mean".to_string(),
        Some(5_000),
    ))
    .await
    .expect("resolve");
    let exact_mean = EXACT_SUM / EXACT_COUNT;
    let means: Vec<f64> = js_sys::Array::from(&v)
        .iter()
        .map(|x| x.as_f64().expect("number"))
        .collect();
    assert_eq!(means.len(), 2);
    for m in means {
        assert!((m - exact_mean).abs() / exact_mean < 0.05, "{m}");
    }
}
