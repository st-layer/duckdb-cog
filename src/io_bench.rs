// cog_io_bench — RFC §6.5(b) I/O 경로 실측 하네스 (이슈 #30, **experimental**).
//
// (a) object_store/file 소스와 (b) DuckDB FileSystem C-API 소스를 같은 엔진
// 워크로드(메타 → 산개 포인트 → 중앙 창 zonal)로 돌려 시간·fetch 횟수·바이트·
// 체크섬을 (metric, value) 행으로 노출한다. §6.5(b) 채택 결정이 내려지면 제거 예정.
// (lib.rs 가 include! 로 포함하므로 //! 내부 독 주석은 못 쓴다.)

use std::error::Error;
use std::ffi::CString;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use duckdb::core::{DataChunkHandle, Inserter, LogicalTypeHandle, LogicalTypeId};
use duckdb::ffi;
use duckdb::vtab::{BindInfo, InitInfo, TableFunctionInfo, VTab};
use duckdb::Connection;

// crate:: 가 아니라 super:: — wasm 우회 빌드에선 lib.rs 가 크레이트 루트가 아니다.
use super::{open_source, FileSource, ObjectStoreSource};

/// fetch 횟수·요청 바이트 누산기 — T5 CountingSource 패턴
/// (crates/engine/tests/fetch_contract.rs)의 인프로세스 이식.
#[derive(Debug, Clone, Default)]
struct IoCounters {
    fetches: Arc<AtomicUsize>,
    bytes: Arc<AtomicU64>,
}

impl IoCounters {
    fn snapshot(&self) -> (f64, f64) {
        (
            self.fetches.load(Ordering::Relaxed) as f64,
            self.bytes.load(Ordering::Relaxed) as f64,
        )
    }
}

/// 소스 종류와 무관하게 reader 경계를 지나는 요청을 세는 래퍼.
#[derive(Debug)]
struct CountingSource {
    inner: Box<dyn engine::ByteSource>,
    counters: IoCounters,
}

impl engine::ByteSource for CountingSource {
    fn fetch(
        &self,
        range: std::ops::Range<u64>,
    ) -> engine::futures::future::BoxFuture<
        '_,
        std::result::Result<engine::bytes::Bytes, engine::SourceError>,
    > {
        self.counters.fetches.fetch_add(1, Ordering::Relaxed);
        self.counters
            .bytes
            .fetch_add(range.end - range.start, Ordering::Relaxed);
        self.inner.fetch(range)
    }
}

/// duckdb_error_data 를 소비해 메시지 문자열로 (없으면 fallback).
unsafe fn take_error_message(error_data: ffi::duckdb_error_data, fallback: &str) -> String {
    if error_data.is_null() {
        return fallback.to_string();
    }
    let mut error_data = error_data;
    let msg = if ffi::duckdb_error_data_has_error(error_data) {
        let c = ffi::duckdb_error_data_message(error_data);
        if c.is_null() {
            fallback.to_string()
        } else {
            std::ffi::CStr::from_ptr(c).to_string_lossy().into_owned()
        }
    } else {
        fallback.to_string()
    };
    ffi::duckdb_destroy_error_data(&mut error_data);
    msg
}

/// DuckDB FileHandle 소유 래퍼 — close/destroy 를 Drop 에 묶는다.
#[derive(Debug)]
struct FsHandle(ffi::duckdb_file_handle);

// SAFETY: 핸들 접근은 DuckDbFsSource 의 Mutex 로 전부 직렬화된다 (프로토타입 —
// seek+read 가 stateful 이라 동시성 자체가 불가능한 설계임을 실측 대상으로 남긴다).
unsafe impl Send for FsHandle {}

impl Drop for FsHandle {
    fn drop(&mut self) {
        unsafe {
            ffi::duckdb_file_handle_close(self.0);
            ffi::duckdb_destroy_file_handle(&mut self.0);
        }
    }
}

/// DuckDB FileSystem 위 [`engine::ByteSource`] — RFC §6.5 (b) 프로토타입.
///
/// httpfs·CREATE SECRET 등 DuckDB 생태계가 그대로 적용되는 대신, 핸들이
/// stateful(seek+read)이라 Mutex 직렬화가 필요하고 range 병렬성이 없다.
#[derive(Debug)]
struct DuckDbFsSource {
    path: String,
    handle: Mutex<FsHandle>,
    len: u64,
}

impl DuckDbFsSource {
    /// bind 단계 전용 — client context 는 bind info 를 통해서만 얻을 수 있다.
    ///
    /// duckdb-rs 는 raw `duckdb_bind_info` 를 노출하지 않아 single-field struct 를
    /// `transmute_copy` 로 읽는다 — §6.5(b) 결정 재료용 우회. 채택 시 duckdb-rs
    /// upstream 에 접근자 추가가 선행 조건이다 (보고서에 기록).
    fn open(bind: &BindInfo, path: &str) -> std::result::Result<Self, String> {
        const _: () = assert!(
            std::mem::size_of::<BindInfo>() == std::mem::size_of::<ffi::duckdb_bind_info>()
        );
        let cpath = CString::new(path).map_err(|e| e.to_string())?;
        unsafe {
            let info = std::mem::transmute_copy::<BindInfo, ffi::duckdb_bind_info>(bind);
            let mut ctx: ffi::duckdb_client_context = std::ptr::null_mut();
            ffi::duckdb_table_function_get_client_context(info, &mut ctx);
            if ctx.is_null() {
                return Err("client context unavailable".to_string());
            }
            let mut fs = ffi::duckdb_client_context_get_file_system(ctx);
            if fs.is_null() {
                ffi::duckdb_destroy_client_context(&mut ctx);
                return Err("file system unavailable via C-API".to_string());
            }
            let mut opts = ffi::duckdb_create_file_open_options();
            ffi::duckdb_file_open_options_set_flag(
                opts,
                ffi::duckdb_file_flag_DUCKDB_FILE_FLAG_READ,
                true,
            );
            let mut raw: ffi::duckdb_file_handle = std::ptr::null_mut();
            let state = ffi::duckdb_file_system_open(fs, cpath.as_ptr(), opts, &mut raw);
            let handle = if state == ffi::duckdb_state_DuckDBSuccess && !raw.is_null() {
                Ok(FsHandle(raw))
            } else {
                Err(take_error_message(
                    ffi::duckdb_file_system_error_data(fs),
                    "open failed",
                ))
            };
            ffi::duckdb_destroy_file_open_options(&mut opts);
            ffi::duckdb_destroy_file_system(&mut fs);
            ffi::duckdb_destroy_client_context(&mut ctx);
            let handle = handle.map_err(|e| format!("cannot open '{path}': {e}"))?;
            let len = ffi::duckdb_file_handle_size(handle.0);
            if len < 0 {
                return Err(format!("cannot size '{path}'"));
            }
            Ok(Self {
                path: path.to_string(),
                handle: Mutex::new(handle),
                len: len as u64,
            })
        }
    }
}

impl engine::ByteSource for DuckDbFsSource {
    fn fetch(
        &self,
        range: std::ops::Range<u64>,
    ) -> engine::futures::future::BoxFuture<
        '_,
        std::result::Result<engine::bytes::Bytes, engine::SourceError>,
    > {
        Box::pin(async move {
            let err = |msg: String| engine::SourceError(format!("{}: {msg}", self.path));
            if range.start > range.end || range.start >= self.len {
                return Err(err(format!(
                    "range {}..{} out of bounds (file len {})",
                    range.start, range.end, self.len
                )));
            }
            // EOF 클램프 (ByteSource 계약): end 만 넘으면 가용 분 반환
            let end = range.end.min(self.len);
            let mut buf = vec![0u8; (end - range.start) as usize];
            let handle = self.handle.lock().map_err(|e| err(e.to_string()))?;
            unsafe {
                if ffi::duckdb_file_handle_seek(handle.0, range.start as i64)
                    != ffi::duckdb_state_DuckDBSuccess
                {
                    return Err(err(take_error_message(
                        ffi::duckdb_file_handle_error_data(handle.0),
                        "seek failed",
                    )));
                }
                let mut off = 0usize;
                while off < buf.len() {
                    let n = ffi::duckdb_file_handle_read(
                        handle.0,
                        buf.as_mut_ptr().add(off).cast(),
                        (buf.len() - off) as i64,
                    );
                    if n < 0 {
                        return Err(err(take_error_message(
                            ffi::duckdb_file_handle_error_data(handle.0),
                            "read failed",
                        )));
                    }
                    if n == 0 {
                        return Err(err(format!(
                            "unexpected EOF at offset {}",
                            range.start + off as u64
                        )));
                    }
                    off += n as usize;
                }
            }
            Ok(engine::bytes::Bytes::from(buf))
        })
    }
}

/// xorshift64* 기반 [0,1) — 외부 rand 의존 없는 결정적 산개 포인트용.
fn next_unit(state: &mut u64) -> f64 {
    let mut x = *state;
    x ^= x << 13;
    x ^= x >> 7;
    x ^= x << 17;
    *state = x;
    (x >> 11) as f64 / (1u64 << 53) as f64
}

/// 엔진 워크로드 실행: open_cog(메타) → 산개 64점 read_pixels → 중앙 절반 창
/// zonal_stats. 각 단계의 (ms, fetch 델타, 바이트 델타)와 픽셀값 체크섬을 낸다.
fn run_workload(
    source: CountingSource,
    counters: &IoCounters,
) -> std::result::Result<Vec<(&'static str, f64)>, Box<dyn Error>> {
    use engine::futures::executor::block_on;
    let mut rows: Vec<(&'static str, f64)> = Vec::with_capacity(12);
    let phase = |rows: &mut Vec<(&'static str, f64)>,
                     names: [&'static str; 3],
                     t0: Instant,
                     before: (f64, f64)| {
        let (fetches, bytes) = counters.snapshot();
        rows.push((names[0], t0.elapsed().as_secs_f64() * 1000.0));
        rows.push((names[1], fetches - before.0));
        rows.push((names[2], bytes - before.1));
        (fetches, bytes)
    };

    let before = counters.snapshot();
    let t0 = Instant::now();
    let (meta, reader) = block_on(engine::open_cog(source))?;
    let before = phase(&mut rows, ["meta_ms", "meta_fetches", "meta_bytes"], t0, before);

    let g = meta
        .georef
        .as_ref()
        .ok_or("not georeferenced — bench workloads need world coordinates")?;
    let l0 = meta.levels.first().ok_or("no IFD levels")?;
    // Georef 규약: pixel_y 는 양수 저장, 북→남 진행은 감산 적용 (meta.rs)
    let x1 = g.origin_x + f64::from(l0.image_width) * g.pixel_x;
    let y1 = g.origin_y - f64::from(l0.image_height) * g.pixel_y;
    let (xmin, xmax) = (g.origin_x.min(x1), g.origin_x.max(x1));
    let (ymin, ymax) = (g.origin_y.min(y1), g.origin_y.max(y1));

    // 산개 포인트: 고정 시드 → 소스 종류와 무관하게 같은 픽셀 (체크섬 패리티 근거)
    let mut state = 0x9E37_79B9_7F4A_7C15_u64;
    let points: Vec<(f64, f64)> = (0..64)
        .map(|_| {
            let fx = next_unit(&mut state);
            let fy = next_unit(&mut state);
            (xmin + fx * (xmax - xmin), ymin + fy * (ymax - ymin))
        })
        .collect();
    let t0 = Instant::now();
    let values = block_on(reader.read_pixels(&meta, &points, 1))?;
    let before = phase(
        &mut rows,
        ["points_ms", "points_fetches", "points_bytes"],
        t0,
        before,
    );
    rows.push(("points_checksum", values.iter().flatten().sum()));

    // 중앙 절반 extent 창 — u16 픽셀의 f64 정수 합산은 순서 무관 정확 (< 2^53)
    let (qw, qh) = ((xmax - xmin) / 4.0, (ymax - ymin) / 4.0);
    let bbox = [xmin + qw, ymin + qh, xmax - qw, ymax - qh];
    let t0 = Instant::now();
    let stats = block_on(reader.zonal_stats(&meta, bbox, 1))?;
    phase(
        &mut rows,
        ["window_ms", "window_fetches", "window_bytes"],
        t0,
        before,
    );
    rows.push(("window_checksum", stats.sum));
    Ok(rows)
}

#[repr(C)]
pub struct IoBenchBindData {
    rows: Vec<(&'static str, f64)>,
}

#[repr(C)]
pub struct IoBenchInitData {
    done: AtomicBool,
}

/// `SELECT * FROM cog_io_bench(path, io := 'duckdb_fs');` → (metric, value) 행.
pub struct CogIoBenchVTab;

impl VTab for CogIoBenchVTab {
    type InitData = IoBenchInitData;
    type BindData = IoBenchBindData;

    fn bind(bind: &BindInfo) -> Result<Self::BindData, Box<dyn Error>> {
        bind.add_result_column("metric", LogicalTypeHandle::from(LogicalTypeId::Varchar));
        bind.add_result_column("value", LogicalTypeHandle::from(LogicalTypeId::Double));
        let path = bind.get_parameter(0).to_string();
        let io = match bind.get_named_parameter("io") {
            None => "auto".to_string(),
            Some(v) if v.is_null() => "auto".to_string(),
            Some(v) => v.to_string(),
        };
        let t0 = Instant::now();
        let inner: Box<dyn engine::ByteSource> = match io.as_str() {
            "auto" => open_source(&path).map_err(|e| format!("cog_io_bench: {e}"))?,
            "file" => Box::new(
                FileSource::open(&path)
                    .map_err(|e| format!("cog_io_bench: cannot open '{path}': {e}"))?,
            ),
            "object_store" => Box::new(
                ObjectStoreSource::open(&path).map_err(|e| format!("cog_io_bench: {e}"))?,
            ),
            "duckdb_fs" => Box::new(
                DuckDbFsSource::open(bind, &path).map_err(|e| format!("cog_io_bench: {e}"))?,
            ),
            other => {
                return Err(format!(
                    "cog_io_bench: unknown io '{other}' — expected file|object_store|duckdb_fs|auto"
                )
                .into())
            }
        };
        let open_ms = t0.elapsed().as_secs_f64() * 1000.0;
        let counters = IoCounters::default();
        let source = CountingSource {
            inner,
            counters: counters.clone(),
        };
        let mut rows = run_workload(source, &counters)
            .map_err(|e| format!("cog_io_bench: '{path}': {e}"))?;
        rows.push(("open_ms", open_ms));
        Ok(IoBenchBindData { rows })
    }

    fn init(_: &InitInfo) -> Result<Self::InitData, Box<dyn Error>> {
        Ok(IoBenchInitData {
            done: AtomicBool::new(false),
        })
    }

    fn func(
        func: &TableFunctionInfo<Self>,
        output: &mut DataChunkHandle,
    ) -> Result<(), Box<dyn Error>> {
        if func.get_init_data().done.swap(true, Ordering::Relaxed) {
            output.set_len(0);
            return Ok(());
        }
        let rows = &func.get_bind_data().rows;
        let metrics = output.flat_vector(0);
        for (i, (name, _)) in rows.iter().enumerate() {
            metrics.insert(i, *name);
        }
        // SAFETY: value 컬럼은 DOUBLE 로 선언 — f64 표현, 행 수는 12(≪ 벡터 용량).
        unsafe {
            let mut values = output.flat_vector(1);
            let values = values.as_mut_slice::<f64>();
            for (i, (_, v)) in rows.iter().enumerate() {
                values[i] = *v;
            }
        }
        output.set_len(rows.len());
        Ok(())
    }

    fn parameters() -> Option<Vec<LogicalTypeHandle>> {
        Some(vec![LogicalTypeHandle::from(LogicalTypeId::Varchar)])
    }

    fn named_parameters() -> Option<Vec<(String, LogicalTypeHandle)>> {
        Some(vec![(
            "io".to_string(),
            LogicalTypeHandle::from(LogicalTypeId::Varchar),
        )])
    }
}

pub(crate) fn register(con: &Connection) -> duckdb::Result<()> {
    con.register_table_function::<CogIoBenchVTab>("cog_io_bench")
}
