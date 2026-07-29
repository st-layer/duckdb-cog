//! 타일 데이터 캐시 계약 (이슈 #55, T5-식): 필지 워크로드가 같은 타일을
//! ~168번 재다운로드하던 것을 끝낸다 — 같은 타일에 대한 두 번째 접근은
//! 원천 소스에 손대지 않는다. 값은 캐시 유무와 무관하게 비트 동일해야 하며
//! (rasterio 오라클이 상위에서 이중 감시), 캐시 미부착 리더는 현행 그대로다.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use engine::{open_cog, ByteSource, MemorySource, SourceError, TileCache};

/// fetch 횟수를 세는 소스 (T5 CountingSource 패턴) — 지연 주입 가능.
#[derive(Debug)]
struct CountingSource {
    inner: MemorySource,
    fetches: Arc<AtomicUsize>,
    delay: Option<Duration>,
}

impl CountingSource {
    fn new(raw: Vec<u8>, fetches: Arc<AtomicUsize>) -> Self {
        Self {
            inner: MemorySource::new(raw),
            fetches,
            delay: None,
        }
    }
}

impl ByteSource for CountingSource {
    fn fetch(
        &self,
        range: std::ops::Range<u64>,
    ) -> engine::futures::future::BoxFuture<'_, Result<engine::bytes::Bytes, SourceError>> {
        self.fetches.fetch_add(1, Ordering::Relaxed);
        if let Some(d) = self.delay {
            std::thread::sleep(d);
        }
        self.inner.fetch(range)
    }
}

fn fixture_bytes() -> Vec<u8> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../test/data/generated/basic_512x512_u16.tif");
    std::fs::read(&path)
        .unwrap_or_else(|_| panic!("픽스처 없음: {} — `just fixtures` 로 생성", path.display()))
}

fn block_on<F: std::future::Future>(f: F) -> F::Output {
    engine::futures::executor::block_on(f)
}

/// (reader, meta, fetch 카운터) — 캐시를 붙일지는 호출측이 정한다.
fn open_counted(
    cache: Option<&TileCache>,
) -> (
    engine::CogReader<CountingSource>,
    engine::CogMeta,
    Arc<AtomicUsize>,
) {
    let fetches = Arc::new(AtomicUsize::new(0));
    let source = CountingSource::new(fixture_bytes(), Arc::clone(&fetches));
    let (meta, mut reader) = block_on(open_cog(source)).expect("valid COG");
    if let Some(c) = cache {
        reader.attach_tile_cache(c);
    }
    (reader, meta, fetches)
}

/// basic 픽스처 (512², 256px 타일, origin 300000/4000000, 10m): 타일 (0,0) 안의
/// 소형 창 — 필지 모사 (100×100m = 10×10px).
const PARCEL: [f64; 4] = [301000.0, 3998000.0, 301100.0, 3998100.0];

#[test]
fn second_zonal_over_same_tile_touches_nothing() {
    let cache = TileCache::new(64 * 1024 * 1024);
    let (reader, meta, fetches) = open_counted(Some(&cache));

    let z1 = block_on(reader.zonal_stats(&meta, PARCEL, 1)).expect("io ok");
    let after_first = fetches.load(Ordering::Relaxed);
    assert!(after_first >= 1, "첫 호출은 실제 fetch");

    let z2 = block_on(reader.zonal_stats(&meta, PARCEL, 1)).expect("io ok");
    assert_eq!(
        fetches.load(Ordering::Relaxed),
        after_first,
        "웜 재호출은 추가 fetch 0 (이슈 #55 의 48.8s→46.7s 동일 비용이 이 부재의 증거였다)"
    );
    assert_eq!((z1.count, z1.sum), (z2.count, z2.sum), "값은 비트 동일");
}

#[test]
fn n_parcels_in_one_tile_cost_one_tile_fetch() {
    let cache = TileCache::new(64 * 1024 * 1024);
    let (reader, meta, fetches) = open_counted(Some(&cache));
    let meta_fetches = fetches.load(Ordering::Relaxed); // open 시 메타 읽기 분

    // 같은 타일 (0,0) 안의 필지 20개 (10×10px 창을 격자로 이동).
    // 타일 0 의 y 범위는 row<256 ⇔ y>3997440 — 그리드 전체(최저 y 3997600)가 안에 든다.
    for i in 0..20u32 {
        let dx = f64::from(i % 5) * 300.0;
        let dy = f64::from(i / 5) * 300.0;
        let parcel = [
            300100.0 + dx,
            3997600.0 + dy,
            300200.0 + dx,
            3997700.0 + dy,
        ];
        block_on(reader.zonal_stats(&meta, parcel, 1)).expect("io ok");
    }
    let tile_fetches = fetches.load(Ordering::Relaxed) - meta_fetches;
    assert_eq!(
        tile_fetches, 1,
        "한 타일을 공유하는 N개 필지 = 타일 fetch 1회 (리포트의 핵심 계약: 168× → 1×)"
    );
}

#[test]
fn detached_reader_keeps_current_behaviour() {
    let (reader, meta, fetches) = open_counted(None);
    let meta_fetches = fetches.load(Ordering::Relaxed);
    block_on(reader.zonal_stats(&meta, PARCEL, 1)).expect("io ok");
    block_on(reader.zonal_stats(&meta, PARCEL, 1)).expect("io ok");
    assert_eq!(
        fetches.load(Ordering::Relaxed) - meta_fetches,
        2,
        "캐시 미부착(로컬 경로 상당) 리더는 호출마다 fetch — 현행 유지"
    );
}

#[test]
fn tiny_budget_evicts_and_refetches() {
    let cache = TileCache::new(1); // 어떤 타일도 못 담는 예산
    let (reader, meta, fetches) = open_counted(Some(&cache));
    let meta_fetches = fetches.load(Ordering::Relaxed);
    block_on(reader.zonal_stats(&meta, PARCEL, 1)).expect("io ok");
    block_on(reader.zonal_stats(&meta, PARCEL, 1)).expect("io ok");
    assert_eq!(
        fetches.load(Ordering::Relaxed) - meta_fetches,
        2,
        "예산 초과 타일은 상주 불가 → 재호출은 재fetch (동작은 여전히 옳아야 한다)"
    );
}

#[test]
fn readers_do_not_cross_serve() {
    let cache = TileCache::new(64 * 1024 * 1024);
    let (ra, ma, _fa) = open_counted(Some(&cache));
    let (rb, mb, fb) = open_counted(Some(&cache));

    // A 가 타일을 캐시에 올린 뒤 B 가 같은 좌표를 읽어도 B 는 자기 소스에서 fetch
    block_on(ra.zonal_stats(&ma, PARCEL, 1)).expect("io ok");
    let b_before = fb.load(Ordering::Relaxed);
    block_on(rb.zonal_stats(&mb, PARCEL, 1)).expect("io ok");
    assert!(
        fb.load(Ordering::Relaxed) > b_before,
        "리더 격리: 같은 캐시라도 ReaderId 가 다르면 교차 서빙 금지 (재열림 무효화의 근거)"
    );
}

#[test]
fn cold_tile_single_flight_under_concurrency() {
    let cache = TileCache::new(64 * 1024 * 1024);
    let fetches = Arc::new(AtomicUsize::new(0));
    let mut source = CountingSource::new(fixture_bytes(), Arc::clone(&fetches));
    source.delay = Some(Duration::from_millis(50)); // 스탬피드 창을 벌린다
    let (meta, mut reader) = block_on(open_cog(source)).expect("valid COG");
    reader.attach_tile_cache(&cache);
    let meta_fetches = fetches.load(Ordering::Relaxed);

    let reader = Arc::new(reader);
    let meta = Arc::new(meta);
    let handles: Vec<_> = (0..8)
        .map(|_| {
            let (r, m) = (Arc::clone(&reader), Arc::clone(&meta));
            std::thread::spawn(move || {
                block_on(r.zonal_stats(&m, PARCEL, 1)).expect("io ok");
            })
        })
        .collect();
    for h in handles {
        h.join().unwrap();
    }
    assert_eq!(
        fetches.load(Ordering::Relaxed) - meta_fetches,
        1,
        "콜드 타일에 8스레드 동시 진입 = fetch 1회 (single-flight, 스탬피드 금지)"
    );
}
