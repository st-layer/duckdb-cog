//! 타일 데이터 캐시 (이슈 #55): 필지 워크로드가 같은 타일을 ~168번 재다운로드
//! 하던 것을 끝낸다. decoded 타일을 바이트 상한 LRU 로 보관하고, 콜드 타일에
//! 몰리는 동시 호출은 single-flight 로 1회 fetch 에 수렴시킨다.
//!
//! 배치 결정: 저장소는 프로세스 전역 1개(ext 가 주입), 키는 `ReaderId` 포함 —
//! 리더 캐시(#26)가 리더를 교체하면 새 id 가 발급되므로 낡은 타일은 자동으로
//! 도달 불가가 된다 (별도 TTL 불요; 타일 수명 ≤ 리더 수명). 계약은
//! tests/tile_cache.rs. 순수 std — wasm 컴파일 무해 (G8).

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};

use async_tiff::{Array, TypedArray};

/// 캐시가 발급하는 리더 식별자 — 재열림마다 새 값 (무효화의 근거).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ReaderId(u64);

/// (리더, tile_x, tile_y). 현 픽셀 경로는 level 0 만 fetch 한다 —
/// 레벨이 추가되면 키를 넓힌다 (지금 넣으면 죽은 차원).
pub(crate) type Key = (ReaderId, usize, usize);

/// fetch 완료 신호: (done, Condvar). done 은 성공/실패 무관 "결판났음".
type Signal = Arc<(Mutex<bool>, Condvar)>;

#[derive(Debug)]
enum Slot {
    Ready {
        arr: Arc<Array>,
        bytes: usize,
        last_used: u64,
    },
    Pending(Signal),
}

#[derive(Debug, Default)]
struct State {
    map: HashMap<Key, Slot>,
    used: usize,
    tick: u64,
}

/// 키 하나의 조회 결과: 히트이거나, 이 호출이 fetch 책임을 진다.
pub(crate) enum Claim {
    Hit(Arc<Array>),
    Mine,
}

/// 프로세스 전역 타일 캐시 — `Clone` 은 같은 저장소 공유.
#[derive(Debug, Clone)]
pub struct TileCache(Arc<Inner>);

#[derive(Debug)]
struct Inner {
    max_bytes: usize,
    next_id: AtomicU64,
    state: Mutex<State>,
}

impl TileCache {
    /// `max_bytes` 상한의 캐시. 단일 타일이 상한을 넘으면 상주하지 못한다
    /// (삽입 즉시 축출 — 동작은 옳고 재fetch 만 발생).
    pub fn new(max_bytes: usize) -> Self {
        Self(Arc::new(Inner {
            max_bytes,
            next_id: AtomicU64::new(0),
            state: Mutex::new(State::default()),
        }))
    }

    /// 리더 등록 — 호출마다 새 id. 같은 URL 이라도 재열림이면 다른 키 공간.
    pub fn register(&self) -> ReaderId {
        ReaderId(self.0.next_id.fetch_add(1, Ordering::Relaxed))
    }

    /// 각 키를 히트/책임으로 판정한다. 다른 호출이 fetch 중(Pending)이면
    /// 결판까지 **스레드 블로킹 대기** — 픽셀 경로는 DuckDB 스레드가
    /// executor::block_on 으로 동기 실행하므로 이것이 올바른 원시다.
    ///
    /// 전제: `keys` 안에 중복 없음 (호출측 타일 목록은 dedup 되어 있다).
    /// `Mine` 을 받은 키는 반드시 [`Self::fulfill`] 또는 [`Self::abort`] 로
    /// 결판을 내야 한다 — 안 내면 대기자가 영원히 깬다/잔다를 반복한다.
    pub(crate) fn claim(&self, keys: &[Key]) -> Vec<Claim> {
        keys.iter().map(|k| self.claim_one(*k)).collect()
    }

    fn claim_one(&self, key: Key) -> Claim {
        loop {
            let signal: Signal = {
                let mut st = self.0.state.lock().unwrap_or_else(|e| e.into_inner());
                st.tick += 1;
                let tick = st.tick;
                match st.map.get_mut(&key) {
                    Some(Slot::Ready { arr, last_used, .. }) => {
                        *last_used = tick;
                        return Claim::Hit(Arc::clone(arr));
                    }
                    Some(Slot::Pending(sig)) => Arc::clone(sig),
                    None => {
                        st.map.insert(
                            key,
                            Slot::Pending(Arc::new((Mutex::new(false), Condvar::new()))),
                        );
                        return Claim::Mine;
                    }
                }
            };
            // 전역 락 없이 결판 대기 — 캐시가 직렬화 지점이 되지 않게 한다
            let (done, cv) = &*signal;
            let mut done = done.lock().unwrap_or_else(|e| e.into_inner());
            while !*done {
                done = cv.wait(done).unwrap_or_else(|e| e.into_inner());
            }
            // 결판 후 재판정: Ready(성공) → 히트, 소멸(실패/축출) → 내가 책임
        }
    }

    /// `Mine` 키에 값을 게재하고 대기자를 깨운다. 예산 초과분은 LRU 축출.
    pub(crate) fn fulfill(&self, key: Key, arr: Arc<Array>) {
        let bytes = array_bytes(&arr);
        let signal = {
            let mut st = self.0.state.lock().unwrap_or_else(|e| e.into_inner());
            st.tick += 1;
            let tick = st.tick;
            let prev = st.map.insert(
                key,
                Slot::Ready {
                    arr,
                    bytes,
                    last_used: tick,
                },
            );
            st.used += bytes;
            // 예산 집행: 방금 넣은 것 포함 last_used 최소부터 — 엔트리 수백 규모라
            // O(n) 스캔으로 충분 (필지 AOI ≈ 타일 수십 개)
            while st.used > self.0.max_bytes {
                let victim = st
                    .map
                    .iter()
                    .filter_map(|(k, s)| match s {
                        Slot::Ready { last_used, .. } => Some((*last_used, *k)),
                        Slot::Pending(_) => None,
                    })
                    .min()
                    .map(|(_, k)| k);
                let Some(v) = victim else { break };
                if let Some(Slot::Ready { bytes, .. }) = st.map.remove(&v) {
                    st.used -= bytes;
                }
            }
            match prev {
                Some(Slot::Pending(sig)) => Some(sig),
                _ => None,
            }
        };
        if let Some(sig) = signal {
            let (done, cv) = &*sig;
            *done.lock().unwrap_or_else(|e| e.into_inner()) = true;
            cv.notify_all();
        }
    }

    /// fetch 실패 시 `Mine` 키를 회수하고 대기자를 깨운다 — 깬 쪽이 스스로
    /// 재시도(claim)하게 된다.
    pub(crate) fn abort(&self, key: Key) {
        let signal = {
            let mut st = self.0.state.lock().unwrap_or_else(|e| e.into_inner());
            match st.map.remove(&key) {
                Some(Slot::Pending(sig)) => Some(sig),
                Some(ready @ Slot::Ready { .. }) => {
                    // 이미 결판난 키의 abort 는 무의미 — 되돌린다
                    st.map.insert(key, ready);
                    None
                }
                None => None,
            }
        };
        if let Some(sig) = signal {
            let (done, cv) = &*sig;
            *done.lock().unwrap_or_else(|e| e.into_inner()) = true;
            cv.notify_all();
        }
    }
}

/// decoded 타일의 상주 바이트 (배열 본체 기준 — 고정 오버헤드는 무시 가능 규모).
fn array_bytes(arr: &Array) -> usize {
    fn v<T>(x: &[T]) -> usize {
        std::mem::size_of_val(x)
    }
    match arr.data() {
        TypedArray::Bool(d) => v(d),
        TypedArray::UInt8(d) => v(d),
        TypedArray::UInt16(d) => v(d),
        TypedArray::UInt32(d) => v(d),
        TypedArray::UInt64(d) => v(d),
        TypedArray::Int8(d) => v(d),
        TypedArray::Int16(d) => v(d),
        TypedArray::Int32(d) => v(d),
        TypedArray::Int64(d) => v(d),
        TypedArray::Float32(d) => v(d),
        TypedArray::Float64(d) => v(d),
    }
}
