//! The two-stage AI on this PC (docs/AI.md section 16): a router ("Lara") on port 8081 sorts a voice command into a
//! category, and the model on port 8080 (on the GPU) acts with only that category's tools.
//!
//! * Router `laya` (the default): [Laya](https://github.com/NandhaKishorM/laya), the open-weights (Apache-2.0)
//!   counterpart of TypeSafe's Jev. Laya serves Jev's own `/v1/systemone` protocol (`laya-serve`) and answers a
//!   `choice` question in one encoder pass (ModernBERT-large, 421M), with no text generated, on the CPU only. It is a
//!   Python package: `install` makes a virtualenv in `~/.local/share/omarchyvr/ai/laya/venv` with PyTorch's CPU wheel and
//!   exact versions ([`LAYA_CONSTRAINTS`]), and fetches the checkpoint at its reviewed revision ([`LAYA_REVISION`]).
//! * Router `qwen3.5-0.8b`: a small GGUF model on a second llama-server (CPU, `--device none`).
//! * Action model: the main slot ([`Slot::Main`], port 8080) serves the chosen GGUF with all layers on the GPU, so
//!   the simple mode's single-model request remains the fallback on the same server.
//!
//! [`up`] starts what ai.toml asks for (and stops what it no longer asks for); [`down`] stops both slots.
//! Nothing here touches a VR process; the servers run at nice 19 with the lowest best-effort I/O.

use crate::*;
use omarchyvr_config::ai::{asr_plan, AiConfig, AsrPlan, LocalEngine, LocalTwoStage, VoiceLocal, ASR_PORT, ASR_VRAM_MIB};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

/// The Laya release on PyPI.
pub const LAYA_VERSION: &str = "0.4.8";
/// The checkpoint the router serves (`english`: ModernBERT-large, 421M; picked by the benchmark of docs/AI.md 16).
pub const LAYA_CHECKPOINT: &str = "english";
/// Its Hugging Face repository and the revision Laya itself lists as reviewed (`laya.revisions.PINNED_REVISIONS`).
pub const LAYA_REPO: &str = "convaiinnovations/laya";
pub const LAYA_REVISION: &str = "55cf4c4ebb4ebe31b2550e8bdf3bd21b99753851";
/// PyTorch's CPU-only wheels (no CUDA libraries: the router never touches the GPU).
pub const TORCH_INDEX: &str = "https://download.pytorch.org/whl/cpu";
/// Every package of the router's virtualenv at the version tested (2026-10-16, Python 3.14).
pub const LAYA_CONSTRAINTS: &str = "annotated-doc==0.0.5\nannotated-types==0.8.0\nanyio==4.15.1\nclick==8.5.0\nfastapi==0.143.2\nfilelock==3.32.3\nfsspec==2026.7.0\nh11==0.16.0\nhf-xet==1.7.0\nhttpcore2==2.13.1\nhttpx2==2.13.1\nhuggingface_hub==2.2.0\nidna==3.20\nJinja2==3.1.6\nlaya==0.4.8\nmarkdown-it-py==4.2.0\nMarkupSafe==3.0.3\nmdurl==0.1.2\nmpmath==1.3.0\nnetworkx==3.6.1\nnumpy==2.5.4\nopentelemetry-api==1.45.1\npackaging==26.3\npydantic==2.14.1\npydantic_core==2.50.1\nPygments==2.21.0\npython-multipart==0.0.32\nPyYAML==6.0.3\nregex==2026.9.29\nrich==15.0.0\nsafetensors==0.8.0\nsetuptools==78.1.0\nshellingham==1.5.4\nstarlette==1.8.0\nsympy==1.14.0\ntokenizers==0.23.3\ntorch==2.14.1+cpu\ntqdm==4.70.1\ntransformers==5.20.0\ntruststore==0.10.5\ntyper==0.27.3\ntyping-inspection==0.4.4\ntyping_extensions==4.16.0\nuvicorn==0.54.0\n";
/// Download size of the router (Python packages ~0.4 GiB, the Laya repository 2.3 GiB), for the app.
pub const LAYA_DOWNLOAD_MIB: u64 = 2760;
/// CPU threads of the router (a few: one short forward pass per command, next to VR).
pub const ROUTER_THREADS: u32 = 8;
/// Headroom left on the GPU besides the action model's own figure (MiB).
pub const VRAM_MARGIN_MIB: u32 = 1024;

/// The router's virtualenv and the files that say it is complete.
pub fn laya_venv(dirs: &Dirs) -> PathBuf {
    dirs.laya().join("venv")
}
pub fn laya_hf(dirs: &Dirs) -> PathBuf {
    dirs.laya().join("hf")
}
pub fn laya_serve_bin(dirs: &Dirs) -> PathBuf {
    laya_venv(dirs).join("bin/laya-serve")
}
/// The checkpoint snapshot in the Hugging Face cache.
pub fn laya_snapshot(dirs: &Dirs) -> PathBuf {
    laya_hf(dirs).join("hub").join(format!("models--{}", LAYA_REPO.replace('/', "--"))).join("snapshots").join(LAYA_REVISION)
}

/// Lara: Laya tuned on this PC to the voice commands (`omarchyvr-ai tune-lara`), a checkpoint directory.
pub fn lara_dir(dirs: &Dirs) -> PathBuf {
    dirs.laya().join("lara")
}

/// The tuned checkpoint is there (its weights and the tuning report).
pub fn lara_installed(dirs: &Dirs) -> bool {
    let d = lara_dir(dirs);
    laya_installed(dirs) && std::fs::read_dir(&d).is_ok_and(|rd| rd.flatten().any(|e| e.file_name().to_string_lossy().ends_with(".safetensors")))
}

/// Tune Laya to the voice commands: `laya-train` on the CPU (nice 19, 3/4 of the threads), 3 epochs, a fixed seed;
/// `data` is the JSONL of `omarchyvr-voice routerbench --out` (every catalogue phrase and an ASR-mangled copy; the audit's
/// natural phrasings stay out for the test). The result replaces `lara/` only when the run finished. About an hour
/// on a 16-core desktop; nothing runs on the GPU.
pub fn tune_lara(dirs: &Dirs, data: &Path, log: &mut dyn FnMut(&str)) -> Result<(), String> {
    if !laya_installed(dirs) {
        install_laya(dirs, log)?;
    }
    let out = dirs.laya().join("lara.tmp");
    let _ = std::fs::remove_dir_all(&out);
    let threads = default_threads().max(4);
    let mut c = low(&laya_venv(dirs).join("bin/laya-train"));
    c.arg("--data").arg(data).args(["--base", LAYA_REPO, "--out"]).arg(&out).args(["--device", "cpu", "--epochs", "3", "--shuffle-options", "--seed", "1"]);
    c.env("HF_HOME", laya_hf(dirs)).env("HF_HUB_DISABLE_TELEMETRY", "1").env("LAYA_REVISION", "reviewed").env("CUDA_VISIBLE_DEVICES", "").env("OMP_NUM_THREADS", threads.to_string());
    run_logged(&mut c, "tuning Lara to the voice commands (about an hour, processor only)", log)?;
    let dest = lara_dir(dirs);
    let _ = std::fs::remove_dir_all(&dest);
    std::fs::rename(&out, &dest).map_err(|e| format!("{}: {e}", dest.display()))?;
    log("Lara: tuned");
    Ok(())
}

/// Laya is installed: the server script and a checkpoint with weights.
pub fn laya_installed(dirs: &Dirs) -> bool {
    laya_serve_bin(dirs).is_file() && std::fs::read_dir(laya_snapshot(dirs)).is_ok_and(|rd| rd.flatten().any(|e| e.file_name().to_string_lossy().ends_with(".safetensors")))
}

/// Is a router / action model id installed?
pub fn model_installed(dirs: &Dirs, id: &str) -> bool {
    match omarchyvr_config::ai::local_model(id).map(|m| m.engine) {
        Some(LocalEngine::Laya) if id == "lara" => lara_installed(dirs),
        Some(LocalEngine::Laya) => laya_installed(dirs),
        Some(LocalEngine::Llama) => model_asset(id).is_some_and(|a| std::fs::metadata(dirs.model_path(&a)).is_ok_and(|m| m.len() == a.size)),
        None => false,
    }
}

fn run_logged(cmd: &mut Command, what: &str, log: &mut dyn FnMut(&str)) -> Result<(), String> {
    log(&format!("{what} ..."));
    let st = cmd.status().map_err(|e| format!("{what}: {e}"))?;
    if !st.success() {
        return Err(format!("{what} failed ({st}); press Download again to retry (it resumes)"));
    }
    Ok(())
}

/// The `nice -n 19 ionice -c 2 -n 7` prefix of heavy installs and servers (lowest best-effort I/O; see `serve_argv`).
fn low(prog: &Path) -> Command {
    let mut c = Command::new("nice");
    c.args(["-n", "19", "ionice", "-c", "2", "-n", "7"]).arg(prog);
    c
}

/// The Laya version in the router's virtualenv (None: not installed).
pub fn laya_version(dirs: &Dirs) -> Option<String> {
    let out = Command::new(laya_venv(dirs).join("bin/python3")).args(["-I", "-c", "import importlib.metadata as m; print(m.version('laya'))"]).output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Install Laya: a virtualenv (system `python3`, 3.10 or newer), PyTorch CPU + Laya at the tested versions, and the
/// checkpoint at the reviewed revision. Resumable: each step is skipped when already done.
pub fn install_laya(dirs: &Dirs, log: &mut dyn FnMut(&str)) -> Result<(), String> {
    let venv = laya_venv(dirs);
    std::fs::create_dir_all(dirs.laya()).map_err(|e| format!("{}: {e}", dirs.laya().display()))?;
    let py = venv.join("bin/python3");
    if !py.is_file() {
        run_logged(Command::new("python3").args(["-m", "venv"]).arg(&venv), "making the router's Python environment", log)?;
    }
    let cons = dirs.laya().join("constraints.txt");
    std::fs::write(&cons, LAYA_CONSTRAINTS).map_err(|e| e.to_string())?;
    if !laya_serve_bin(dirs).is_file() || laya_version(dirs).as_deref() != Some(LAYA_VERSION) {
        let mut c = low(&py);
        c.args(["-m", "pip", "install", "--disable-pip-version-check", "--no-input", "--no-cache-dir", "--index-url", TORCH_INDEX, "-c"]).arg(&cons).arg("torch");
        run_logged(&mut c, "downloading PyTorch for the CPU (~200 MiB)", log)?;
        let mut c = low(&py);
        c.args(["-m", "pip", "install", "--disable-pip-version-check", "--no-input", "--no-cache-dir", "-c"]).arg(&cons).arg(format!("laya[serve]=={LAYA_VERSION}"));
        run_logged(&mut c, &format!("installing Laya {LAYA_VERSION}"), log)?;
    }
    if !laya_installed(dirs) {
        let mut c = low(&py);
        c.arg("-I")
            .arg("-c")
            .arg("import sys; from huggingface_hub import snapshot_download as s; print(s(sys.argv[1], revision=sys.argv[2]))")
            .args([LAYA_REPO, LAYA_REVISION])
            .env("HF_HOME", laya_hf(dirs))
            .env("HF_HUB_DISABLE_TELEMETRY", "1");
        run_logged(&mut c, &format!("downloading the Laya checkpoint {LAYA_REPO} at {}", &LAYA_REVISION[..7]), log)?;
    }
    if !laya_installed(dirs) {
        return Err("the Laya checkpoint is still missing after the download".into());
    }
    log("router (Laya): OK");
    Ok(())
}

/// Install everything a two-stage setting needs (the runtime, the router, the action model).
pub fn install_two_stage(t: &LocalTwoStage, runtime: Asset, dirs: &Dirs, log: &mut dyn FnMut(&str)) -> Result<(), String> {
    if installed_server(dirs).is_none() {
        let tb = fetch(&runtime, &dirs.downloads().join(runtime.file), log)?;
        unpack_runtime(&runtime, &tb, dirs)?;
    }
    for id in [t.router.as_str(), t.action.as_str()].into_iter().chain(t.agent.as_deref()) {
        match omarchyvr_config::ai::local_model(id).map(|m| m.engine) {
            Some(LocalEngine::Laya) if id == "lara" && !lara_installed(dirs) => {
                install_laya(dirs, log)?;
                // the tuning takes hours: in the background (Laya as published serves meanwhile)
                start_tuning(dirs, log);
            }
            Some(LocalEngine::Laya) => install_laya(dirs, log)?,
            Some(LocalEngine::Llama) => {
                let a = model_asset(id).ok_or_else(|| format!("{id}: no download known"))?;
                fetch(&a, &dirs.model_path(&a), log)?;
            }
            None => return Err(format!("{id}: not a known local model")),
        }
    }
    Ok(())
}

/// The background tuning's pid file and log.
pub fn tune_pid(dirs: &Dirs) -> PathBuf {
    dirs.state.join("lara-tune.pid")
}

/// Is `omarchyvr-ai tune-lara` running?
pub fn tuning(dirs: &Dirs) -> Option<i32> {
    let pid: i32 = std::fs::read_to_string(tune_pid(dirs)).ok()?.trim().parse().ok()?;
    let c = std::fs::read(format!("/proc/{pid}/cmdline")).ok()?;
    let c = String::from_utf8_lossy(&c);
    (c.contains("omarchyvr-ai") && c.contains("tune-lara")).then_some(pid)
}

/// Start `omarchyvr-ai tune-lara` detached (own session, nice 19), unless it runs or Lara is tuned.
pub fn start_tuning(dirs: &Dirs, log: &mut dyn FnMut(&str)) {
    use std::os::unix::process::CommandExt;
    if tuning(dirs).is_some() || lara_installed(dirs) {
        return;
    }
    let Ok(me) = std::env::current_exe() else { return };
    let _ = std::fs::create_dir_all(&dirs.logs);
    let _ = std::fs::create_dir_all(&dirs.state);
    let Ok(f) = std::fs::OpenOptions::new().create(true).append(true).open(dirs.logs.join("ai-lara-tune.log")) else { return };
    let Ok(f2) = f.try_clone() else { return };
    let mut c = Command::new("nice");
    c.args(["-n", "19"]).arg(me).arg("tune-lara").stdin(std::process::Stdio::null()).stdout(f).stderr(f2);
    // SAFETY: setsid is async-signal-safe.
    unsafe {
        c.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }
    match c.spawn() {
        Ok(ch) => {
            let _ = std::fs::write(tune_pid(dirs), ch.id().to_string());
            log(&format!("Lara: tuning to the voice commands in the background (pid {}, about an hour on the processor; log {})", ch.id(), dirs.logs.join("ai-lara-tune.log").display()));
        }
        Err(e) => log(&format!("Lara: could not start the tuning: {e}")),
    }
}

/// The training rows for Lara from the command catalogue: `omarchyvr-voice routerbench --out FILE` (omarchyvr-voice next to
/// this program, else on PATH).
pub fn write_training_rows(out: &Path, log: &mut dyn FnMut(&str)) -> Result<(), String> {
    let sib = std::env::current_exe().ok().and_then(|e| e.parent().map(|d| d.join("omarchyvr-voice"))).filter(|p| p.is_file());
    let prog = sib.unwrap_or_else(|| PathBuf::from("omarchyvr-voice"));
    let mut c = Command::new(prog);
    c.args(["routerbench", "--out"]).arg(out);
    run_logged(&mut c, "writing Lara's training phrases from the command catalogue", log)
}

/// The Laya server's command line and environment (localhost, CPU only, offline, reviewed revision); `tuned`: the
/// Lara checkpoint instead of the published one.
pub fn laya_command(dirs: &Dirs, threads: u32, tuned: bool) -> Command {
    let mut c = low(&laya_serve_bin(dirs));
    let ckpt = if tuned { "lara" } else { LAYA_CHECKPOINT };
    if tuned {
        c.env("LAYA_EXTRA_MODELS", serde_like_json(&[("lara", &lara_dir(dirs).to_string_lossy())]));
    }
    c.env("LAYA_HOST", "127.0.0.1")
        .env("LAYA_PORT", ROUTER_PORT.to_string())
        .env("LAYA_DEVICE", "cpu")
        .env("LAYA_MODELS", ckpt)
        .env("LAYA_DEFAULT_MODEL", ckpt)
        .env("LAYA_MAX_LOADED", "1")
        .env("LAYA_PRELOAD", "1")
        .env("LAYA_THREADS", threads.to_string())
        .env("LAYA_REVISION", "reviewed")
        .env("LAYA_MAX_CONCURRENT", "4")
        .env("LAYA_LOG_LEVEL", "warning")
        .env("HF_HOME", laya_hf(dirs))
        .env("HF_HUB_OFFLINE", "1")
        .env("HF_HUB_DISABLE_TELEMETRY", "1")
        .env("CUDA_VISIBLE_DEVICES", "")
        .stdin(std::process::Stdio::null());
    c
}

/// `{"k": "v", ...}` with JSON string escapes.
fn serde_like_json(kv: &[(&str, &str)]) -> String {
    let q = |s: &str| format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""));
    format!("{{{}}}", kv.iter().map(|(k, v)| format!("{}: {}", q(k), q(v))).collect::<Vec<_>>().join(", "))
}

/// The router's model name in the pid file.
pub fn router_model_name(id: &str) -> String {
    match id {
        "lara" => "laya:lara".into(),
        "laya" => format!("laya:{LAYA_CHECKPOINT}"),
        other => model_asset(other).map(|a| a.file.to_string()).unwrap_or_default(),
    }
}

/// Start the router slot (port 8081) with a router id.
pub fn serve_router(dirs: &Dirs, id: &str, foreground: bool) -> Result<PidInfo, String> {
    if let Some(p) = running_slot(dirs, Slot::Router) {
        return Err(format!("the router already runs (pid {}, {})", p.pid, p.model));
    }
    match omarchyvr_config::ai::local_model(id).map(|m| m.engine) {
        Some(LocalEngine::Laya) => {
            let tuned = id == "lara";
            if !laya_installed(dirs) || (tuned && !lara_installed(dirs)) {
                return Err(format!("the router ({id}) is not downloaded yet: press Download on the AI page"));
            }
            let info = PidInfo { pid: 0, port: ROUTER_PORT, mode: "cpu".into(), model: router_model_name(id) };
            spawn_slot(laya_command(dirs, ROUTER_THREADS, tuned), dirs, foreground, Slot::Router, info)
        }
        Some(LocalEngine::Llama) => {
            let a = model_asset(id).ok_or_else(|| format!("{id}: no model file known"))?;
            let o = ServeOpts { gpu_layers: Some(0), port: ROUTER_PORT, ctx: 4096, threads: ROUTER_THREADS, model: Some(dirs.model_path(&a)), fit_margin: None };
            serve_slot(&o, dirs, foreground, Slot::Router)
        }
        None => Err(format!("{id}: not a known router")),
    }
}

// ------------------------------------------------------------------------------------- speech recognition (AI.md 17)

/// whisper.cpp's Vulkan server (`~/.local/lib/omarchyvr/whisper.cpp/bin`, else built in `~/src/whisper.cpp/build-vk`) and the large-v3-turbo q5_0 model.
pub fn asr_server_bin() -> PathBuf {
    omarchyvr_config::paths::whisper_server(&home_dir())
}
pub fn asr_model_file() -> PathBuf {
    home_dir().join("models/whisper/ggml-large-v3-turbo-q5_0.bin")
}
fn home_dir() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default()
}

/// What is missing for the large model, in plain words (None: all there).
pub fn asr_missing(server: &Path, model: &Path) -> Option<String> {
    if !server.is_file() {
        return Some(format!("whisper-server is not built ({} is missing)", server.display()));
    }
    if !model.is_file() {
        return Some(format!("the large speech model is not downloaded ({} is missing)", model.display()));
    }
    None
}

/// The server's command: `-nc` (without it the server feeds earlier output into the next request and degrades),
/// English, no timestamps, loopback only, nice 19.
pub fn asr_argv(server: &Path, model: &Path, port: u16) -> Vec<String> {
    let s = |p: &Path| p.to_string_lossy().into_owned();
    ["nice", "-n", "19", "ionice", "-c", "2", "-n", "7"].iter().map(|x| x.to_string()).chain([s(server), "-m".into(), s(model), "--host".into(), "127.0.0.1".into(), "--port".into(), port.to_string(), "-nc".into(), "-l".into(), "en".into(), "-nt".into()]).collect()
}

/// Start whisper-server (port 8082) in the Asr slot.
pub fn serve_asr(dirs: &Dirs) -> Result<PidInfo, String> {
    if let Some(p) = running_slot(dirs, Slot::Asr) {
        return Err(format!("the speech server already runs (pid {})", p.pid));
    }
    let (server, model) = (asr_server_bin(), asr_model_file());
    if let Some(m) = asr_missing(&server, &model) {
        return Err(m);
    }
    let argv = asr_argv(&server, &model, ASR_PORT);
    let mut cmd = Command::new(&argv[0]);
    cmd.args(&argv[1..]).stdin(std::process::Stdio::null());
    let file = model.file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default();
    spawn_slot(cmd, dirs, false, Slot::Asr, PidInfo { pid: 0, port: ASR_PORT, mode: "gpu (vulkan)".into(), model: file })
}

/// What `up` does with the speech server: the decision (config, free VRAM counting our own servers as free) and
/// whether it can start at all.
pub fn asr_decision(cfg: &AiConfig, free_all_ours: Option<u32>) -> (AsrPlan, bool) {
    let p = asr_plan(cfg.asr, cfg.voice_local_models != Some(false), free_all_ours);
    let usable = p.gpu && asr_missing(&asr_server_bin(), &asr_model_file()).is_none();
    (p, usable)
}

/// Free and total VRAM of the first GPU (MiB), from nvidia-smi.
pub fn gpu_free_mib() -> Option<(u32, u32)> {
    let out = Command::new("nvidia-smi").args(["--query-gpu=memory.free,memory.total", "--format=csv,noheader,nounits"]).output().ok()?;
    parse_free(&String::from_utf8_lossy(&out.stdout))
}

pub fn parse_free(csv: &str) -> Option<(u32, u32)> {
    let l = csv.lines().next()?;
    let (f, t) = l.split_once(',')?;
    Some((f.trim().parse().ok()?, t.trim().parse().ok()?))
}

/// What runs on the GPU besides the AI, from the process list: it sets how much VRAM the AI leaves free.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Session {
    /// No VR.
    None,
    /// The Work desk (WayVR on Monado).
    Work,
    /// SteamVR without a game yet (a game will want the memory).
    SteamVr,
    /// A game runs (SteamVR or not).
    Game,
}

impl Session {
    /// VRAM kept free for the session besides what it uses now (MiB).
    pub fn reserve_mib(self) -> u32 {
        match self {
            Session::None => 1024,
            Session::Work => 2048,
            Session::SteamVr => 8192,
            Session::Game => 3072,
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Session::None => "no VR session",
            Session::Work => "the Work desk",
            Session::SteamVr => "SteamVR (room kept for a game)",
            Session::Game => "a running game",
        }
    }
}

/// The session from `/proc/*/cmdline` (`proc_root` = `/proc`): a Steam game (a process under `steamapps/common/`
/// other than SteamVR itself), SteamVR (`vrserver`), the Work desk (`wayvr`).
pub fn detect_session(proc_root: &Path) -> Session {
    let (mut game, mut steamvr, mut work) = (false, false, false);
    let Ok(rd) = std::fs::read_dir(proc_root) else { return Session::None };
    for e in rd.flatten() {
        if !e.file_name().to_string_lossy().bytes().all(|b| b.is_ascii_digit()) {
            continue;
        }
        let Ok(c) = std::fs::read(e.path().join("cmdline")) else { continue };
        let c = String::from_utf8_lossy(&c);
        let first = c.split('\0').next().unwrap_or("");
        if first.contains("steamapps/common/") && !first.contains("steamapps/common/SteamVR/") {
            game = true;
        } else if first.ends_with("/vrserver") || first.ends_with("/vrcompositor") {
            steamvr = true;
        } else if first.ends_with("/wayvr") {
            work = true;
        }
    }
    if game {
        Session::Game
    } else if steamvr {
        Session::SteamVr
    } else if work {
        Session::Work
    } else {
        Session::None
    }
}

/// Where the main slot's model goes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Gpu {
    /// Every layer on the GPU.
    All,
    /// llama.cpp's own fit (`--fit on --fit-target M`): as many layers as fit with M MiB left free.
    Fit { margin_mib: u32 },
    /// A fixed number of layers (the setting `gpu_layers = N`).
    Layers(u32),
    /// The CPU only.
    Cpu,
}

impl Gpu {
    /// The pid file's `mode` (coarse, so a changed free amount alone does not restart the server).
    pub fn mode(&self) -> String {
        match self {
            Gpu::All => "gpu (all layers)".into(),
            Gpu::Fit { .. } => "gpu (fitted)".into(),
            Gpu::Layers(n) => format!("gpu ({n} layers)"),
            Gpu::Cpu => "cpu".into(),
        }
    }
}

/// Below this budget the GPU is not worth it: the model runs on the CPU.
pub const MIN_GPU_MIB: u32 = 1536;

/// The decision of [`fit`]: the model id, where it goes and why (plain words for the app and the log).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fit {
    pub model: String,
    pub gpu: Gpu,
    pub why: String,
}

/// Fit the action model next to the session. `cap`: "VRAM for AI: up to N" (None = Auto); `free`: free VRAM now
/// (None: unknown, the model's need is trusted); `installed`: the action models on disk. A model that does not fit
/// gives way to a smaller installed one that does, then to a partial GPU fit, then to the CPU.
pub fn fit(chosen: &str, layers: Option<u32>, cap: Option<u32>, free: Option<u32>, session: Session, installed: &[&str]) -> Fit {
    let need = |id: &str| omarchyvr_config::ai::local_model(id).map(|m| m.memory_mib).unwrap_or(4096);
    let label = |id: &str| omarchyvr_config::ai::local_model(id).map(|m| m.label).unwrap_or("the model");
    let room = free.map(|f| f.saturating_sub(session.reserve_mib()));
    let budget = match (room, cap) {
        (Some(r), Some(c)) => r.min(c),
        (Some(r), None) => r,
        (None, Some(c)) => c,
        (None, None) => u32::MAX,
    };
    let gb = |m: u32| format!("{:.1} GB", m as f64 / 1024.0);
    let limit = if cap.is_some_and(|c| room.is_none_or(|r| c <= r)) { format!("the {} limit", gb(budget)) } else { format!("{} free next to {}", gb(budget), session.label()) };
    if let Some(n) = layers {
        let gpu = if n == 0 { Gpu::Cpu } else { Gpu::Layers(n) };
        return Fit { model: chosen.into(), gpu, why: format!("{} with the GPU layers set by hand ({n})", label(chosen)) };
    }
    if need(chosen) <= budget {
        return Fit { model: chosen.into(), gpu: Gpu::All, why: format!("{} fully on the GPU ({} of {limit})", label(chosen), gb(need(chosen))) };
    }
    if let Some(s) = installed.iter().filter(|m| **m != chosen && need(m) < need(chosen)).filter(|m| need(m) <= budget).max_by_key(|m| need(m)) {
        return Fit { model: s.to_string(), gpu: Gpu::All, why: format!("{} does not fit ({limit}); {} runs instead, fully on the GPU", label(chosen), label(s)) };
    }
    if budget >= MIN_GPU_MIB {
        let margin = free.map(|f| f.saturating_sub(budget)).unwrap_or(session.reserve_mib()).max(512);
        return Fit { model: chosen.into(), gpu: Gpu::Fit { margin_mib: margin }, why: format!("{} partly on the GPU ({limit}), the rest on the CPU: slower", label(chosen)) };
    }
    let small = installed.iter().copied().min_by_key(|m| need(m)).unwrap_or(chosen);
    Fit { model: small.into(), gpu: Gpu::Cpu, why: format!("no room on the GPU ({limit}): {} runs on the CPU", label(small)) }
}

/// What [`up`] does, for the log and the tests.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    /// The main slot's model file and where it goes.
    pub main_model: PathBuf,
    pub gpu: Gpu,
    /// The router id (two stages) or None (simple: a running router is stopped).
    pub router: Option<String>,
    /// Why (two stages: the VRAM fit in plain words).
    pub why: String,
}

/// The plan for ai.toml's setting. `cpu`: the simple mode's last choice (`omarchyvr-ai up --cpu`). Two stages: the
/// action model fitted to the free VRAM next to `session` ([`fit`]).
pub fn plan(cfg: &AiConfig, dirs: &Dirs, cpu: bool, free: Option<u32>, session: Session) -> Result<Plan, String> {
    match cfg.voice_local {
        VoiceLocal::Simple => {
            let o = ServeOpts { gpu_layers: cpu.then_some(0), ..Default::default() };
            Ok(Plan { main_model: pick_model(&o, dirs)?, gpu: if cpu { Gpu::Cpu } else { Gpu::All }, router: None, why: "simple mode".into() })
        }
        VoiceLocal::TwoStage => {
            let t = cfg.two_stage();
            let installed: Vec<&str> = omarchyvr_config::ai::ACTION_MODELS.iter().map(|m| m.id).filter(|id| model_installed(dirs, id)).collect();
            let f = fit(&t.action, t.gpu_layers, t.vram_mib, free, session, &installed);
            let a = model_asset(&f.model).ok_or_else(|| format!("{}: not a known action model", f.model))?;
            Ok(Plan { main_model: dirs.model_path(&a), gpu: f.gpu, router: Some(t.router), why: f.why })
        }
    }
}

/// The main slot's options for a plan.
pub fn main_opts(p: &Plan) -> ServeOpts {
    let mut o = ServeOpts { model: Some(p.main_model.clone()), ..Default::default() };
    match &p.gpu {
        Gpu::All => o.gpu_layers = None,
        Gpu::Layers(n) => o.gpu_layers = Some(*n),
        Gpu::Cpu => o.gpu_layers = Some(0),
        Gpu::Fit { margin_mib } => o.fit_margin = Some(*margin_mib),
    }
    o
}

/// Start what ai.toml asks for: the main slot with the right model (restarted when another model or placement runs
/// there), the router in two-stage mode (stopped otherwise), and the VRAM watch when the model is on the GPU. Waits
/// until both answer `/health` (up to `wait`).
pub fn up(cfg: &AiConfig, dirs: &Dirs, cpu: bool, wait: Duration, log: &mut dyn FnMut(&str)) -> Result<(), String> {
    if cfg.voice_local_models == Some(false) {
        for (slot, pid) in down(dirs)? {
            log(&format!("{}: stopped (pid {pid})", slot.id()));
        }
        if let Some(pid) = stop_slot(dirs, Slot::Asr)? {
            log(&format!("speech: whisper-server stopped (pid {pid})"));
        }
        log("Local models are off (AI page): nothing loaded");
        return Ok(());
    }
    let session = detect_session(Path::new("/proc"));
    // our own servers' memory counts as free: a server is replaced if the fit changes
    let own = running(dirs).and_then(|r| gpu_mib(r.pid)).unwrap_or(0);
    let own_asr = running_slot(dirs, Slot::Asr).and_then(|r| gpu_mib(r.pid)).unwrap_or(0);
    let raw = gpu_free_mib().map(|g| g.0);
    // the speech server first: it counts its video memory in the fit of the action model
    let (ap, asr_usable) = asr_decision(cfg, raw.map(|f| f + own + own_asr));
    log(&format!("speech: {}", ap.why));
    if let (true, Some(m)) = (ap.gpu && !asr_usable, asr_missing(&asr_server_bin(), &asr_model_file())) {
        log(&format!("speech: {m}; the small model answers"));
    }
    if asr_usable {
        if running_slot(dirs, Slot::Asr).is_none() {
            match serve_asr(dirs) {
                Ok(s) => log(&format!("speech: started whisper-server (pid {}, port {})", s.pid, s.port)),
                Err(e) => log(&format!("speech: {e}; the small model answers")),
            }
        }
    } else if let Some(pid) = stop_slot(dirs, Slot::Asr)? {
        log(&format!("speech: whisper-server stopped (pid {pid})"));
    }
    let asr_taken = if asr_usable && own_asr == 0 { ASR_VRAM_MIB } else { 0 };
    let free = raw.map(|f| (f + own).saturating_sub(asr_taken));
    let p = plan(cfg, dirs, cpu, free, session)?;
    if !p.main_model.is_file() {
        return Err(format!("{} is not downloaded yet: press Download on the AI page", p.main_model.file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default()));
    }
    let file = p.main_model.file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default();
    let mode = p.gpu.mode();
    log(&format!("fit: {} ({}, free {} MiB)", p.why, session.label(), free.map(|f| f.to_string()).unwrap_or("?".into())));
    if let Some(r) = running(dirs) {
        if r.model == file && r.mode == mode {
            log(&format!("model: {file} already runs ({mode}, pid {})", r.pid));
        } else {
            log(&format!("model: {} ({}) runs; restarting with {file} ({mode})", r.model, r.mode));
            stop(dirs)?;
        }
    }
    if running(dirs).is_none() {
        let s = serve(&main_opts(&p), dirs, false)?;
        log(&format!("model: started {file} ({mode}, pid {}, port {})", s.pid, s.port));
    }
    match &p.router {
        None => {
            if let Some(pid) = stop_slot(dirs, Slot::Router)? {
                log(&format!("router: stopped (pid {pid}); the simple mode needs none"));
            }
            stop_watch(dirs);
        }
        Some(id) => {
            // Lara not tuned yet (the tuning runs after the download): Laya as published meanwhile
            let id = if id == "lara" && !lara_installed(dirs) && laya_installed(dirs) {
                log("router: Lara is not tuned yet; Laya as published runs meanwhile");
                "laya"
            } else {
                id.as_str()
            };
            let want = router_model_name(id);
            if let Some(r) = running_slot(dirs, Slot::Router) {
                if r.model != want {
                    log(&format!("router: {} runs; restarting with {want}", r.model));
                    stop_slot(dirs, Slot::Router)?;
                }
            }
            if running_slot(dirs, Slot::Router).is_none() {
                let s = serve_router(dirs, id, false)?;
                log(&format!("router: started {} (pid {}, port {})", s.model, s.pid, s.port));
            }
        }
    }
    if p.gpu != Gpu::Cpu && p.router.is_some() {
        start_watch(dirs, log);
    }
    agent_slot(cfg, dirs, session, free.map(|f| (f, p.gpu.clone())), log);
    // wait for both to load
    let t0 = std::time::Instant::now();
    loop {
        let main_ok = running(dirs).is_some_and(|r| http_get(r.port, "/health", Duration::from_millis(300)).is_ok_and(|(c, _)| c == 200));
        let router_ok = p.router.is_none() || running_slot(dirs, Slot::Router).is_some_and(|r| http_get(r.port, "/health", Duration::from_millis(500)).is_ok_and(|(c, _)| c == 200));
        // the speech server is a leaf: if it exits, voice falls back to the small model and `up` still succeeds
        let asr_ok = !asr_usable || http_get(ASR_PORT, "/health", Duration::from_millis(500)).is_ok_and(|(c, _)| c == 200) || (running_slot(dirs, Slot::Asr).is_none() && t0.elapsed() >= Duration::from_secs(3));
        if main_ok && router_ok && asr_ok {
            log("ready");
            return Ok(());
        }
        if running(dirs).is_none() {
            return Err(format!("llama-server exited; see {}", dirs.server_log().display()));
        }
        if p.router.is_some() && running_slot(dirs, Slot::Router).is_none() {
            return Err(format!("the router exited; see {}", dirs.slot_log(Slot::Router).display()));
        }
        if t0.elapsed() >= wait {
            log("still loading; check `omarchyvr-ai status`");
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(500));
    }
}

/// The agent-mode model never sits next to SteamVR or a game: only with no session or the Work desk.
pub fn agent_allowed(session: Session) -> bool {
    matches!(session, Session::None | Session::Work)
}

/// The optional agent-mode model (port 8083): started only when the setting asks for it, the file is there, the main
/// model is on the GPU and the free video memory next to the session leaves its reserve. A leaf: any problem is logged
/// and agent mode keeps using the action model. `fit`: (free MiB before the main model, where the main model runs).
fn agent_slot(cfg: &AiConfig, dirs: &Dirs, session: Session, fit: Option<(u32, Gpu)>, log: &mut dyn FnMut(&str)) {
    let Some(id) = cfg.two_stage().agent else {
        if let Ok(Some(pid)) = stop_slot(dirs, Slot::Agent) {
            log(&format!("agent: stopped (pid {pid}); the setting is off"));
        }
        return;
    };
    if running_slot(dirs, Slot::Agent).is_some() {
        return;
    }
    if !agent_allowed(session) {
        log(&format!("agent: {id} is not loaded next to {} (video memory belongs to the session)", session.label()));
        return;
    }
    let Some(a) = model_asset(&id) else { return };
    let Some(m) = omarchyvr_config::ai::local_model(&id) else { return };
    if !dirs.model_path(&a).is_file() {
        log(&format!("agent: {id} is not downloaded yet; agent mode uses the action model"));
        return;
    }
    let main_mib = omarchyvr_config::ai::local_model(&cfg.two_stage().action).map(|x| x.memory_mib).unwrap_or(0);
    // free before the main model (its own memory counted as free), less what the main model takes
    let free = fit.as_ref().map(|f| f.0.saturating_sub(main_mib));
    let need = m.memory_mib + session.reserve_mib();
    match (free, fit.map(|f| f.1)) {
        (Some(f), Some(Gpu::All)) if f >= need => {}
        (f, g) => {
            log(&format!("agent: {id} needs {need} MiB free (it takes {} MiB, the session keeps {}); free {} MiB, main model on {} ({main_mib} MiB): not loaded", m.memory_mib, session.reserve_mib(), f.map(|x| x.to_string()).unwrap_or("?".into()), g.map(|g| g.mode()).unwrap_or("?".into())));
            return;
        }
    }
    let o = ServeOpts { model: Some(dirs.model_path(&a)), port: omarchyvr_config::ai::AGENT_PORT, ctx: 16384, ..Default::default() };
    match serve_slot(&o, dirs, false, Slot::Agent) {
        Ok(s) => log(&format!("agent: started {} (pid {}, port {}); loads in the background", s.model, s.pid, s.port)),
        Err(e) => log(&format!("agent: {e}; agent mode uses the action model")),
    }
}

/// The VRAM watch's pid file.
pub fn watch_pid(dirs: &Dirs) -> PathBuf {
    dirs.state.join("vram-watch.pid")
}

fn watch_alive(dirs: &Dirs) -> Option<i32> {
    let pid: i32 = std::fs::read_to_string(watch_pid(dirs)).ok()?.trim().parse().ok()?;
    let c = std::fs::read(format!("/proc/{pid}/cmdline")).ok()?;
    let c = String::from_utf8_lossy(&c);
    (c.contains("omarchyvr-ai") && c.contains("watch")).then_some(pid)
}

/// Stop the VRAM watch (the simple mode keeps its own placement).
pub fn stop_watch(dirs: &Dirs) {
    if let Some(pid) = watch_alive(dirs) {
        // SAFETY: kill(2) on our own watch process (checked by its command line).
        unsafe { libc::kill(pid, libc::SIGTERM) };
    }
    let _ = std::fs::remove_file(watch_pid(dirs));
}

/// Start `omarchyvr-ai watch` (detached, nice 19) unless it runs.
pub fn start_watch(dirs: &Dirs, log: &mut dyn FnMut(&str)) {
    use std::os::unix::process::CommandExt;
    if watch_alive(dirs).is_some() {
        return;
    }
    let Ok(me) = std::env::current_exe() else { return };
    let _ = std::fs::create_dir_all(&dirs.logs);
    let Ok(f) = std::fs::OpenOptions::new().create(true).append(true).open(dirs.logs.join("ai-vram-watch.log")) else { return };
    let Ok(f2) = f.try_clone() else { return };
    let mut c = Command::new("nice");
    c.args(["-n", "19"]).arg(me).arg("watch").stdin(std::process::Stdio::null()).stdout(f).stderr(f2);
    // SAFETY: setsid is async-signal-safe.
    unsafe {
        c.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }
    match c.spawn() {
        Ok(ch) => {
            let _ = std::fs::write(watch_pid(dirs), ch.id().to_string());
            log(&format!("vram watch: pid {}", ch.id()));
        }
        Err(e) => log(&format!("vram watch: {e}")),
    }
}

/// Should the watch move the model off the GPU now? (free VRAM below half the session's reserve, or a game started
/// while the whole model sits on the GPU and the free memory is under the game reserve).
pub fn must_shrink(free_mib: u32, session: Session, gpu: &str) -> bool {
    gpu != "cpu" && (free_mib < session.reserve_mib() / 2 || (session == Session::Game && gpu.contains("all") && free_mib < Session::Game.reserve_mib()))
}

/// `omarchyvr-ai watch`: every 15 s, while the main model is on the GPU, check the free VRAM against the session; when a
/// game needs the memory, fit the model again (fewer layers, the smaller model or the CPU). It never grows back by
/// itself (the next Play/Work start does), so it cannot flap. Ends when the main server stops.
pub fn watch(dirs: &Dirs) -> Result<(), String> {
    let _ = std::fs::write(watch_pid(dirs), std::process::id().to_string());
    let stamp = || std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    loop {
        std::thread::sleep(Duration::from_secs(15));
        let Some(r) = running(dirs) else {
            let _ = std::fs::remove_file(watch_pid(dirs));
            return Ok(());
        };
        let cfg = AiConfig::load().unwrap_or_default();
        if cfg.voice_local != VoiceLocal::TwoStage {
            // the simple mode keeps its own placement (omarchyvr-ai serve / up); nothing to watch
            let _ = std::fs::remove_file(watch_pid(dirs));
            return Ok(());
        }
        let Some((free, _)) = gpu_free_mib() else { continue };
        let session = detect_session(Path::new("/proc"));
        if !agent_allowed(session) {
            if let Ok(Some(pid)) = stop_slot(dirs, Slot::Agent) {
                eprintln!("[{}] agent model stopped (pid {pid}): {} needs the video memory", stamp(), session.label());
            }
        }
        if !must_shrink(free, session, &r.mode) {
            continue;
        }
        let own = gpu_mib(r.pid).unwrap_or(0);
        let p = plan(&cfg, dirs, false, Some(free + own), session)?;
        eprintln!("[{}] {free} MiB free next to {}: {}", stamp(), session.label(), p.why);
        if p.gpu.mode() == r.mode && p.main_model.file_name().is_some_and(|f| f.to_string_lossy() == r.model) {
            continue;
        }
        stop(dirs)?;
        match serve(&main_opts(&p), dirs, false) {
            Ok(s) => eprintln!("[{}] restarted: {} ({}), pid {}", stamp(), s.model, s.mode, s.pid),
            Err(e) => eprintln!("[{}] restart failed: {e}", stamp()),
        }
        if p.gpu == Gpu::Cpu {
            let _ = std::fs::remove_file(watch_pid(dirs));
            return Ok(());
        }
    }
}

/// Stop the three slots (speech server, router, model).
pub fn down(dirs: &Dirs) -> Result<Vec<(Slot, i32)>, String> {
    let mut v = Vec::new();
    stop_watch(dirs);
    for s in [Slot::Agent, Slot::Asr, Slot::Router, Slot::Main] {
        if let Some(pid) = stop_slot(dirs, s)? {
            v.push((s, pid));
        }
    }
    Ok(v)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dirs(tag: &str) -> Dirs {
        let base = std::env::temp_dir().join(format!("omarchyvr-ai-ts-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        Dirs { data: base.join("data"), state: base.join("state"), logs: base.join("logs") }
    }

    #[test]
    fn asr_server_command_and_missing_files() {
        let a = asr_argv(Path::new("/b/whisper-server"), Path::new("/m/g.bin"), 8082);
        let line = a.join(" ");
        assert!(line.starts_with("nice -n 19 ionice"), "{line}");
        assert!(line.contains("-m /m/g.bin --host 127.0.0.1 --port 8082 -nc -l en -nt"), "{line}");
        let m = asr_missing(Path::new("/nope/whisper-server"), Path::new("/nope/m.bin")).unwrap();
        assert!(m.contains("not built"), "{m}");
        assert!(asr_missing(Path::new("/proc/self/exe"), Path::new("/nope/m.bin")).unwrap().contains("not downloaded"));
        assert_eq!(asr_missing(Path::new("/proc/self/exe"), Path::new("/proc/self/exe")), None);
    }

    #[test]
    fn asr_slot_has_its_own_pid_file_and_log() {
        let d = dirs("asr");
        assert!(d.slot_pid(Slot::Asr).ends_with("asr.pid") && d.slot_log(Slot::Asr).ends_with("ai-asr.log"));
        assert_eq!(Slot::Asr.port(), 8082);
        assert!(running_slot(&d, Slot::Asr).is_none());
    }

    #[test]
    fn asr_decision_follows_the_switch() {
        let mut c = AiConfig::default();
        assert!(!asr_decision(&c, Some(20000)).0.gpu, "the default is the processor: no GPU whisper");
        assert!(!asr_decision(&c, Some(20000)).1, "and the server is not wanted");
        c.asr = omarchyvr_config::ai::Asr::Gpu;
        assert!(asr_decision(&c, Some(20000)).0.gpu);
        c.voice_local_models = Some(false);
        assert!(!asr_decision(&c, Some(20000)).0.gpu, "Local models Off");
    }

    #[test]
    fn plans_follow_ai_toml() {
        let d = dirs("plan");
        std::fs::create_dir_all(d.models()).unwrap();
        std::fs::write(d.model_path(&MODEL_GPU), vec![0u8; 0]).unwrap();
        let simple = AiConfig::parse("").unwrap();
        let p = plan(&simple, &d, true, Some(20000), Session::Work).unwrap();
        assert_eq!((p.main_model, p.gpu, p.router), (d.model_path(&MODEL_GPU), Gpu::Cpu, None));
        let two = AiConfig::parse("voice_local = \"two-stage\"\n[local_two_stage]\nrouter = \"laya\"\naction = \"qwen3.5-9b\"\n").unwrap();
        let p = plan(&two, &d, true, Some(20000), Session::Work).unwrap();
        assert_eq!((p.main_model, p.gpu, p.router.as_deref()), (d.model_path(&MODEL_ACTION_9B), Gpu::All, Some("laya")), "two stages: the GPU, whatever --cpu said");
        let o = main_opts(&Plan { main_model: "m".into(), gpu: Gpu::Fit { margin_mib: 3000 }, router: None, why: String::new() });
        assert_eq!((o.gpu_layers, o.fit_margin), (None, Some(3000)));
        let s = serve_argv(Path::new("s"), Path::new("m"), &o).join(" ");
        assert!(s.contains("--fit on --fit-target 3000") && !s.contains("-ngl"), "{s}");
        let _ = std::fs::remove_dir_all(d.data.parent().unwrap());
    }

    #[test]
    fn vram_fit_is_adaptive() {
        let both = ["qwen3.5-4b", "qwen3.5-9b"];
        // Auto, Work desk, 21 GB free: the chosen 9B, whole
        let f = fit("qwen3.5-9b", None, None, Some(21000), Session::Work, &both);
        assert_eq!((f.model.as_str(), &f.gpu), ("qwen3.5-9b", &Gpu::All), "{}", f.why);
        // a limit of 4 GB: the 4B instead
        let f = fit("qwen3.5-9b", None, Some(4096), Some(21000), Session::Work, &both);
        assert_eq!((f.model.as_str(), &f.gpu), ("qwen3.5-4b", &Gpu::All));
        assert!(f.why.contains("4.0 GB limit"), "{}", f.why);
        // SteamVR keeps 8 GB for a game: 11 GB free leaves 2.8 GB: the 4B only partly on the GPU
        let f = fit("qwen3.5-4b", None, None, Some(11000), Session::SteamVr, &both);
        assert_eq!(f.gpu, Gpu::Fit { margin_mib: 8192 }, "{}", f.why);
        assert!(f.why.contains("partly"));
        // a game left 3.5 GB: no room: the CPU, with the smallest model
        let f = fit("qwen3.5-9b", None, None, Some(3500), Session::Game, &both);
        assert_eq!((f.model.as_str(), &f.gpu), ("qwen3.5-4b", &Gpu::Cpu), "{}", f.why);
        // hand-set layers win; unknown free memory trusts the limit
        assert_eq!(fit("qwen3.5-9b", Some(0), None, Some(20000), Session::None, &both).gpu, Gpu::Cpu);
        assert_eq!(fit("qwen3.5-9b", Some(20), None, None, Session::None, &both).gpu, Gpu::Layers(20));
        assert_eq!(fit("qwen3.5-9b", None, None, None, Session::None, &both).gpu, Gpu::All);
        assert_eq!(parse_free("21890, 24564\n"), Some((21890, 24564)));
        assert_eq!(parse_free(""), None);
        // the watch: a game squeezed the free memory
        assert!(must_shrink(900, Session::Game, "gpu (all layers)"));
        assert!(!must_shrink(900, Session::Game, "cpu"));
        assert!(!must_shrink(6000, Session::Work, "gpu (all layers)"));
    }

    #[test]
    fn sessions_from_the_process_list() {
        let root = std::env::temp_dir().join(format!("omarchyvr-ai-proc-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let add = |pid: u32, cmd: &str| {
            let d = root.join(pid.to_string());
            std::fs::create_dir_all(&d).unwrap();
            std::fs::write(d.join("cmdline"), cmd.replace('|', "\0")).unwrap();
        };
        add(1, "/usr/lib/systemd/systemd");
        assert_eq!(detect_session(&root), Session::None);
        add(10, "/home/u/src/target-desk/release/wayvr|--openxr");
        assert_eq!(detect_session(&root), Session::Work);
        add(20, "/home/u/.local/share/Steam/steamapps/common/SteamVR/bin/linux64/vrserver|--keepalive");
        assert_eq!(detect_session(&root), Session::SteamVr, "SteamVR's own files are not a game");
        add(30, "/home/u/.local/share/Steam/steamapps/common/Beat Saber/Beat Saber.exe");
        assert_eq!(detect_session(&root), Session::Game);
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn laya_runs_local_offline_and_cpu_only() {
        let d = dirs("laya");
        let c = laya_command(&d, 8, false);
        let env: Vec<(String, String)> = c.get_envs().map(|(k, v)| (k.to_string_lossy().into(), v.map(|v| v.to_string_lossy().into_owned()).unwrap_or_default())).collect();
        let get = |k: &str| env.iter().find(|(x, _)| x == k).map(|(_, v)| v.as_str());
        assert_eq!(get("LAYA_HOST"), Some("127.0.0.1"));
        assert_eq!(get("LAYA_PORT"), Some("8081"));
        assert_eq!(get("LAYA_DEVICE"), Some("cpu"));
        assert_eq!(get("CUDA_VISIBLE_DEVICES"), Some(""));
        assert_eq!(get("HF_HUB_OFFLINE"), Some("1"));
        assert_eq!(get("LAYA_REVISION"), Some("reviewed"));
        let args: Vec<String> = c.get_args().map(|a| a.to_string_lossy().into_owned()).collect();
        assert_eq!(c.get_program(), "nice");
        assert_eq!(&args[..4], ["-n", "19", "ionice", "-c"]);
        assert!(!laya_installed(&d) && !lara_installed(&d));
        let t = laya_command(&d, 8, true);
        let extra = t.get_envs().find(|(k, _)| *k == "LAYA_EXTRA_MODELS").and_then(|(_, v)| v).map(|v| v.to_string_lossy().into_owned()).unwrap();
        assert!(extra.starts_with("{\"lara\": \"") && extra.ends_with("laya/lara\"}"), "{extra}");
        assert_eq!(router_model_name("lara"), "laya:lara");
        assert!(LAYA_CONSTRAINTS.lines().all(|l| l.contains("==")), "every package pinned");
        assert!(LAYA_CONSTRAINTS.contains(&format!("laya=={LAYA_VERSION}")));
        assert_eq!(laya_snapshot(&d), d.data.join("laya/hf/hub/models--convaiinnovations--laya/snapshots").join(LAYA_REVISION));
    }

    #[test]
    fn the_agent_model_is_opt_in_and_never_next_to_a_game() {
        // nothing in a default file, a preset or the Work/Play switches asks for it
        assert_eq!(AiConfig::parse("").unwrap().two_stage().agent, None);
        let mut t = omarchyvr_config::ai::LocalTwoStage::default();
        t.apply(omarchyvr_config::ai::Preset::High);
        assert_eq!(t.agent, None);
        assert!(agent_allowed(Session::None) && agent_allowed(Session::Work));
        assert!(!agent_allowed(Session::SteamVr) && !agent_allowed(Session::Game));
    }

    #[test]
    fn every_catalogue_model_has_a_download() {
        for m in omarchyvr_config::ai::ROUTER_MODELS.iter().chain(omarchyvr_config::ai::ACTION_MODELS.iter()).chain(omarchyvr_config::ai::AGENT_MODELS.iter()) {
            match m.engine {
                LocalEngine::Laya => assert!(m.id == "laya" || m.id == "lara"),
                LocalEngine::Llama => {
                    let a = model_asset(m.id).unwrap_or_else(|| panic!("{} has no asset", m.id));
                    assert!(a.url.starts_with("https://huggingface.co/unsloth/") && a.url.contains("/resolve/") && !a.url.contains("/resolve/main/"), "{}: a pinned revision", a.url);
                    assert!(a.sha256.len() == 64 && a.url.ends_with(a.file.rsplit('/').next().unwrap()));
                    assert!((a.size >> 20).abs_diff(m.download_mib as u64) < 64, "{}: catalogue size {} vs {}", m.id, m.download_mib, a.size >> 20);
                }
            }
        }
    }
}
