//! URL→리더 레지스트리 + TileCache (#66 슬라이스 3).
//!
//! TileCache 는 ReaderId 키(tile_cache.rs)라 **리더를 재사용해야 캐시가 산다**
//! — 같은 씬들 위에서 폴리곤을 수정하는 상호작용 패턴의 웜 경로. engine 의
//! ReaderCache 는 쓰지 않는다 (`Instant::now()` 가 wasm 런타임 panic).
//! wasm 은 단일 스레드 — thread_local + RefCell 로 충분하며, **borrow 는
//! await 너머로 들고 가지 않는다** (조회/삽입은 동기 구간에서만).

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use engine::{open_cog, CogMeta, CogReader, MetaError, TileCache, TileCacheStats};

use crate::fetch::FetchSource;

pub(crate) type Opened = (CogMeta, CogReader<FetchSource>);

/// 브라우저 기본 타일 캐시 상한 — 네이티브 256MB 대비 축소 (#66 노트).
const DEFAULT_TILE_CACHE_MB: usize = 64;
/// 레지스트리 상한 — 초과 시 통째 clear (조잡하지만 단순; 버려진 ReaderId 의
/// 타일은 도달 불가가 되어 LRU 압력으로 자연 소멸).
const MAX_READERS: usize = 64;

thread_local! {
    static READERS: RefCell<HashMap<String, Rc<Opened>>> = RefCell::new(HashMap::new());
    static TILE_CACHE: RefCell<Option<TileCache>> =
        RefCell::new(Some(TileCache::new(DEFAULT_TILE_CACHE_MB * 1024 * 1024)));
}

/// 캐시 재구성 (0 = 비활성) — 기존 내용은 버려진다. 레지스트리도 함께 비워
/// 기존 리더가 옛 캐시를 계속 잡는 비일관을 막는다 (콜드 리셋).
pub(crate) fn configure_tile_cache(mb: u32) {
    TILE_CACHE.with(|c| {
        *c.borrow_mut() = (mb > 0).then(|| TileCache::new(mb as usize * 1024 * 1024));
    });
    READERS.with(|r| r.borrow_mut().clear());
}

/// 카운터 스냅샷 — 비활성이면 0 카운터 (max_bytes 0).
pub(crate) fn tile_cache_stats() -> TileCacheStats {
    TILE_CACHE.with(|c| {
        c.borrow()
            .as_ref()
            .map(|t| t.stats())
            .unwrap_or(TileCacheStats {
                hits: 0,
                misses: 0,
                evictions: 0,
                bytes: 0,
                max_bytes: 0,
            })
    })
}

/// URL 의 (meta, reader) — 히트면 재사용(같은 ReaderId → 타일 캐시 유효),
/// 미스면 open 후 캐시 부착·등록. 같은 URL 동시 미스는 중복 open 될 수 있으나
/// 마지막 삽입이 이기고 값은 동일 (배치 호출측이 dedupe 로 예방).
pub(crate) async fn open_cached(url: &str) -> Result<Rc<Opened>, MetaError> {
    if let Some(hit) = READERS.with(|r| r.borrow().get(url).cloned()) {
        return Ok(hit);
    }
    let (meta, mut reader) = open_cog(FetchSource::new(url)).await?;
    TILE_CACHE.with(|c| {
        if let Some(tc) = c.borrow().as_ref() {
            reader.attach_tile_cache(tc);
        }
    });
    let opened = Rc::new((meta, reader));
    READERS.with(|r| {
        let mut map = r.borrow_mut();
        if map.len() >= MAX_READERS {
            map.clear();
        }
        map.insert(url.to_string(), Rc::clone(&opened));
    });
    Ok(opened)
}
