use std::collections::VecDeque;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use super::AudiocppError;
use super::server_config::ServerInstanceSpec;

/// 健康检查总 deadline（含 eager 模型加载）。实测冷启动 spawn+加载 1~3s，留足余量。
const READY_TIMEOUT_SECS: u32 = 20;
/// 单次 HTTP 探测超时。
const PROBE_TIMEOUT: Duration = Duration::from_secs(2);
/// 健康轮询间隔。
const PROBE_INTERVAL: Duration = Duration::from_millis(100);
/// stderr 环形缓冲行数（错误诊断附带末尾若干行）。
const STDERR_TAIL_LINES: usize = 20;

/// sidecar 进程租约：RAII 计数，Drop 时释放；计数归零按 keepalive 策略回收。
///
/// 携带所属实例的配置指纹与代际：多实例并存（模型族切换窗口）下，释放时
/// 精确路由到自己的实例。持有者（`AudiocppTts`）生命周期即租约生命周期：
/// voice 会话/Announcer 常驻则 server 常驻；GUI 每次合成取放，配合
/// `set_idle_keepalive(Some(45s))` 在窗口内复用热 server（热请求 0.13s 级）。
#[derive(Debug)]
pub struct ServerLease {
    port: u16,
    generation: u64,
    config_hash: u64,
}

impl ServerLease {
    /// server 基地址（`http://127.0.0.1:<port>`），HTTP 客户端直连用。
    pub fn base_url(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }
}

impl Drop for ServerLease {
    fn drop(&mut self) {
        release_lease(self.config_hash, self.generation);
    }
}

struct ServerInstance {
    child: Child,
    port: u16,
    stderr_tail: Arc<Mutex<VecDeque<String>>>,
    generation: u64,
    /// 本实例的 server config 落盘路径（按指纹分文件），回收时删除
    config_path: PathBuf,
}

/// 单个配置指纹对应的实例条目（实例 + 活跃租约数）。
struct InstanceEntry {
    inst: ServerInstance,
    leases: usize,
}

struct ManagerState {
    /// 按配置指纹并存的实例：模型族/后端切换窗口双实例共存（旧实例在租约
    /// 归零 + keepalive 后自然回收），热切换不杀正在合成的进程（技术方案 §4.6）。
    instances: std::collections::HashMap<u64, InstanceEntry>,
    orphan_reaped: bool,
}

static MANAGER: OnceLock<Mutex<ManagerState>> = OnceLock::new();
/// 空闲回收窗口（毫秒）：0 = lease 归零立即回收（CLI 语义，进程绝不残留）；
/// >0 = GUI keepalive 窗口（窗口内复用热 server）。
static IDLE_KEEPALIVE_MS: AtomicU64 = AtomicU64::new(0);
/// 实例代际：每次 spawn 递增；keepalive 线程复核时比对，避免误杀重新 spawn 的新实例。
static GENERATION: AtomicU64 = AtomicU64::new(0);

fn manager() -> &'static Mutex<ManagerState> {
    MANAGER.get_or_init(|| {
        Mutex::new(ManagerState {
            instances: std::collections::HashMap::new(),
            orphan_reaped: false,
        })
    })
}

/// 设置空闲回收窗口（宿主应用在启动时调用）。
///
/// GUI 传 `Some(45s)`；CLI 保持 `None`（缺省）即用即杀。
pub fn set_idle_keepalive(keepalive: Option<Duration>) {
    IDLE_KEEPALIVE_MS.store(
        keepalive.map_or(0, |d| d.as_millis().min(u64::MAX as u128) as u64),
        Ordering::SeqCst,
    );
}

/// 获取 server 租约：按配置指纹复用健康实例或 spawn（同指纹崩溃懒重启）。
///
/// manager 互斥锁串行化，避免并发 lease 双 spawn。**不同指纹的实例互不影响**
/// （并存直至各自租约归零回收）——切换模型时旧 server 继续服务在途请求，
/// 新 server 独立启动，热切换零中断。TTS 与 ASR 任务的指纹含 task 维度，
/// 两任务各自独立实例并存（路线 A，技术方案 §3.2）。
pub fn lease(spec: &ServerInstanceSpec) -> Result<ServerLease, AudiocppError> {
    let engine = super::locator::locate_engine(spec.engine_path.as_deref())?;
    let hash = config_hash(spec, &engine);

    let mut state = manager().lock().unwrap_or_else(|e| e.into_inner());
    if !state.orphan_reaped {
        state.orphan_reaped = true;
        reap_orphan_process();
    }

    // 已退出实例（崩溃）懒重启：只移除死实例，不动其他指纹的活实例
    let mut dead: Vec<u64> = Vec::new();
    for (key, entry) in state.instances.iter_mut() {
        if entry
            .inst
            .child
            .try_wait()
            .map(|s| s.is_some())
            .unwrap_or(true)
        {
            dead.push(*key);
        }
    }
    for key in dead {
        if let Some(mut entry) = state.instances.remove(&key) {
            tracing::warn!(
                target: "audiocpp",
                "server 已退出 (pid {})，移除实例",
                entry.inst.child.id()
            );
            kill_instance(&mut entry.inst);
        }
    }

    let entry = match state.instances.get_mut(&hash) {
        Some(entry) => entry,
        None => {
            let inst = spawn_instance(spec, &engine, hash)?;
            state
                .instances
                .insert(hash, InstanceEntry { inst, leases: 0 });
            state.instances.get_mut(&hash).expect("刚插入的条目必在")
        }
    };
    entry.leases += 1;
    Ok(ServerLease {
        port: entry.inst.port,
        generation: entry.inst.generation,
        config_hash: hash,
    })
}

/// 显式停止全部实例（幂等）。GUI 退出钩子 / CLI epilogue 调用。
pub fn shutdown_blocking() {
    let mut state = manager().lock().unwrap_or_else(|e| e.into_inner());
    for (_key, mut entry) in state.instances.drain() {
        kill_instance(&mut entry.inst);
    }
}

fn release_lease(config_hash: u64, generation: u64) {
    let keepalive_ms = IDLE_KEEPALIVE_MS.load(Ordering::SeqCst);
    let mut state = manager().lock().unwrap_or_else(|e| e.into_inner());
    let Some(entry) = state.instances.get_mut(&config_hash) else {
        return;
    };
    entry.leases = entry.leases.saturating_sub(1);
    if entry.leases > 0 {
        return;
    }
    if keepalive_ms == 0 {
        // CLI 语义：归零立即回收，进程绝不残留
        if let Some(mut entry) = state.instances.remove(&config_hash) {
            kill_instance(&mut entry.inst);
        }
        return;
    }
    // GUI 语义：延迟回收。sleep 后复核「计数仍为零且代际未变」，避免误杀
    // 窗口内新取的 lease / 崩溃重启的新实例。
    std::thread::Builder::new()
        .name("audiocpp-reaper".to_string())
        .spawn(move || {
            std::thread::sleep(Duration::from_millis(keepalive_ms));
            let mut state = manager().lock().unwrap_or_else(|e| e.into_inner());
            if let Some(entry) = state.instances.get(&config_hash)
                && entry.leases == 0
                && entry.inst.generation == generation
                && let Some(mut entry) = state.instances.remove(&config_hash)
            {
                kill_instance(&mut entry.inst);
            }
        })
        .ok();
}

/// 决定「复用哪个实例」的配置指纹：任务 / 模型目录 / 模型 id / 推理后端 / 线程数 / 引擎路径。
///
/// `task` 首字段（TTS 与 ASR 即使同 model_dir 也不撞指纹，双任务独立实例并存——
/// 路线 A 的隔离自证）；`model_id` 覆盖模型族角色（防 external 目录同 dir 不同
/// kind 的边角）。voice/音色不进指纹——音色随请求传（pocket 的 `voice` /
/// omnivoice 的 `voice_ref`），换音色复用热实例。
fn config_hash(spec: &ServerInstanceSpec, engine: &std::path::Path) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    spec.task.hash(&mut h);
    spec.model_dir.hash(&mut h);
    spec.model_id.hash(&mut h);
    spec.provider.hash(&mut h);
    spec.num_threads.hash(&mut h);
    engine.hash(&mut h);
    h.finish()
}

/// 分配空闲端口：`bind(("127.0.0.1", 0))` 取后释放（对齐 dsh 桥模式，无 rand 依赖）。
fn allocate_port() -> Result<u16, AudiocppError> {
    std::net::TcpListener::bind(("127.0.0.1", 0))
        .map_err(|e| AudiocppError::SpawnFailed(format!("分配端口失败: {e}")))?
        .local_addr()
        .map(|a| a.port())
        .map_err(|e| AudiocppError::SpawnFailed(format!("读取端口失败: {e}")))
}

fn spawn_instance(
    spec: &ServerInstanceSpec,
    engine: &std::path::Path,
    hash: u64,
) -> Result<ServerInstance, AudiocppError> {
    let port = allocate_port()?;
    let config_path = super::server_config::write_server_config(spec, port, hash)
        .map_err(AudiocppError::SpawnFailed)?;

    let mut cmd = Command::new(engine);
    cmd.arg("--config").arg(&config_path);
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
    // Windows：不弹控制台窗口
    #[cfg(target_os = "windows")]
    {
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }

    let mut child = cmd
        .spawn()
        .map_err(|e| AudiocppError::SpawnFailed(format!("{}: {e}", engine.display())))?;

    // stderr drain 线程：转发 tracing + 环形缓冲（错误诊断）
    let stderr_tail: Arc<Mutex<VecDeque<String>>> = Arc::new(Mutex::new(VecDeque::new()));
    if let Some(stderr) = child.stderr.take() {
        let tail = stderr_tail.clone();
        std::thread::Builder::new()
            .name("audiocpp-stderr".to_string())
            .spawn(move || {
                use std::io::BufRead;
                for line in std::io::BufReader::new(stderr)
                    .lines()
                    .map_while(Result::ok)
                {
                    tracing::debug!(target: "audiocpp", "{line}");
                    let mut buf = tail.lock().unwrap_or_else(|e| e.into_inner());
                    if buf.len() >= STDERR_TAIL_LINES {
                        buf.pop_front();
                    }
                    buf.push_back(line);
                }
            })
            .ok();
    }

    let generation = GENERATION.fetch_add(1, Ordering::SeqCst) + 1;
    write_pidfile(generation, child.id());
    let mut instance = ServerInstance {
        child,
        port,
        stderr_tail,
        generation,
        config_path,
    };

    // 健康检查失败 → 回收进程再返回错误（不留半启动实例）
    if let Err(e) = wait_until_ready(&mut instance, &spec.model_id) {
        kill_instance(&mut instance);
        return Err(e);
    }
    tracing::info!(target: "audiocpp", "audiocpp_server 就绪 (port {port}, generation {generation})");
    Ok(instance)
}

/// 轮询直到 `/health` 200 且 `/v1/models` 列出目标模型（eager 模式下含模型加载），
/// 或超时/进程退出。`model_id` 来自模型族描述表（多族下按实例校验）。
fn wait_until_ready(inst: &mut ServerInstance, model_id: &str) -> Result<(), AudiocppError> {
    let client = reqwest::blocking::Client::builder()
        .timeout(PROBE_TIMEOUT)
        // 回环探测不走系统代理（代理会拦 localhost 请求返回 5xx）
        .no_proxy()
        .build()
        .map_err(|e| AudiocppError::Connection(e.to_string()))?;
    let health_url = format!("http://127.0.0.1:{}/health", inst.port);
    let models_url = format!("http://127.0.0.1:{}/v1/models", inst.port);
    let deadline = Instant::now() + Duration::from_secs(READY_TIMEOUT_SECS as u64);

    loop {
        if inst.child.try_wait().map(|s| s.is_some()).unwrap_or(true) {
            return Err(AudiocppError::SpawnFailed(format!(
                "audiocpp_server 启动后立即退出。引擎输出末尾：\n{}",
                tail_string(&inst.stderr_tail)
            )));
        }
        let healthy = client
            .get(&health_url)
            .send()
            .map(|r| r.status().is_success())
            .unwrap_or(false);
        if healthy {
            let listed = client
                .get(&models_url)
                .send()
                .ok()
                .and_then(|r| r.json::<serde_json::Value>().ok())
                .and_then(|j| {
                    j["data"]
                        .as_array()
                        .map(|a| a.iter().any(|m| m["id"].as_str() == Some(model_id)))
                })
                .unwrap_or(false);
            if listed {
                return Ok(());
            }
        }
        if Instant::now() >= deadline {
            return Err(AudiocppError::StartupTimeout {
                timeout_secs: READY_TIMEOUT_SECS,
                stderr_tail: tail_string(&inst.stderr_tail),
            });
        }
        std::thread::sleep(PROBE_INTERVAL);
    }
}

fn kill_instance(inst: &mut ServerInstance) {
    let generation = inst.generation;
    let _ = inst.child.kill();
    let _ = inst.child.wait();
    remove_pidfile(generation);
    // 按指纹分文件的 server config 随实例回收（见 server_config::server_config_path）
    let _ = std::fs::remove_file(&inst.config_path);
}

fn tail_string(tail: &Arc<Mutex<VecDeque<String>>>) -> String {
    let buf = tail.lock().unwrap_or_else(|e| e.into_inner());
    if buf.is_empty() {
        "(无输出)".to_string()
    } else {
        buf.iter().cloned().collect::<Vec<_>>().join("\n")
    }
}

/// pidfile（按实例代际命名，多实例并存互不覆盖）：`<data_dir>/engines/audiocpp-server-<gen>.pid`。
fn pidfile_path(generation: u64) -> PathBuf {
    super::locator::engines_dir().join(format!("audiocpp-server-{generation}.pid"))
}

fn write_pidfile(generation: u64, pid: u32) {
    let _ = std::fs::create_dir_all(super::locator::engines_dir());
    let _ = std::fs::write(pidfile_path(generation), pid.to_string());
}

fn remove_pidfile(generation: u64) {
    let _ = std::fs::remove_file(pidfile_path(generation));
}

/// 孤儿清理（宿主崩溃/强杀兜底，对齐 dsh「残留→下次启动清理」模式）：
/// 扫描 engines 目录下全部 `audiocpp-server-*.pid`，pid 存活且进程名匹配
/// `audiocpp_server` 时 kill。不做复用（manager 总是按当前配置重新生成
/// config 再 spawn）。
fn reap_orphan_process() {
    let Ok(entries) = std::fs::read_dir(super::locator::engines_dir()) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if !(name.starts_with("audiocpp-server-") && name.ends_with(".pid")) {
            continue;
        }
        let Ok(content) = std::fs::read_to_string(entry.path()) else {
            continue;
        };
        let Ok(pid) = content.trim().parse::<u32>() else {
            let _ = std::fs::remove_file(entry.path());
            continue;
        };
        let sys_pid = sysinfo::Pid::from_u32(pid);
        let mut sys = sysinfo::System::new();
        sys.refresh_processes(sysinfo::ProcessesToUpdate::Some(&[sys_pid]), true);
        if let Some(proc) = sys.process(sys_pid) {
            let pname = proc.name().to_string_lossy().to_string();
            if pname.contains("audiocpp_server") {
                tracing::warn!(target: "audiocpp", "发现残留 audiocpp_server 进程 (pid {pid})，正在清理");
                let _ = proc.kill();
                std::thread::sleep(Duration::from_millis(200));
            }
        }
        let _ = std::fs::remove_file(entry.path());
    }
    // 残留的按指纹分文件 server config 一并清理：reap 发生在本进程首次 spawn
    // 之前，目录下现存 config 必属于已退出/刚被杀的实例（运行中的 server 只在
    // 启动时读一次 config，删除不影响本进程后续重新生成）
    if let Ok(entries) = std::fs::read_dir(super::locator::engines_dir()) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with("audiocpp-server-") && name.ends_with(".json") {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_allocate_port_returns_nonzero() {
        let p = allocate_port().unwrap();
        assert!(p > 0);
    }

    // ---------- 全链路（python3 stub 引擎，unix-only：覆盖率在 ubuntu 跑） ----------

    /// 写一个最小 stub audiocpp_server（python3 http server）：解析 `--config <path>`
    /// 的 json 取端口，实现 /health、/v1/models、/v1/audio/speech（返回固定 wav）。
    /// 放入固定临时目录并注入 SEARCH_DIRS（OnceLock 全局一次，目录固定保证幂等）。
    #[cfg(all(unix, not(target_os = "macos")))]
    fn setup_stub_engine() -> std::path::PathBuf {
        let dir = std::env::temp_dir().join("zapmomo-audiocpp-stub-test");
        std::fs::create_dir_all(&dir).unwrap();
        let script = r#"#!/usr/bin/env python3
import sys, json, struct
cfg = json.load(open(sys.argv[sys.argv.index('--config') + 1]))
port = cfg['port']
import http.server
class H(http.server.BaseHTTPRequestHandler):
    def _ok(self, b):
        self.send_response(200)
        self.send_header('Content-Length', str(len(b)))
        self.end_headers()
        self.wfile.write(b)
    def do_GET(self):
        if self.path == '/health':
            self._ok(b'{"status":"ok"}')
        elif self.path == '/v1/models':
            self._ok(json.dumps({"data": [{"id": "pocket-tts-english"}, {"id": "omnivoice"}]}).encode())
        else:
            self.send_error(404)
    def do_POST(self):
        if self.path == '/v1/audio/speech':
            sr, n = 24000, 2400
            data = b'\x00\x01' * n
            hdr = b'RIFF' + struct.pack('<I', 36 + len(data)) + b'WAVEfmt ' \
                + struct.pack('<IHHIIHH', 16, 1, 1, sr, sr * 2, 2, 16) \
                + b'data' + struct.pack('<I', len(data))
            self._ok(hdr + data)
        else:
            self.send_error(404)
    def log_message(self, *a):
        pass
http.server.HTTPServer(('127.0.0.1', port), H).serve_forever()
"#;
        let exe = dir.join(super::super::locator::engine_file_name());
        std::fs::write(&exe, script).unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
        super::super::locator::set_search_dirs(vec![dir.clone()]);
        dir
    }

    /// audiocpp 后端测试配置（HOME 隔离 + pocket 两文件齐）。
    #[cfg(all(unix, not(target_os = "macos")))]
    fn stub_ready_cfg(home: &std::path::Path) -> crate::tts::config::ResolvedTtsConfig {
        crate::test_util::set_custom_data_dir(home);
        let model_dir = home.join("models/pocket-stub");
        std::fs::create_dir_all(model_dir.join("embeddings")).unwrap();
        std::fs::write(
            model_dir.join(super::super::families::POCKET.gguf_file),
            b"x",
        )
        .unwrap();
        std::fs::write(model_dir.join("embeddings/alba.safetensors"), b"x").unwrap();
        let mut cfg = crate::tts::config::ResolvedTtsConfig::default();
        cfg.backend = crate::tts::config::TtsBackendKind::Audiocpp;
        cfg.model_type = crate::tts::config::TtsModelKind::Pocket;
        cfg.model_dir = model_dir;
        cfg
    }

    /// 是否存在任一实例 pidfile（多实例下按前缀探测；目录在 HOME 隔离下随 data_dir 走）。
    #[cfg(all(unix, not(target_os = "macos")))]
    fn stub_pidfile_exists() -> bool {
        let Ok(entries) = std::fs::read_dir(super::super::locator::engines_dir()) else {
            return false;
        };
        entries.flatten().any(|e| {
            e.file_name()
                .to_string_lossy()
                .starts_with("audiocpp-server-")
        })
    }

    #[test]
    #[cfg(all(unix, not(target_os = "macos")))]
    fn test_lease_lifecycle_with_stub_engine() {
        crate::test_util::run_with_temp_home(|home| {
            setup_stub_engine();
            let cfg = stub_ready_cfg(home);
            let spec = ServerInstanceSpec::from_tts(&cfg).unwrap();

            // 首个 lease：spawn stub → 健康检查 → 模型在列
            let l1 = lease(&spec).expect("lease 应成功（stub 引擎健康）");
            let url = l1.base_url();
            assert!(url.starts_with("http://127.0.0.1:"), "url: {url}");
            assert!(stub_pidfile_exists(), "pidfile 应写入");

            // 第二个 lease 复用同一实例（计数 +1，不重复 spawn）
            let l2 = lease(&spec).expect("第二个 lease 复用实例");
            assert_eq!(l2.base_url(), url);

            // keepalive=None（测试环境缺省）：lease 全部释放后立即回收
            drop(l1);
            drop(l2);
            assert!(!stub_pidfile_exists(), "归零应立即回收（CLI 语义）");

            // 显式 shutdown 幂等
            shutdown_blocking();
            shutdown_blocking();
        });
    }

    #[test]
    #[cfg(all(unix, not(target_os = "macos")))]
    fn test_idle_keepalive_delays_reaping() {
        crate::test_util::run_with_temp_home(|home| {
            setup_stub_engine();

            set_idle_keepalive(Some(Duration::from_millis(400)));
            let spec = ServerInstanceSpec::from_tts(&stub_ready_cfg(home)).unwrap();
            let l = lease(&spec).expect("lease 应成功");
            drop(l);
            // 保活窗口内进程仍存活（pidfile 未删）
            assert!(stub_pidfile_exists(), "保活窗口内不应回收");
            // 窗口过后 reaper 回收
            std::thread::sleep(Duration::from_millis(700));
            assert!(!stub_pidfile_exists(), "窗口过后应回收");
            set_idle_keepalive(None);
            shutdown_blocking();
        });
    }

    #[test]
    #[cfg(all(unix, not(target_os = "macos")))]
    fn test_client_synthesize_via_stub_engine() {
        crate::test_util::run_with_temp_home(|home| {
            setup_stub_engine();
            let cfg = stub_ready_cfg(home);

            // 生产构造全链路：AudiocppTts::new → lease → POST /v1/audio/speech → wav 解码
            let tts = super::super::client::AudiocppTts::new(cfg.clone())
                .expect("生产构造应成功（stub 健康且模型在列）");
            let samples = tts
                .synthesize("hello", 1.0, &crate::tts::TtsVoiceParams::Sid(0))
                .expect("stub 合成应成功");
            assert_eq!(samples.len(), 2400, "stub 返回 2400 样本（静音）");
            assert_eq!(tts.sample_rate(), 24000, "首响应校准采样率");
            shutdown_blocking();
        });
    }

    /// TtsEngine 门面 audiocpp 臂：音色参数三态 / 进度回调取消 / 落盘。
    #[test]
    #[cfg(all(unix, not(target_os = "macos")))]
    fn test_engine_facade_audiocpp_arm() {
        crate::test_util::run_with_temp_home(|home| {
            setup_stub_engine();
            let cfg = stub_ready_cfg(home);
            let engine = crate::tts::TtsEngine::new(cfg.clone())
                .expect("门面 audiocpp 构造（内部 lease stub）");
            assert_eq!(engine.sample_rate(), 24_000, "初值 PocketTTS 固定采样率");

            // Named 音色合成成功（请求体 voice=alba）
            let out = engine
                .synthesize(
                    "hello",
                    1.0,
                    &crate::tts::TtsVoiceParams::Named("alba".into()),
                )
                .unwrap();
            assert_eq!(out.len(), 2400);

            // Reference 在 audiocpp 臂报错（连接前拦截）
            let err = engine
                .synthesize(
                    "x",
                    1.0,
                    &crate::tts::TtsVoiceParams::Reference {
                        wav_path: std::path::PathBuf::from("/r.wav"),
                        reference_text: "t".into(),
                    },
                )
                .unwrap_err();
            assert!(err.contains("固定音色"), "err: {err}");

            // 进度回调返回 false → 请求前取消（不发请求）
            let err = engine
                .synthesize_with_progress("x", 1.0, &crate::tts::TtsVoiceParams::Sid(0), |_| false)
                .unwrap_err();
            assert_eq!(err, "已取消");

            // 落盘 + 进度全流程
            let wav = home.join("out.wav");
            let n = engine
                .synthesize_to_wav_with_progress(
                    "hello",
                    1.0,
                    &crate::tts::TtsVoiceParams::Sid(0),
                    &wav,
                    |p| p < 0.5,
                )
                .unwrap();
            assert_eq!(n, 2400);
            assert!(wav.is_file());
            shutdown_blocking();
        });
    }

    /// 坏 stub（立即退出）→ SpawnFailed「启动后立即退出」错误分支。
    #[test]
    #[cfg(all(unix, not(target_os = "macos")))]
    fn test_lease_fails_when_engine_exits_immediately() {
        crate::test_util::run_with_temp_home(|home| {
            // 覆盖 setup_stub_engine 已注入的搜索目录：直接换掉脚本内容为退出脚本。
            // SEARCH_DIRS 指向固定目录，覆盖同名文件即生效。
            let dir = std::env::temp_dir().join("zapmomo-audiocpp-stub-test");
            std::fs::create_dir_all(&dir).unwrap();
            let exe = dir.join(super::super::locator::engine_file_name());
            std::fs::write(&exe, "#!/bin/sh\nexit 3\n").unwrap();
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
            super::super::locator::set_search_dirs(vec![dir]);

            let cfg = stub_ready_cfg(home);
            let err = lease(&ServerInstanceSpec::from_tts(&cfg).unwrap()).unwrap_err();
            let msg = err.to_user_message();
            assert!(msg.contains("启动后立即退出"), "msg: {msg}");
            shutdown_blocking();

            // 恢复好 stub（供后续测试使用）
            setup_stub_engine();
        });
    }

    /// 多实例并存：不同配置指纹各起实例、互不误杀；释放一个不影响另一个。
    #[test]
    #[cfg(all(unix, not(target_os = "macos")))]
    fn test_multi_instance_two_configs_coexist() {
        crate::test_util::run_with_temp_home(|home| {
            setup_stub_engine();
            let cfg_a = stub_ready_cfg(home);
            // 实例 B：仅模型目录不同（指纹不同；stub 引擎不读模型文件）
            let mut cfg_b = cfg_a.clone();
            cfg_b.model_dir = home.join("models/pocket-stub-b");
            let spec_a = ServerInstanceSpec::from_tts(&cfg_a).unwrap();
            let spec_b = ServerInstanceSpec::from_tts(&cfg_b).unwrap();

            let la = lease(&spec_a).expect("实例 A 应起");
            let lb = lease(&spec_b).expect("实例 B 应起");
            assert_ne!(la.base_url(), lb.base_url(), "不同指纹应各起实例");

            // 释放 A（keepalive=0 → 立即回收）不影响 B
            drop(la);
            assert!(stub_pidfile_exists(), "实例 B 应仍存活");
            drop(lb);
            assert!(!stub_pidfile_exists(), "全部释放后应回收干净");
            shutdown_blocking();
        });
    }

    #[test]
    fn test_config_hash_distinguishes_dimensions() {
        let mut cfg = crate::tts::config::ResolvedTtsConfig::default();
        cfg.model_type = crate::tts::config::TtsModelKind::Pocket;
        let engine = std::path::Path::new("/engines/audiocpp_server");
        let spec = ServerInstanceSpec::from_tts(&cfg).unwrap();
        let h1 = config_hash(&spec, engine);
        // 模型目录变更 → 指纹必变
        cfg.model_dir = std::path::PathBuf::from("/models/other");
        let spec2 = ServerInstanceSpec::from_tts(&cfg).unwrap();
        let h2 = config_hash(&spec2, engine);
        assert_ne!(h1, h2);
        // 模型族变更 → 指纹必变（即使 external 目录同名）
        cfg.model_type = crate::tts::config::TtsModelKind::Omnivoice;
        let spec3 = ServerInstanceSpec::from_tts(&cfg).unwrap();
        let h3 = config_hash(&spec3, engine);
        assert_ne!(h2, h3);
        // 同配置幂等
        assert_eq!(h3, config_hash(&spec3, engine));
        // task 维度：同目录同模型 id，task 不同 → 指纹必变（TTS/ASR 双实例隔离自证）
        let mut spec_asr = spec3.clone();
        spec_asr.task = "asr";
        assert_ne!(config_hash(&spec_asr, engine), h3);
    }

    #[test]
    fn test_set_idle_keepalive_stores_ms() {
        set_idle_keepalive(None);
        assert_eq!(IDLE_KEEPALIVE_MS.load(Ordering::SeqCst), 0);
        set_idle_keepalive(Some(Duration::from_millis(45_000)));
        assert_eq!(IDLE_KEEPALIVE_MS.load(Ordering::SeqCst), 45_000);
        set_idle_keepalive(None);
    }

    #[test]
    fn test_shutdown_blocking_is_idempotent_when_no_instance() {
        // 无实例时调用不 panic（退出钩子可能在任何状态下触发）
        shutdown_blocking();
        shutdown_blocking();
    }
}
