//! read_stac 의 엔진 재료 계약: STAC JSON 파싱 + 전체 문서 fetch.

use engine::{fetch_all, parse_stac, MemorySource};

fn block_on<F: std::future::Future>(f: F) -> F::Output {
    engine::futures::executor::block_on(f)
}

const ITEM: &str = r#"{
  "type": "Feature", "id": "solo", "collection": "c1",
  "bbox": [1.0, 2.0, 3.0, 4.0],
  "properties": { "datetime": "2026-01-01T00:00:00Z" },
  "assets": { "cog": { "href": "a.tif", "type": "image/tiff" } }
}"#;

#[test]
fn parses_single_item() {
    let rows = parse_stac(ITEM.as_bytes()).expect("valid STAC");
    assert_eq!(rows.len(), 1);
    let r = &rows[0];
    assert_eq!(r.item_id, "solo");
    assert_eq!(r.collection.as_deref(), Some("c1"));
    assert_eq!(r.datetime.as_deref(), Some("2026-01-01T00:00:00Z"));
    assert_eq!((r.asset_key.as_str(), r.href.as_str()), ("cog", "a.tif"));
    assert_eq!(r.media_type.as_deref(), Some("image/tiff"));
    assert_eq!(r.bbox, Some([1.0, 2.0, 3.0, 4.0]));
}

#[test]
fn parses_item_collection_with_graceful_degradation() {
    let doc = format!(
        r#"{{"type": "FeatureCollection", "features": [{ITEM},
            {{"type": "Feature", "id": "bare", "properties": {{}},
              "assets": {{"data": {{"href": "b.tif"}}}}}}]}}"#
    );
    let rows = parse_stac(doc.as_bytes()).expect("valid STAC");
    assert_eq!(rows.len(), 2);
    let bare = rows.iter().find(|r| r.item_id == "bare").expect("bare");
    assert_eq!(bare.collection, None);
    assert_eq!(bare.datetime, None);
    assert_eq!(bare.bbox, None);
    assert_eq!(bare.media_type, None);
}

#[test]
fn rejects_invalid_documents() {
    assert!(parse_stac(b"{ not json").is_err());
    assert!(
        parse_stac(br#"{"type": "Unknown"}"#).is_err(),
        "모르는 type"
    );
    // asset 에 href 가 없으면 그 asset 은 건너뛴다 (graceful) — item 자체는 유효
    let rows = parse_stac(
        br#"{"type": "Feature", "id": "x", "properties": {},
             "assets": {"broken": {}, "ok": {"href": "h.tif"}}}"#,
    )
    .expect("valid");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].asset_key, "ok");
}

#[test]
fn fetch_all_reads_whole_document_progressively() {
    // 초기 청크(64KiB)보다 큰 문서 — 점진 배증으로 전체 획득 (EOF 클램프 계약 활용)
    let big = vec![b'x'; 1_500_000];
    let got = block_on(fetch_all(&MemorySource::new(big.clone()))).expect("io ok");
    assert_eq!(got.len(), big.len());
    let small = vec![b'y'; 10];
    let got = block_on(fetch_all(&MemorySource::new(small.clone()))).expect("io ok");
    assert_eq!(got.as_ref(), small.as_slice());
}

#[test]
fn three_dimensional_bbox_drops_z() {
    // 6원소 bbox = [xmin, ymin, zmin, xmax, ymax, zmax] → z 제외 인덱스 (0,1,3,4)
    let rows = parse_stac(
        br#"{"type": "Feature", "id": "z", "bbox": [10.0, 20.0, -5.0, 30.0, 40.0, 99.0],
             "properties": {}, "assets": {"a": {"href": "h.tif"}}}"#,
    )
    .expect("valid");
    assert_eq!(rows[0].bbox, Some([10.0, 20.0, 30.0, 40.0]));
    // 그 외 길이(5 등)는 graceful None
    let rows = parse_stac(
        br#"{"type": "Feature", "id": "w", "bbox": [1.0, 2.0, 3.0, 4.0, 5.0],
             "properties": {}, "assets": {"a": {"href": "h.tif"}}}"#,
    )
    .expect("valid");
    assert_eq!(rows[0].bbox, None);
}
