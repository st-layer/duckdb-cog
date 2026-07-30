//! 캐시 관측 계약 (이슈 #61): hits/misses/evictions/bytes 카운터.
//! 필드 리포트 4차 ① — 작업집합이 예산을 넘으면 캐시가 **무음 스래싱**해서
//! 74분을 태우고서야 알았다. 카운터가 있으면 1분 진단 (miss·eviction 이
//! 치솟고 bytes 가 예산에 고정되는 패턴).

use engine::{open_cog, MemorySource, TileCache};

fn fixture_bytes() -> Vec<u8> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../test/data/generated/basic_512x512_u16.tif");
    std::fs::read(&path)
        .unwrap_or_else(|_| panic!("픽스처 없음: {} — `just fixtures` 로 생성", path.display()))
}

fn block_on<F: std::future::Future>(f: F) -> F::Output {
    engine::futures::executor::block_on(f)
}

/// 타일 (0,0) 안의 소형 창 (tile_cache.rs 와 동일 좌표).
const PARCEL: [f64; 4] = [301000.0, 3998000.0, 301100.0, 3998100.0];

#[test]
fn stats_track_miss_then_hit_and_resident_bytes() {
    let cache = TileCache::new(64 * 1024 * 1024);
    let s0 = cache.stats();
    assert_eq!(
        (s0.hits, s0.misses, s0.evictions, s0.bytes),
        (0, 0, 0, 0),
        "초기 상태는 전부 0"
    );
    assert_eq!(s0.max_bytes, 64 * 1024 * 1024);

    let (meta, mut reader) =
        block_on(open_cog(MemorySource::new(fixture_bytes()))).expect("valid COG");
    reader.attach_tile_cache(&cache);

    block_on(reader.zonal_stats(&meta, PARCEL, 1)).expect("io ok");
    let s1 = cache.stats();
    assert_eq!((s1.hits, s1.misses), (0, 1), "콜드 = miss 1");
    assert!(s1.bytes > 0, "타일이 상주");
    assert!(s1.bytes <= s1.max_bytes);

    block_on(reader.zonal_stats(&meta, PARCEL, 1)).expect("io ok");
    let s2 = cache.stats();
    assert_eq!((s2.hits, s2.misses), (1, 1), "웜 = hit 1, miss 불변");
    assert_eq!(s2.bytes, s1.bytes, "상주 바이트 불변");
    assert_eq!(s2.evictions, 0);
}

#[test]
fn stats_count_evictions_under_budget_pressure() {
    // 예산 1바이트: 어떤 타일도 상주 불가 — 삽입 즉시 축출이 매번 찍힌다.
    // 필드 리포트의 스래싱 시그니처: miss·eviction 만 늘고 hit 0, bytes 0 고정.
    let cache = TileCache::new(1);
    let (meta, mut reader) =
        block_on(open_cog(MemorySource::new(fixture_bytes()))).expect("valid COG");
    reader.attach_tile_cache(&cache);

    block_on(reader.zonal_stats(&meta, PARCEL, 1)).expect("io ok");
    block_on(reader.zonal_stats(&meta, PARCEL, 1)).expect("io ok");
    let s = cache.stats();
    assert_eq!(s.hits, 0, "스래싱 = hit 없음");
    assert_eq!(s.misses, 2);
    assert_eq!(s.evictions, 2, "삽입 즉시 축출 × 2");
    assert_eq!(s.bytes, 0, "상주 불가");
}
