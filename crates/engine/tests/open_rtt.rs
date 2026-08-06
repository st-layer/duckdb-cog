//! COG open 왕복수 계약 (#72): 메타데이터 open 은 **기저 fetch 정확히 1회**
//! (async-tiff ReadaheadMetadataCache 의 32KiB 초기 윈도)여야 한다.
//!
//! 실측(2026-08-06): 실제 S2 L2A B04 헤더(5레벨, 앞 2MB 실물)도 fetch 1회로
//! 전부 파싱됨 — 원격에서 fetch 1회 = RTT 1회다. 이 가드는 readahead 경로의
//! 회귀(태그 단위 직행 fetch 등 — 캐시 없이는 나열 한 번에 수백 fetch)를 잡는다.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use engine::{open_cog, ByteSource, MemorySource, SourceError};

#[derive(Debug)]
struct CountingSource {
    inner: MemorySource,
    fetches: Arc<AtomicUsize>,
}

impl ByteSource for CountingSource {
    fn fetch(
        &self,
        range: std::ops::Range<u64>,
    ) -> engine::futures::future::BoxFuture<'_, Result<engine::bytes::Bytes, SourceError>> {
        self.fetches.fetch_add(1, Ordering::Relaxed);
        self.inner.fetch(range)
    }
}

#[test]
fn open_costs_exactly_one_fetch_for_every_fixture() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../test/data/generated");
    let mut checked = 0;
    for entry in std::fs::read_dir(&dir)
        .unwrap_or_else(|_| panic!("픽스처 없음: {} — `just fixtures` 로 생성", dir.display()))
    {
        let path = entry.expect("dir entry").path();
        if path.extension().is_none_or(|e| e != "tif") {
            continue;
        }
        let data = std::fs::read(&path).expect("fixture read");
        let fetches = Arc::new(AtomicUsize::new(0));
        let src = CountingSource {
            inner: MemorySource::new(data),
            fetches: Arc::clone(&fetches),
        };
        engine::futures::executor::block_on(open_cog(src)).expect("valid COG");
        assert_eq!(
            fetches.load(Ordering::Relaxed),
            1,
            "{}: open 은 fetch 1회여야 한다",
            path.display()
        );
        checked += 1;
    }
    assert!(checked >= 5, "픽스처 {checked}개만 검사됨 — 생성 여부 확인");
}
