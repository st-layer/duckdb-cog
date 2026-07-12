//! 프로세스 수명 리더 캐시 (이슈 #26): 같은 key(URL)의 반복 open 이 원천
//! 소스를 다시 읽지 않게 한다 — 원격 콜드 메타(~2s)가 반복되는 것이 문제였고,
//! 계약은 tests/reader_cache.rs (T5-식: 두 번째 조회 = open 0 · fetch 0).
//!
//! 정책(무엇을 언제 캐시할지 — 원격만, TTL 기본값, env 스위치)은 호출측
//! 소관이다. 여기는 기제만: TTL 신선도, cap 초과 시 oldest-out.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use futures::future::BoxFuture;

use crate::meta::CogMeta;
use crate::pixel::CogReader;
use crate::source::ByteSource;

/// 캐시 공유 단위: 메타 + 픽셀 리더 (스칼라 배선의 `Opened` 와 동형).
pub type SharedCog = (CogMeta, CogReader<Box<dyn ByteSource>>);

struct Entry {
    cog: Arc<SharedCog>,
    inserted: Instant,
}

/// TTL + cap 짜리 key→[`SharedCog`] 캐시. 스레드 안전 (DuckDB 스칼라는
/// 멀티스레드 실행). LRU 아님 — 접근이 아니라 삽입 시각 기준 oldest-out.
pub struct ReaderCache {
    ttl: Duration,
    cap: usize,
    map: Mutex<HashMap<String, Entry>>,
}

impl ReaderCache {
    /// `ttl == Duration::ZERO` 는 캐시 비활성과 동치 (항상 miss, 삽입도 안 함).
    pub fn new(ttl: Duration, cap: usize) -> Self {
        Self {
            ttl,
            cap,
            map: Mutex::new(HashMap::new()),
        }
    }

    /// hit(신선) → 공유 Arc. miss/만료 → `opener` 실행 후 삽입.
    ///
    /// 잠금은 조회/삽입 순간에만 잡는다 — open(원격 왕복, 초 단위) 동안 다른
    /// key 를 막지 않기 위해. 대가로 같은 key 의 동시 miss 는 중복 open 될 수
    /// 있다 (후승자가 삽입을 덮음 — 정확성 무영향, 의도된 트레이드오프).
    pub async fn get_or_open<F, E>(&self, key: &str, opener: F) -> Result<Arc<SharedCog>, E>
    where
        F: FnOnce() -> BoxFuture<'static, Result<SharedCog, E>>,
    {
        if self.ttl > Duration::ZERO {
            let map = self.map.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(e) = map.get(key) {
                if e.inserted.elapsed() < self.ttl {
                    return Ok(Arc::clone(&e.cog));
                }
            }
        }
        let cog = Arc::new(opener().await?);
        if self.ttl > Duration::ZERO {
            let mut map = self.map.lock().unwrap_or_else(|e| e.into_inner());
            let ttl = self.ttl;
            map.retain(|_, e| e.inserted.elapsed() < ttl);
            if map.len() >= self.cap && !map.contains_key(key) {
                let oldest = map
                    .iter()
                    .min_by_key(|(_, e)| e.inserted)
                    .map(|(k, _)| k.clone());
                if let Some(k) = oldest {
                    map.remove(&k);
                }
            }
            map.insert(
                key.to_string(),
                Entry {
                    cog: Arc::clone(&cog),
                    inserted: Instant::now(),
                },
            );
        }
        Ok(cog)
    }
}
