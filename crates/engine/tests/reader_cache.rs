//! ReaderCache 계약 테스트 (이슈 #26, T5-식): 같은 key 의 두 번째 open 은
//! 원천 소스에 손대지 않는다 — 반복 원격 접근의 콜드 비용(~2s)이 프로세스
//! 안에서 반복되면 회귀다. TTL 0 은 캐시 비활성과 동치, cap 은 oldest-out.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use engine::{ByteSource, MemorySource, ReaderCache, SharedCog, SourceError};

/// open 횟수와 fetch 횟수를 세는 소스 팩토리 (T5 CountingSource 패턴).
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

fn fixture_bytes() -> Vec<u8> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../test/data/generated/basic_512x512_u16.tif");
    std::fs::read(&path)
        .unwrap_or_else(|_| panic!("픽스처 없음: {} — `just fixtures` 로 생성", path.display()))
}

/// (opens, fetches) 카운터가 달린 opener 를 만든다.
fn counting_opener(
    raw: Vec<u8>,
    opens: Arc<AtomicUsize>,
    fetches: Arc<AtomicUsize>,
) -> impl Fn() -> engine::futures::future::BoxFuture<'static, Result<SharedCog, engine::MetaError>>
{
    move || {
        opens.fetch_add(1, Ordering::Relaxed);
        let source: Box<dyn ByteSource> = Box::new(CountingSource {
            inner: MemorySource::new(raw.clone()),
            fetches: Arc::clone(&fetches),
        });
        Box::pin(engine::open_cog(source))
    }
}

#[test]
fn second_open_same_key_touches_nothing() {
    let raw = fixture_bytes();
    let opens = Arc::new(AtomicUsize::new(0));
    let fetches = Arc::new(AtomicUsize::new(0));
    let cache = ReaderCache::new(Duration::from_secs(3600), 8);
    let opener = counting_opener(raw, Arc::clone(&opens), Arc::clone(&fetches));

    let a = engine::futures::executor::block_on(cache.get_or_open("k", &opener)).expect("open");
    let after_first = (opens.load(Ordering::Relaxed), fetches.load(Ordering::Relaxed));
    assert_eq!(after_first.0, 1, "첫 조회는 실제 open");
    assert!(after_first.1 >= 1, "첫 open 은 소스를 읽는다");

    let b = engine::futures::executor::block_on(cache.get_or_open("k", &opener)).expect("hit");
    assert_eq!(
        (opens.load(Ordering::Relaxed), fetches.load(Ordering::Relaxed)),
        after_first,
        "두 번째 조회는 open 도 fetch 도 없어야 한다 (T5 계약)"
    );
    assert!(Arc::ptr_eq(&a, &b), "같은 항목의 Arc 공유");
    assert_eq!(a.0.width(), Some(512), "캐시된 meta 는 유효");
}

#[test]
fn ttl_zero_disables_caching() {
    let raw = fixture_bytes();
    let opens = Arc::new(AtomicUsize::new(0));
    let fetches = Arc::new(AtomicUsize::new(0));
    let cache = ReaderCache::new(Duration::ZERO, 8);
    let opener = counting_opener(raw, Arc::clone(&opens), Arc::clone(&fetches));

    engine::futures::executor::block_on(cache.get_or_open("k", &opener)).expect("open");
    engine::futures::executor::block_on(cache.get_or_open("k", &opener)).expect("reopen");
    assert_eq!(
        opens.load(Ordering::Relaxed),
        2,
        "TTL 0 = 항상 miss (캐시 비활성)"
    );
}

#[test]
fn distinct_keys_are_isolated() {
    let raw = fixture_bytes();
    let opens = Arc::new(AtomicUsize::new(0));
    let fetches = Arc::new(AtomicUsize::new(0));
    let cache = ReaderCache::new(Duration::from_secs(3600), 8);
    let opener = counting_opener(raw, Arc::clone(&opens), Arc::clone(&fetches));

    let a = engine::futures::executor::block_on(cache.get_or_open("a", &opener)).expect("a");
    let b = engine::futures::executor::block_on(cache.get_or_open("b", &opener)).expect("b");
    assert_eq!(opens.load(Ordering::Relaxed), 2, "key 별 독립 open");
    assert!(!Arc::ptr_eq(&a, &b));
}

#[test]
fn cap_evicts_oldest() {
    let raw = fixture_bytes();
    let opens = Arc::new(AtomicUsize::new(0));
    let fetches = Arc::new(AtomicUsize::new(0));
    let cache = ReaderCache::new(Duration::from_secs(3600), 1);
    let opener = counting_opener(raw, Arc::clone(&opens), Arc::clone(&fetches));

    engine::futures::executor::block_on(cache.get_or_open("a", &opener)).expect("a");
    engine::futures::executor::block_on(cache.get_or_open("b", &opener)).expect("b (a 축출)");
    engine::futures::executor::block_on(cache.get_or_open("a", &opener)).expect("a 재-open");
    assert_eq!(
        opens.load(Ordering::Relaxed),
        3,
        "cap=1 에서 a→b→a 는 3회 open (oldest-out)"
    );
}
