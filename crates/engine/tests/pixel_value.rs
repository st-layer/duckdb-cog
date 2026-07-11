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

/// 배치 읽기: 개별 read_pixel 과 동일 결과 + 위치 보존.
#[test]
fn batch_matches_single_reads() {
    let (meta, reader) = block_on(open_cog(fixture("basic_512x512_u16.tif"))).expect("valid COG");
    let pts = [
        (300005.0, 3999995.0),
        (0.0, 0.0), // extent 밖 → None
        (302565.0, 3997435.0),
        (305115.0, 3994885.0),
    ];
    let batch = block_on(reader.read_pixels(&meta, &pts, 1)).expect("io ok");
    assert_eq!(batch.len(), pts.len());
    for (i, (x, y)) in pts.iter().enumerate() {
        let single = block_on(reader.read_pixel(&meta, *x, *y, 1)).expect("io ok");
        assert_eq!(batch[i], single, "위치 {i} 불일치");
    }
}

/// T5 스타일 decode 효율 계약: 같은 타일의 점 4096개 배치 → 타일 fetch 는 1회.
/// (naive 구현은 점마다 fetch+decode — O(points) fetch 가 나오면 회귀)
#[test]
fn batch_fetches_each_tile_once() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    #[derive(Debug)]
    struct CountingSource {
        inner: MemorySource,
        fetches: Arc<AtomicUsize>,
    }
    impl engine::ByteSource for CountingSource {
        fn fetch(
            &self,
            range: std::ops::Range<u64>,
        ) -> engine::futures::future::BoxFuture<'_, Result<engine::bytes::Bytes, engine::SourceError>>
        {
            self.fetches.fetch_add(1, Ordering::Relaxed);
            self.inner.fetch(range)
        }
    }

    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../test/data/generated/multiband_64x64_u8.tif");
    let raw = std::fs::read(&path).expect("픽스처 없음 — `just fixtures`");
    let fetches = Arc::new(AtomicUsize::new(0));
    let source = CountingSource {
        inner: MemorySource::new(raw),
        fetches: Arc::clone(&fetches),
    };
    let (meta, reader) = block_on(open_cog(source)).expect("valid COG");
    let meta_fetches = fetches.load(Ordering::Relaxed);

    // 64x64 전 픽셀 중심 — 단일 타일 (256 블록에 패딩된 1타일)
    let g = meta.georef.clone().expect("georef");
    let pts: Vec<(f64, f64)> = (0..64)
        .flat_map(|r| {
            let g = g.clone();
            (0..64).map(move |c| {
                (
                    g.origin_x + (c as f64 + 0.5) * g.pixel_x,
                    g.origin_y - (r as f64 + 0.5) * g.pixel_y,
                )
            })
        })
        .collect();
    let vals = block_on(reader.read_pixels(&meta, &pts, 1)).expect("io ok");
    assert_eq!(vals.len(), 4096);
    assert!(vals.iter().all(|v| v.is_some()), "전 픽셀 값 존재");
    let tile_fetches = fetches.load(Ordering::Relaxed) - meta_fetches;
    assert!(
        tile_fetches <= 2,
        "타일 fetch {tile_fetches}회 — 4096점 배치가 타일을 반복 fetch (캐시 회귀)"
    );
}

/// normalized difference 순수 함수 계약: (v2-v1)/(v2+v1), 합 0·결측 → None.
#[test]
fn normalized_difference_contract() {
    use engine::normalized_difference as nd;
    assert_eq!(
        nd(Some(191.0), Some(110.0)),
        Some((110.0 - 191.0) / (110.0 + 191.0))
    );
    assert_eq!(nd(Some(5.0), Some(5.0)), Some(0.0));
    assert_eq!(nd(Some(0.0), Some(0.0)), None, "합 0 → 정의 불가");
    assert_eq!(nd(Some(-3.0), Some(3.0)), None, "합 0 (부호 상쇄)");
    assert_eq!(nd(None, Some(1.0)), None);
    assert_eq!(nd(Some(1.0), None), None);
}

/// bbox 영역 집계: rasterio+numpy 오라클 수치와 일치 (픽셀 중심 포함, nodata 제외).
#[test]
fn zonal_stats_matches_oracle_numbers() {
    let (meta, reader) = block_on(open_cog(fixture("basic_512x512_u16.tif"))).expect("valid COG");
    let z = block_on(reader.zonal_stats(&meta, [300000.0, 3999000.0, 301000.0, 4000000.0], 1))
        .expect("io ok");
    assert_eq!(z.count, 10_000);
    assert_eq!(z.sum, 326_869_359.0);
    assert_eq!(z.min, Some(17.0));
    assert_eq!(z.max, Some(65_532.0));
    // 타일 경계를 걸치는 영역 (4타일)
    let z2 = block_on(reader.zonal_stats(&meta, [302000.0, 3997000.0, 303000.0, 3998000.0], 1))
        .expect("io ok");
    assert_eq!((z2.count, z2.sum), (10_000, 329_035_529.0));
}

#[test]
fn zonal_stats_excludes_nodata_and_handles_empty() {
    let (meta, reader) =
        block_on(open_cog(fixture("nodatahole_64x64_u16.tif"))).expect("valid COG");
    let z = block_on(reader.zonal_stats(&meta, [900000.0, 3999980.0, 900020.0, 4000000.0], 1))
        .expect("io ok");
    assert_eq!(
        (z.count, z.sum, z.min, z.max),
        (3, 26_257.0, Some(5849.0), Some(14_370.0))
    );
    // extent 와 교차하지 않는 영역 → 빈 집계
    let empty = block_on(reader.zonal_stats(&meta, [0.0, 0.0, 1.0, 1.0], 1)).expect("io ok");
    assert_eq!((empty.count, empty.min), (0, None));
    // 범위 밖 밴드 → 빈 집계
    let nb = block_on(reader.zonal_stats(&meta, [900000.0, 3999980.0, 900020.0, 4000000.0], 9))
        .expect("io ok");
    assert_eq!(nb.count, 0);
    // 역전 bbox → 에러
    assert!(
        block_on(reader.zonal_stats(&meta, [1.0, 0.0, 0.0, 1.0], 1)).is_err(),
        "역전 bbox 는 에러"
    );
}
