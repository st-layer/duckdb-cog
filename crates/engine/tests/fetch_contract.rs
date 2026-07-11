//! T5 경계 계층 테스트 (RFC §6.9): reader 경계의 fetch 횟수·바이트를 계약화.
//!
//! lazy read 가 이 프로젝트의 존재 이유 — 메타데이터 나열이 픽셀 데이터를
//! 끌어오기 시작하면 회귀다. 수치가 바뀌면 async-tiff 동작 변화를 의심하고
//! 정당하면 이 계약을 사람이 승인해 갱신한다.

use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;

use engine::{read_cog_meta, ByteSource, MemorySource, SourceError};

/// fetch 호출·바이트를 세는 [`ByteSource`] 래퍼 (T5 전용).
#[derive(Debug)]
struct CountingSource {
    inner: MemorySource,
    fetches: Arc<AtomicUsize>,
    bytes: Arc<AtomicU64>,
}

impl ByteSource for CountingSource {
    fn fetch(
        &self,
        range: std::ops::Range<u64>,
    ) -> engine::futures::future::BoxFuture<'_, Result<engine::bytes::Bytes, SourceError>> {
        self.fetches.fetch_add(1, Ordering::Relaxed);
        self.bytes.fetch_add(range.end - range.start, Ordering::Relaxed);
        self.inner.fetch(range)
    }
}

fn fixture_bytes(name: &str) -> Vec<u8> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../test/data/generated")
        .join(name);
    std::fs::read(&path)
        .unwrap_or_else(|_| panic!("픽스처 없음: {} — `just fixtures` 로 생성", path.display()))
}

#[test]
fn metadata_listing_never_touches_pixel_data() {
    let raw = fixture_bytes("basic_512x512_u16.tif");
    let total = raw.len() as u64;
    let fetches = Arc::new(AtomicUsize::new(0));
    let bytes = Arc::new(AtomicU64::new(0));
    let source = CountingSource {
        inner: MemorySource::new(raw),
        fetches: Arc::clone(&fetches),
        bytes: Arc::clone(&bytes),
    };

    let meta = engine::futures::executor::block_on(read_cog_meta(source)).expect("valid COG");
    assert_eq!(meta.levels.len(), 2, "본체 + 오버뷰");

    let fetches = fetches.load(Ordering::Relaxed);
    let bytes = bytes.load(Ordering::Relaxed);
    // 계약 1: 요청 횟수는 상수 규모 (IFD 체인 순회) — 타일 수에 비례하면 회귀.
    assert!(fetches <= 8, "fetch {fetches}회 — 메타데이터 나열이 과도한 왕복 유발");
    // 계약 2: 읽은 바이트 ≪ 파일 크기 — 픽셀 데이터(파일의 대부분)를 안 끌어온다.
    assert!(
        bytes * 4 < total,
        "{bytes}B / 전체 {total}B — 메타데이터 읽기가 픽셀 영역을 침범"
    );
}

#[test]
fn edge_fixture_same_contract() {
    let raw = fixture_bytes("edge_400x300_u16.tif");
    let total = raw.len() as u64;
    let fetches = Arc::new(AtomicUsize::new(0));
    let bytes = Arc::new(AtomicU64::new(0));
    let source = CountingSource {
        inner: MemorySource::new(raw),
        fetches: Arc::clone(&fetches),
        bytes: Arc::clone(&bytes),
    };
    engine::futures::executor::block_on(read_cog_meta(source)).expect("valid COG");
    assert!(fetches.load(Ordering::Relaxed) <= 8);
    assert!(bytes.load(Ordering::Relaxed) * 4 < total);
}
