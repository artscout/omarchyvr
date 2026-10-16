//! The local AI of Pimax voice control (`pimax-ai`): llama.cpp's `llama-server` plus a small tool-calling model.
//!
//! * `install`: downloads the pinned runtime (the official GitHub release of ggml-org/llama.cpp, Vulkan build for
//!   Linux x64, which also runs on the CPU) and the pinned models (Hugging Face, unsloth's GGUF quantisations of
//!   Qwen3.5) into `~/.local/share/pimax/ai/`, resuming partial downloads (`curl -C -`) and checking each file's
//!   SHA-256 against the value published by GitHub (release asset digest) or Hugging Face (LFS oid).
//! * `serve`: `llama-server` on 127.0.0.1 only, `nice 19` + idle I/O class + low thread priority, one slot, 8k
//!   context, tool calling through the model's own chat template (`--jinja`), thinking off. GPU layers are an
//!   option (`all` by default, `0` = CPU only with the small CPU model).
//! * `status`, `stop` (the PID in `~/.local/state/pimax/ai/llama-server.pid`, checked to be a llama-server).
//!
//! GPU memory budget (RTX 4090, 24 GiB, next to a VR session): Qwen3.5-4B Q4_K_M weights 2.6 GiB + 8k context
//! (hybrid linear attention: a small KV cache) + compute buffers: about 3.5 GiB, documented limit 4 GiB.
//! The CPU fallback (Qwen3.5-2B Q4_K_M, 1.2 GiB in RAM) uses no GPU memory at all (`--device none`).
//!
//! Everything goes through small helpers over `curl`, `tar`, `sha256sum`, `nice`, `ionice` (no network crates).

use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

/// The llama.cpp release in use.
pub const LLAMA_TAG: &str = "b11630";
/// The default port (`pimax_config::ai::LLAMA_SERVER_BASE` = http://127.0.0.1:8080/v1).
pub const DEFAULT_PORT: u16 = 8080;
/// The model name the server answers to (`model` in ai.toml).
pub const MODEL_ALIAS: &str = "pimax-local";
/// The context size.
pub const DEFAULT_CTX: u32 = 8192;
/// Documented GPU memory budget of the server (MiB).
pub const GPU_BUDGET_MIB: u32 = 4096;

/// What a download is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssetKind {
    Runtime,
    Model,
}

/// A pinned download.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Asset {
    /// Short id (`runtime-vulkan`, `model-gpu`, ...).
    pub id: &'static str,
    pub kind: AssetKind,
    /// The file name on disk.
    pub file: &'static str,
    pub url: &'static str,
    /// Lower-case hex SHA-256, as published by the source.
    pub sha256: &'static str,
    pub size: u64,
    /// For people.
    pub label: &'static str,
}

/// llama.cpp b11630, Vulkan build (NVIDIA through the Vulkan driver; no CUDA runtime needed; CPU backends included).
pub const RUNTIME_VULKAN: Asset = Asset {
    id: "runtime-vulkan",
    kind: AssetKind::Runtime,
    file: "llama-b11630-bin-ubuntu-vulkan-x64.tar.gz",
    url: "https://github.com/ggml-org/llama.cpp/releases/download/b11630/llama-b11630-bin-ubuntu-vulkan-x64.tar.gz",
    sha256: "fa8110aaf6a91dbf8b9f04a0a533c0cc2006692d157aef8e471268c25168fbf0",
    size: 30_416_480,
    label: "llama.cpp b11630 (Vulkan + CPU), 29 MiB",
};

/// llama.cpp b11630, CPU-only build (for machines without Vulkan).
pub const RUNTIME_CPU: Asset = Asset {
    id: "runtime-cpu",
    kind: AssetKind::Runtime,
    file: "llama-b11630-bin-ubuntu-x64.tar.gz",
    url: "https://github.com/ggml-org/llama.cpp/releases/download/b11630/llama-b11630-bin-ubuntu-x64.tar.gz",
    sha256: "112e75af7718aeb3a3bb54d563b7d44e104a1734a9bb9657fd4a3930976c4bbd",
    size: 17_789_649,
    label: "llama.cpp b11630 (CPU only), 17 MiB",
};

/// The GPU model: Qwen3.5-4B, Q4_K_M (unsloth GGUF, revision e87f176).
pub const MODEL_GPU: Asset = Asset {
    id: "model-gpu",
    kind: AssetKind::Model,
    file: "Qwen3.5-4B-Q4_K_M.gguf",
    url: "https://huggingface.co/unsloth/Qwen3.5-4B-GGUF/resolve/e87f176479d0855a907a41277aca2f8ee7a09523/Qwen3.5-4B-Q4_K_M.gguf",
    sha256: "00fe7986ff5f6b463e62455821146049db6f9313603938a70800d1fb69ef11a4",
    size: 2_740_937_888,
    label: "Qwen3.5-4B Q4_K_M, 2.6 GiB (GPU)",
};

/// The CPU fallback model: Qwen3.5-2B, Q4_K_M (unsloth GGUF, revision f6d5376).
pub const MODEL_CPU: Asset = Asset {
    id: "model-cpu",
    kind: AssetKind::Model,
    file: "Qwen3.5-2B-Q4_K_M.gguf",
    url: "https://huggingface.co/unsloth/Qwen3.5-2B-GGUF/resolve/f6d5376be1edb4d416d56da11e5397a961aca8ae/Qwen3.5-2B-Q4_K_M.gguf",
    sha256: "aaf42c8b7c3cab2bf3d69c355048d4a0ee9973d48f16c731c0520ee914699223",
    size: 1_280_835_840,
    label: "Qwen3.5-2B Q4_K_M, 1.2 GiB (CPU fallback)",
};

/// Every known asset.
pub const ASSETS: [Asset; 4] = [RUNTIME_VULKAN, RUNTIME_CPU, MODEL_GPU, MODEL_CPU];

/// Where things live.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dirs {
    /// `~/.local/share/pimax/ai`.
    pub data: PathBuf,
    /// `~/.local/state/pimax/ai` (pid file).
    pub state: PathBuf,
    /// `~/.local/state/pimax/logs` (server log).
    pub logs: PathBuf,
}

impl Dirs {
    /// From the environment (`XDG_DATA_HOME`, `XDG_STATE_HOME`, `HOME`).
    pub fn from_env(env: &dyn Fn(&str) -> Option<String>) -> Dirs {
        let home = PathBuf::from(env("HOME").unwrap_or_else(|| "/tmp".into()));
        let xdg = |k: &str, d: &str| env(k).filter(|v| v.starts_with('/')).map(PathBuf::from).unwrap_or_else(|| home.join(d));
        let data = xdg("XDG_DATA_HOME", ".local/share").join("pimax/ai");
        let st = xdg("XDG_STATE_HOME", ".local/state").join("pimax");
        Dirs { data, state: st.join("ai"), logs: st.join("logs") }
    }

    pub fn home() -> Dirs {
        Dirs::from_env(&|k| std::env::var(k).ok())
    }

    pub fn downloads(&self) -> PathBuf {
        self.data.join("downloads")
    }
    pub fn models(&self) -> PathBuf {
        self.data.join("models")
    }
    /// `runtime/<asset id>-<tag>/` (the extracted release).
    pub fn runtime(&self, a: &Asset) -> PathBuf {
        self.data.join("runtime").join(format!("{}-{LLAMA_TAG}", a.id))
    }
    pub fn pid_file(&self) -> PathBuf {
        self.state.join("llama-server.pid")
    }
    pub fn server_log(&self) -> PathBuf {
        self.logs.join("llama-server.log")
    }
    pub fn model_path(&self, a: &Asset) -> PathBuf {
        self.models().join(a.file)
    }
}

// ------------------------------------------------------------------------------------------------------ install

/// `sha256sum FILE` -> the hex digest.
pub fn sha256_file(p: &Path) -> Result<String, String> {
    let out = Command::new("sha256sum").arg(p).output().map_err(|e| format!("sha256sum: {e}"))?;
    if !out.status.success() {
        return Err(format!("sha256sum {}: {}", p.display(), String::from_utf8_lossy(&out.stderr).trim()));
    }
    parse_sha256sum(&String::from_utf8_lossy(&out.stdout)).ok_or_else(|| "sha256sum: unexpected output".into())
}

/// The digest of a `sha256sum` line.
pub fn parse_sha256sum(line: &str) -> Option<String> {
    let h = line.split_whitespace().next()?.to_ascii_lowercase();
    (h.len() == 64 && h.chars().all(|c| c.is_ascii_hexdigit())).then_some(h)
}

/// The `curl` arguments of one resumable download (`-C -` continues a `.part` file).
pub fn curl_args(a: &Asset, part: &Path) -> Vec<String> {
    vec![
        "-L".into(),
        "--fail".into(),
        "--proto".into(),
        "=https".into(),
        "--retry".into(),
        "5".into(),
        "--retry-delay".into(),
        "3".into(),
        "-C".into(),
        "-".into(),
        "--progress-bar".into(),
        "-o".into(),
        part.to_string_lossy().into_owned(),
        a.url.into(),
    ]
}

/// Download one asset into `dest` (a `.part` file alongside, resumed), check size and SHA-256, then rename.
/// Already present with the right digest: nothing to do. Returns the final path.
pub fn fetch(a: &Asset, dest: &Path, log: &mut dyn FnMut(&str)) -> Result<PathBuf, String> {
    if let Some(d) = dest.parent() {
        std::fs::create_dir_all(d).map_err(|e| format!("{}: {e}", d.display()))?;
    }
    if file_size(dest) == Some(a.size) {
        log(&format!("{}: present, checking SHA-256", a.file));
        if sha256_file(dest)? == a.sha256 {
            return Ok(dest.to_path_buf());
        }
        log(&format!("{}: digest mismatch, downloading again", a.file));
        std::fs::remove_file(dest).map_err(|e| e.to_string())?;
    }
    let part = PathBuf::from(format!("{}.part", dest.display()));
    match file_size(&part) {
        Some(n) if n > a.size => {
            std::fs::remove_file(&part).map_err(|e| e.to_string())?;
        }
        Some(n) if n == a.size => log(&format!("{}: download complete earlier", a.file)),
        Some(n) => log(&format!("{}: resuming at {} MiB of {} MiB", a.file, n >> 20, a.size >> 20)),
        None => log(&format!("{}: downloading {} MiB from {}", a.file, a.size >> 20, a.url)),
    }
    if file_size(&part) != Some(a.size) {
        let st = Command::new("curl").args(curl_args(a, &part)).status().map_err(|e| format!("curl: {e}"))?;
        if !st.success() {
            return Err(format!("{}: curl failed ({st}); run install again to resume", a.file));
        }
    }
    let got = file_size(&part).unwrap_or(0);
    if got != a.size {
        return Err(format!("{}: {got} bytes, expected {}; run install again to resume", a.file, a.size));
    }
    let h = sha256_file(&part)?;
    if h != a.sha256 {
        let _ = std::fs::remove_file(&part);
        return Err(format!("{}: SHA-256 {h} does not match the published {}; the file was deleted", a.file, a.sha256));
    }
    std::fs::rename(&part, dest).map_err(|e| e.to_string())?;
    log(&format!("{}: OK (SHA-256 verified)", a.file));
    Ok(dest.to_path_buf())
}

fn file_size(p: &Path) -> Option<u64> {
    std::fs::metadata(p).ok().filter(|m| m.is_file()).map(|m| m.len())
}

/// Find `llama-server` below a directory (release tarballs nest it, e.g. `build/bin/`).
pub fn find_server(dir: &Path) -> Option<PathBuf> {
    let rd = std::fs::read_dir(dir).ok()?;
    let mut subdirs = Vec::new();
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            subdirs.push(p);
        } else if p.file_name().and_then(|n| n.to_str()) == Some("llama-server") {
            return Some(p);
        }
    }
    subdirs.sort();
    subdirs.iter().find_map(|d| find_server(d))
}

/// Unpack a runtime tarball into its directory (atomic: a temporary directory renamed at the end).
pub fn unpack_runtime(a: &Asset, tarball: &Path, dirs: &Dirs) -> Result<PathBuf, String> {
    let dest = dirs.runtime(a);
    if let Some(s) = find_server(&dest) {
        return Ok(s);
    }
    let tmp = PathBuf::from(format!("{}.tmp", dest.display()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).map_err(|e| format!("{}: {e}", tmp.display()))?;
    let st = Command::new("tar")
        .args(["-xzf"])
        .arg(tarball)
        .arg("-C")
        .arg(&tmp)
        .args(["--no-same-owner", "--no-same-permissions"])
        .status()
        .map_err(|e| format!("tar: {e}"))?;
    if !st.success() {
        return Err(format!("tar failed on {}", tarball.display()));
    }
    if find_server(&tmp).is_none() {
        return Err(format!("{}: no llama-server inside", tarball.display()));
    }
    let _ = std::fs::remove_dir_all(&dest);
    std::fs::rename(&tmp, &dest).map_err(|e| e.to_string())?;
    find_server(&dest).ok_or_else(|| "llama-server vanished".into())
}

/// What `install` fetches.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InstallPlan {
    pub runtime: Asset,
    pub gpu_model: bool,
    pub cpu_model: bool,
}

impl InstallPlan {
    pub fn assets(&self) -> Vec<Asset> {
        let mut v = vec![self.runtime];
        if self.gpu_model {
            v.push(MODEL_GPU);
        }
        if self.cpu_model {
            v.push(MODEL_CPU);
        }
        v
    }
    pub fn bytes(&self) -> u64 {
        self.assets().iter().map(|a| a.size).sum()
    }
}

/// Run an install plan.
pub fn install(plan: &InstallPlan, dirs: &Dirs, log: &mut dyn FnMut(&str)) -> Result<(), String> {
    for a in plan.assets() {
        match a.kind {
            AssetKind::Runtime => {
                let tb = fetch(&a, &dirs.downloads().join(a.file), log)?;
                let s = unpack_runtime(&a, &tb, dirs)?;
                log(&format!("runtime: {}", s.display()));
            }
            AssetKind::Model => {
                fetch(&a, &dirs.model_path(&a), log)?;
            }
        }
    }
    Ok(())
}

// -------------------------------------------------------------------------------------------------------- serve

/// How to serve.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServeOpts {
    /// GPU layers: None = all (`-ngl 999`), Some(0) = CPU only.
    pub gpu_layers: Option<u32>,
    pub port: u16,
    pub ctx: u32,
    /// CPU threads (CPU mode; GPU mode uses a few for sampling).
    pub threads: u32,
    /// A model file instead of the default for the mode.
    pub model: Option<PathBuf>,
}

impl Default for ServeOpts {
    fn default() -> Self {
        ServeOpts { gpu_layers: None, port: DEFAULT_PORT, ctx: DEFAULT_CTX, threads: default_threads(), model: None }
    }
}

/// CPU threads by default: 3/4 of the hardware threads, 4..24 (24 on the 32-thread desktop: the voice AI bench of
/// docs/AI.md section 11 was fastest there, with no effect on VR frame timing at nice 19).
pub fn default_threads() -> u32 {
    let n = std::thread::available_parallelism().map(|n| n.get() as u32).unwrap_or(8);
    (n * 3 / 4).clamp(4, 24)
}

impl ServeOpts {
    pub fn cpu(&self) -> bool {
        self.gpu_layers == Some(0)
    }
}

/// The installed runtime (Vulkan first, then the CPU build).
pub fn installed_server(dirs: &Dirs) -> Option<PathBuf> {
    [RUNTIME_VULKAN, RUNTIME_CPU].iter().find_map(|a| find_server(&dirs.runtime(a)))
}

/// The model to serve: the option, else the 4B ("GPU") model in both modes, else the 2B. On the CPU the 2B is about
/// twice as fast but picked the wrong intent for ~20 % of misheard commands in the voice AI bench (the 4B: 0-3 %),
/// so it is only the fallback when the 4B is missing (or asked for with --model).
pub fn pick_model(o: &ServeOpts, dirs: &Dirs) -> Result<PathBuf, String> {
    if let Some(m) = &o.model {
        return if m.is_file() { Ok(m.clone()) } else { Err(format!("{}: no such model file", m.display())) };
    }
    let order: &[Asset] = &[MODEL_GPU, MODEL_CPU];
    order
        .iter()
        .map(|a| dirs.model_path(a))
        .find(|p| p.is_file())
        .ok_or_else(|| "no model installed: run `pimax-ai install`".into())
}

/// The full command line: `nice -n 19 ionice -c 3 llama-server ...` (localhost only).
pub fn serve_argv(server: &Path, model: &Path, o: &ServeOpts) -> Vec<String> {
    let mut v: Vec<String> = vec!["nice".into(), "-n".into(), "19".into(), "ionice".into(), "-c".into(), "3".into()];
    v.push(server.to_string_lossy().into_owned());
    let s = |x: &str| x.to_string();
    v.extend([s("-m"), model.to_string_lossy().into_owned()]);
    v.extend([s("--host"), s("127.0.0.1"), s("--port"), o.port.to_string()]);
    v.extend([s("--alias"), s(MODEL_ALIAS)]);
    v.extend([s("--jinja"), s("--reasoning"), s("off"), s("--chat-template-kwargs"), s(r#"{"enable_thinking":false}"#), s("--no-webui")]);
    v.extend([s("-c"), o.ctx.to_string(), s("-np"), s("1")]);
    // low CPU thread priority for the server's own threads too
    v.extend([s("--prio"), s("-1")]);
    if o.cpu() {
        v.extend([s("-ngl"), s("0"), s("--device"), s("none"), s("-t"), o.threads.to_string()]);
    } else {
        let n = o.gpu_layers.unwrap_or(999);
        v.extend([s("-ngl"), n.to_string(), s("-t"), o.threads.min(4).to_string()]);
    }
    v
}

/// A running server, as the pid file records it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PidInfo {
    pub pid: i32,
    pub port: u16,
    pub mode: String,
    pub model: String,
}

impl PidInfo {
    pub fn to_text(&self) -> String {
        format!("{}\nport={}\nmode={}\nmodel={}\n", self.pid, self.port, self.mode, self.model)
    }
    pub fn parse(t: &str) -> Option<PidInfo> {
        let mut l = t.lines();
        let pid = l.next()?.trim().parse().ok()?;
        let mut p = PidInfo { pid, port: DEFAULT_PORT, mode: String::new(), model: String::new() };
        for x in l {
            match x.split_once('=') {
                Some(("port", v)) => p.port = v.trim().parse().unwrap_or(DEFAULT_PORT),
                Some(("mode", v)) => p.mode = v.trim().into(),
                Some(("model", v)) => p.model = v.trim().into(),
                _ => {}
            }
        }
        Some(p)
    }
}

/// The process is alive and is a llama-server (by its command line).
pub fn is_server(pid: i32) -> bool {
    std::fs::read(format!("/proc/{pid}/cmdline")).map(|c| String::from_utf8_lossy(&c).contains("llama-server")).unwrap_or(false)
}

/// The pid file, when it names a live llama-server.
pub fn running(dirs: &Dirs) -> Option<PidInfo> {
    let p = PidInfo::parse(&std::fs::read_to_string(dirs.pid_file()).ok()?)?;
    is_server(p.pid).then_some(p)
}

/// Start the server. `foreground`: replace this process (systemd); else detach (new session), log to the server
/// log, write the pid file and return its info.
pub fn serve(o: &ServeOpts, dirs: &Dirs, foreground: bool) -> Result<PidInfo, String> {
    use std::os::unix::process::CommandExt;
    if let Some(p) = running(dirs) {
        return Err(format!("already running (pid {}, port {})", p.pid, p.port));
    }
    let server = installed_server(dirs).ok_or("llama-server is not installed: run `pimax-ai install`")?;
    let model = pick_model(o, dirs)?;
    let argv = serve_argv(&server, &model, o);
    let libdir = server.parent().map(Path::to_path_buf).unwrap_or_default();
    let mut cmd = Command::new(&argv[0]);
    cmd.args(&argv[1..]).env("LD_LIBRARY_PATH", &libdir).stdin(std::process::Stdio::null());
    if o.cpu() {
        // no GPU at all in CPU mode
        cmd.env("GGML_VK_VISIBLE_DEVICES", "");
    }
    let mode = if o.cpu() { "cpu".to_string() } else { format!("gpu ({} layers)", o.gpu_layers.map(|n| n.to_string()).unwrap_or("all".into())) };
    let info = |pid: i32| PidInfo { pid, port: o.port, mode: mode.clone(), model: model.file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default() };
    std::fs::create_dir_all(&dirs.state).map_err(|e| e.to_string())?;
    if foreground {
        // the pid stays the same across exec
        let me = std::process::id() as i32;
        std::fs::write(dirs.pid_file(), info(me).to_text()).map_err(|e| e.to_string())?;
        let e = cmd.exec();
        let _ = std::fs::remove_file(dirs.pid_file());
        return Err(format!("exec {}: {e}", argv[0]));
    }
    std::fs::create_dir_all(&dirs.logs).map_err(|e| e.to_string())?;
    let log = std::fs::OpenOptions::new().create(true).append(true).open(dirs.server_log()).map_err(|e| e.to_string())?;
    let log2 = log.try_clone().map_err(|e| e.to_string())?;
    cmd.stdout(log).stderr(log2);
    // SAFETY: setsid is async-signal-safe; it detaches the server from our terminal session.
    unsafe {
        cmd.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }
    let child = cmd.spawn().map_err(|e| format!("{}: {e}", argv[0]))?;
    let p = info(child.id() as i32);
    std::fs::write(dirs.pid_file(), p.to_text()).map_err(|e| e.to_string())?;
    Ok(p)
}

/// Stop the server named by the pid file (SIGTERM, then SIGKILL after 5 s). `Ok(None)` = nothing was running.
pub fn stop(dirs: &Dirs) -> Result<Option<i32>, String> {
    let Some(p) = std::fs::read_to_string(dirs.pid_file()).ok().and_then(|t| PidInfo::parse(&t)) else {
        return Ok(None);
    };
    if !is_server(p.pid) {
        let _ = std::fs::remove_file(dirs.pid_file());
        return Ok(None);
    }
    // SAFETY: plain kill(2) on a pid we verified to be a llama-server.
    unsafe { libc::kill(p.pid, libc::SIGTERM) };
    for _ in 0..50 {
        if !is_server(p.pid) {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    if is_server(p.pid) {
        // SAFETY: as above.
        unsafe { libc::kill(p.pid, libc::SIGKILL) };
    }
    let _ = std::fs::remove_file(dirs.pid_file());
    Ok(Some(p.pid))
}

// ------------------------------------------------------------------------------------------------------- status

/// `GET http://127.0.0.1:<port><path>` -> (status code, body).
pub fn http_get(port: u16, path: &str, timeout: Duration) -> Result<(u16, String), String> {
    let mut s = TcpStream::connect_timeout(&([127, 0, 0, 1], port).into(), timeout).map_err(|e| e.to_string())?;
    s.set_read_timeout(Some(timeout)).ok();
    write!(s, "GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n").map_err(|e| e.to_string())?;
    let mut buf = String::new();
    s.read_to_string(&mut buf).map_err(|e| e.to_string())?;
    let code = buf.split_whitespace().nth(1).and_then(|c| c.parse().ok()).ok_or("no HTTP status")?;
    Ok((code, buf.split_once("\r\n\r\n").map(|(_, b)| b.to_string()).unwrap_or_default()))
}

/// GPU memory a process uses (MiB), from `nvidia-smi --query-compute-apps`.
pub fn gpu_mib(pid: i32) -> Option<u32> {
    let out = Command::new("nvidia-smi").args(["--query-compute-apps=pid,used_memory", "--format=csv,noheader,nounits"]).output().ok()?;
    parse_gpu_mib(&String::from_utf8_lossy(&out.stdout), pid)
}

pub fn parse_gpu_mib(csv: &str, pid: i32) -> Option<u32> {
    csv.lines().find_map(|l| {
        let (p, m) = l.split_once(',')?;
        (p.trim().parse::<i32>().ok()? == pid).then(|| m.trim().parse().ok()).flatten()
    })
}

/// The state the AI tab shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Status {
    pub runtime: Option<PathBuf>,
    /// (asset, installed with the right size).
    pub models: Vec<(Asset, bool)>,
    pub running: Option<PidInfo>,
    /// `/health` answered 200 (the model is loaded).
    pub ready: bool,
    pub gpu_mib: Option<u32>,
}

impl Status {
    pub fn get(dirs: &Dirs) -> Status {
        let running = running(dirs);
        let ready = running.as_ref().is_some_and(|p| http_get(p.port, "/health", Duration::from_millis(500)).is_ok_and(|(c, _)| c == 200));
        let gpu = running.as_ref().and_then(|p| gpu_mib(p.pid));
        Status {
            runtime: installed_server(dirs),
            models: [MODEL_GPU, MODEL_CPU].iter().map(|a| (*a, file_size(&dirs.model_path(a)) == Some(a.size))).collect(),
            running,
            ready,
            gpu_mib: gpu,
        }
    }

    /// One JSON object (no serde: the GUI parses it with its own JSON reader).
    pub fn to_json(&self) -> String {
        let q = |s: &str| format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""));
        let models: Vec<String> = self.models.iter().map(|(a, ok)| format!("{{\"id\":{},\"file\":{},\"installed\":{ok}}}", q(a.id), q(a.file))).collect();
        let run = match &self.running {
            Some(p) => format!(
                "{{\"pid\":{},\"port\":{},\"mode\":{},\"model\":{},\"url\":{}}}",
                p.pid,
                p.port,
                q(&p.mode),
                q(&p.model),
                q(&format!("http://127.0.0.1:{}/v1", p.port))
            ),
            None => "null".into(),
        };
        format!(
            "{{\"installed\":{},\"runtime\":{},\"models\":[{}],\"running\":{run},\"ready\":{},\"gpu_mib\":{},\"gpu_budget_mib\":{GPU_BUDGET_MIB},\"model_alias\":{}}}",
            self.runtime.is_some() && self.models.iter().any(|(_, ok)| *ok),
            self.runtime.as_ref().map(|p| q(&p.to_string_lossy())).unwrap_or("null".into()),
            models.join(","),
            self.ready,
            self.gpu_mib.map(|m| m.to_string()).unwrap_or("null".into()),
            q(MODEL_ALIAS)
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dirs() -> Dirs {
        Dirs::from_env(&|k| (k == "HOME").then(|| "/home/u".to_string()))
    }

    #[test]
    fn paths_follow_xdg() {
        let d = dirs();
        assert_eq!(d.data, PathBuf::from("/home/u/.local/share/pimax/ai"));
        assert_eq!(d.pid_file(), PathBuf::from("/home/u/.local/state/pimax/ai/llama-server.pid"));
        assert_eq!(d.server_log(), PathBuf::from("/home/u/.local/state/pimax/logs/llama-server.log"));
        let d2 = Dirs::from_env(&|k| match k {
            "HOME" => Some("/h".into()),
            "XDG_DATA_HOME" => Some("/d".into()),
            "XDG_STATE_HOME" => Some("relative/ignored".into()),
            _ => None,
        });
        assert_eq!(d2.data, PathBuf::from("/d/pimax/ai"));
        assert_eq!(d2.state, PathBuf::from("/h/.local/state/pimax/ai"));
    }

    #[test]
    fn assets_are_pinned_official_and_small() {
        for a in ASSETS {
            assert!(a.url.starts_with("https://github.com/ggml-org/llama.cpp/releases/download/") || a.url.starts_with("https://huggingface.co/unsloth/"));
            assert!(a.url.ends_with(a.file));
            assert_eq!(a.sha256.len(), 64);
            assert!(a.sha256.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
        }
        let all = InstallPlan { runtime: RUNTIME_VULKAN, gpu_model: true, cpu_model: true };
        assert!(all.bytes() < 6 << 30, "the default install stays under 6 GB");
    }

    #[test]
    fn serve_command_is_local_and_low_priority() {
        let o = ServeOpts::default();
        let v = serve_argv(Path::new("/x/llama-server"), Path::new("/m/q.gguf"), &o);
        assert_eq!(&v[..6], ["nice", "-n", "19", "ionice", "-c", "3"]);
        let s = v.join(" ");
        assert!(s.contains("--host 127.0.0.1 --port 8080"));
        assert!(s.contains("--jinja") && s.contains("enable_thinking\":false"));
        assert!(s.contains("-ngl 999") && !s.contains("--device none"));
        let cpu = ServeOpts { gpu_layers: Some(0), threads: 12, ..Default::default() };
        let s = serve_argv(Path::new("/x/llama-server"), Path::new("/m/q.gguf"), &cpu).join(" ");
        assert!(s.contains("-ngl 0 --device none -t 12"));
        let some = ServeOpts { gpu_layers: Some(20), ..Default::default() };
        assert!(serve_argv(Path::new("s"), Path::new("m"), &some).join(" ").contains("-ngl 20"));
        // never a public bind
        assert!(!s.contains("0.0.0.0"));
    }

    #[test]
    fn curl_resumes_https_only() {
        let a = curl_args(&MODEL_GPU, Path::new("/tmp/x.part"));
        let s = a.join(" ");
        assert!(s.contains("-C -") && s.contains("--proto =https") && s.contains("--fail"));
        assert_eq!(a.last().unwrap(), MODEL_GPU.url);
    }

    #[test]
    fn pid_file_and_parsers() {
        let p = PidInfo { pid: 42, port: 8081, mode: "cpu".into(), model: "a.gguf".into() };
        assert_eq!(PidInfo::parse(&p.to_text()), Some(p));
        assert_eq!(PidInfo::parse("x"), None);
        assert_eq!(parse_sha256sum(&format!("{}  /f\n", MODEL_CPU.sha256)).as_deref(), Some(MODEL_CPU.sha256));
        assert_eq!(parse_sha256sum("nope /f"), None);
        assert_eq!(parse_gpu_mib("123, 3500\n42, 3120\n", 42), Some(3120));
        assert_eq!(parse_gpu_mib("", 42), None);
    }

    #[test]
    fn status_json_shape() {
        let s = Status { runtime: None, models: vec![(MODEL_GPU, true), (MODEL_CPU, false)], running: None, ready: false, gpu_mib: None };
        let j = s.to_json();
        assert!(j.starts_with("{\"installed\":false,\"runtime\":null,"));
        assert!(j.contains("\"running\":null") && j.contains("\"gpu_budget_mib\":4096"));
        let s2 = Status { runtime: Some("/r/llama-server".into()), running: Some(PidInfo { pid: 7, port: 8080, mode: "cpu".into(), model: "m".into() }), ready: true, ..s };
        assert!(s2.to_json().contains("\"url\":\"http://127.0.0.1:8080/v1\""));
    }

    #[test]
    fn find_server_and_pick_model() {
        let base = std::env::temp_dir().join(format!("pimax-ai-test-{}", std::process::id()));
        let d = Dirs { data: base.join("data"), state: base.join("state"), logs: base.join("logs") };
        let rt = d.runtime(&RUNTIME_VULKAN).join("build/bin");
        std::fs::create_dir_all(&rt).unwrap();
        std::fs::write(rt.join("llama-server"), "").unwrap();
        assert_eq!(installed_server(&d), Some(rt.join("llama-server")));
        assert!(pick_model(&ServeOpts::default(), &d).is_err());
        std::fs::create_dir_all(d.models()).unwrap();
        std::fs::write(d.model_path(&MODEL_GPU), "").unwrap();
        // CPU mode falls back to the GPU model when the small one is missing
        assert_eq!(pick_model(&ServeOpts { gpu_layers: Some(0), ..Default::default() }, &d).unwrap(), d.model_path(&MODEL_GPU));
        std::fs::write(d.model_path(&MODEL_CPU), "").unwrap();
        assert_eq!(pick_model(&ServeOpts { gpu_layers: Some(0), ..Default::default() }, &d).unwrap(), d.model_path(&MODEL_GPU), "the 4B on the CPU too");
        assert_eq!(pick_model(&ServeOpts::default(), &d).unwrap(), d.model_path(&MODEL_GPU));
        assert!(running(&d).is_none());
        std::fs::remove_dir_all(&base).unwrap();
    }
}
