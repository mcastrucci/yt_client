//! **Local dependencies** under `dependencies/mac/` or `dependencies/windows/` (OS-specific) are preferred:
//! - `yt-dlp` or `youtube-dl` (required)
//! - `ffmpeg` + `ffprobe` in that folder, or under `…/bin/`
//! - `deno` and/or `node` (≥20) in the same folder or `…/bin/` (YouTube JS; optional but recommended)
//! Packaged apps bundle both platform folders from the repo (see `tauri.conf.json`); at runtime only the matching OS folder is used.

use std::io::{BufRead, BufReader, ErrorKind, Read, Write};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::thread::JoinHandle;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tauri::path::BaseDirectory;
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_dialog::DialogExt;
use tauri_plugin_opener::OpenerExt;
use url::Url;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DownloadMode {
    Video,
    Audio,
}

impl DownloadMode {
    fn parse(s: &str) -> Result<Self, String> {
        match s.to_ascii_lowercase().as_str() {
            "video" | "v" => Ok(DownloadMode::Video),
            "audio" | "a" => Ok(DownloadMode::Audio),
            _ => Err(format!(
                "Formato no reconocido: {s}. Usa «video» o «audio»."
            )),
        }
    }
}

/// Prefer [yt-dlp](https://github.com/yt-dlp/yt-dlp) (maintained; YouTube works). youtube-dl is mostly unmaintained and often breaks.
fn candidate_names() -> &'static [&'static str] {
    if cfg!(target_os = "windows") {
        &["yt-dlp.exe", "youtube-dl.exe"]
    } else {
        &["yt-dlp", "youtube-dl"]
    }
}

fn first_existing_in_dir(dir: &Path) -> Option<PathBuf> {
    for name in candidate_names() {
        let p = dir.join(name);
        if p.is_file() {
            return Some(p);
        }
    }
    None
}

/// `dependencies/mac` on macOS, `dependencies/windows` on Windows (repo root, next to `app/`).
fn deps_platform_dir_name() -> &'static str {
    if cfg!(target_os = "windows") {
        "windows"
    } else {
        "mac"
    }
}

/// `tauri dev` copies `../../dependencies/{mac|windows}/` into the app bundle. Prefer the real repo folder while developing.
fn try_dev_dependencies() -> Option<(PathBuf, PathBuf)> {
    let base = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../dependencies")
        .join(deps_platform_dir_name());
    let file = first_existing_in_dir(&base)?;
    Some((base, file))
}

/// True for the legacy `youtube-dl` Python script (not for standalone `yt-dlp` binaries).
fn file_looks_like_python_shebang(path: &Path) -> bool {
    let Ok(mut f) = std::fs::File::open(path) else {
        return false;
    };
    let mut buf = [0u8; 96];
    let n = f.read(&mut buf).unwrap_or(0);
    if n < 2 || buf[0] != b'#' || buf[1] != b'!' {
        return false;
    }
    let s = String::from_utf8_lossy(&buf[..n]);
    s.to_ascii_lowercase().contains("python")
}

/// macOS OpenSSL: often no trust store. Less safe on untrusted networks.
const NO_CHECK_CERT: &str = "--no-check-certificate";
/// Single video when a watch URL has `&list=…` (e.g. Mix / RD…). Ignored for raw `playlist?list=` URLs.
const NO_PLAYLIST: &str = "--no-playlist";
/// After one finished download, stop (playlist pages would otherwise fetch every entry).
const MAX_DOWNLOADS: &str = "--max-downloads";

fn ffmpeg_exe_name() -> &'static str {
    if cfg!(target_os = "windows") {
        "ffmpeg.exe"
    } else {
        "ffmpeg"
    }
}

fn ffprobe_exe_name() -> &'static str {
    if cfg!(target_os = "windows") {
        "ffprobe.exe"
    } else {
        "ffprobe"
    }
}

fn deno_exe_name() -> &'static str {
    if cfg!(target_os = "windows") {
        "deno.exe"
    } else {
        "deno"
    }
}

fn node_exe_name() -> &'static str {
    if cfg!(target_os = "windows") {
        "node.exe"
    } else {
        "node"
    }
}

/// First `name` found on `PATH` (must be a file).
fn first_on_path(name: &str) -> Option<PathBuf> {
    let path_var = std::env::var_os("PATH")?;
    let sep = if cfg!(target_os = "windows") { ';' } else { ':' };
    for dir in path_var.to_string_lossy().split(sep) {
        let dir = dir.trim();
        if dir.is_empty() {
            continue;
        }
        let p = Path::new(dir).join(name);
        if p.is_file() {
            return Some(p);
        }
    }
    None
}

fn ff_dir_and_probe_ok(dir: &Path) -> bool {
    dir.join(ffmpeg_exe_name()).is_file() && dir.join(ffprobe_exe_name()).is_file()
}

/// `dependencies/…/bin` or the platform folder itself: `ffmpeg`, `ffprobe`, optional `deno` next to `yt-dlp`.
fn bundled_ffmpeg_dir(deps_dir: &Path) -> Option<PathBuf> {
    for dir in [deps_dir, &deps_dir.join("bin")] {
        if ff_dir_and_probe_ok(dir) {
            return Some(dir.to_path_buf());
        }
    }
    None
}

/// Directory for `--ffmpeg-location`: **bundled** `dependencies/…` first, then `PATH` / common Homebrew paths.
fn resolve_ffmpeg_bin_dir(deps_dir: &Path) -> Option<PathBuf> {
    if let Some(d) = bundled_ffmpeg_dir(deps_dir) {
        return Some(d);
    }
    if let Some(ffmpeg) = first_on_path(ffmpeg_exe_name()) {
        if let Some(dir) = ffmpeg.parent() {
            if ff_dir_and_probe_ok(dir) {
                return Some(dir.to_path_buf());
            }
        }
    }
    #[cfg(unix)]
    {
        for d in [
            "/opt/homebrew/bin",
            "/usr/local/bin",
            "/opt/homebrew/opt/ffmpeg/bin",
        ] {
            let p = Path::new(d);
            if ff_dir_and_probe_ok(p) {
                return Some(p.to_path_buf());
            }
        }
    }
    None
}

/// `deno:path` for `--js-runtimes` (bundled `dependencies/…`, then `PATH`, then common Homebrew paths on macOS).
fn resolve_deno_js_flag(deps_dir: &Path) -> Option<String> {
    for base in [deps_dir, &deps_dir.join("bin")] {
        let p = base.join(deno_exe_name());
        if p.is_file() {
            return Some(format!("deno:{}", p.display()));
        }
    }
    if let Some(deno) = first_on_path(deno_exe_name()) {
        return Some(format!("deno:{}", deno.display()));
    }
    #[cfg(unix)]
    {
        let p = Path::new("/opt/homebrew/bin").join(deno_exe_name());
        if p.is_file() {
            return Some(format!("deno:{}", p.display()));
        }
    }
    None
}

/// `node:path` for `--js-runtimes` (same search order as Deno).
fn resolve_node_js_flag(deps_dir: &Path) -> Option<String> {
    for base in [deps_dir, &deps_dir.join("bin")] {
        let p = base.join(node_exe_name());
        if p.is_file() {
            return Some(format!("node:{}", p.display()));
        }
    }
    if let Some(node) = first_on_path(node_exe_name()) {
        return Some(format!("node:{}", node.display()));
    }
    #[cfg(unix)]
    {
        for d in ["/opt/homebrew/bin", "/usr/local/bin"] {
            let p = Path::new(d).join(node_exe_name());
            if p.is_file() {
                return Some(format!("node:{}", p.display()));
            }
        }
    }
    None
}

/// Pass one or more `--js-runtimes` flags (yt-dlp order: Deno preferred over Node when both exist).
fn append_js_runtime_args(cmd: &mut std::process::Command, deps_dir: &Path) {
    if let Some(flag) = resolve_deno_js_flag(deps_dir) {
        cmd.arg("--js-runtimes").arg(flag);
    }
    if let Some(flag) = resolve_node_js_flag(deps_dir) {
        cmd.arg("--js-runtimes").arg(flag);
    }
}

/// Shared yt-dlp / youtube-dl CLI: TLS, playlist, optional ffmpeg + JS runtimes, output template, mode, URL.
/// `work_dir` is the resolved `dependencies/{mac|windows}` folder (tooling + `current_dir`).
/// `output_dir` is where media files are written (`-o` template).
fn apply_mode_and_url(
    cmd: &mut std::process::Command,
    mode: DownloadMode,
    url: &str,
    work_dir: &Path,
    output_dir: &Path,
) {
    cmd
        .arg(NO_CHECK_CERT)
        .arg(NO_PLAYLIST)
        .arg(MAX_DOWNLOADS)
        .arg("1");
    let out_tpl = output_dir.join("%(title)s [%(id)s].%(ext)s");
    cmd.arg("-o").arg(out_tpl);
    if let Some(dir) = resolve_ffmpeg_bin_dir(work_dir) {
        cmd.arg("--ffmpeg-location").arg(dir);
    }
    append_js_runtime_args(cmd, work_dir);
    if mode == DownloadMode::Audio {
        cmd.arg("-x").arg("--audio-format").arg("mp3");
    }
    cmd.arg(url);
}

/// Parse a percentage from yt-dlp / youtube-dl progress lines, e.g. `[download]  45.2% of …`
fn percent_from_line(line: &str) -> Option<f64> {
    for token in line.split_whitespace() {
        if let Some(num) = token.strip_suffix('%') {
            if let Ok(v) = num.parse::<f64>() {
                return Some(v.clamp(0.0, 100.0));
            }
        }
    }
    // e.g. percentage immediately before `%` with odd spacing
    if let Some(idx) = line.find('%') {
        let mut start = idx;
        while start > 0 {
            let c = line.as_bytes()[start - 1];
            if (c >= b'0' && c <= b'9') || c == b'.' {
                start -= 1;
            } else {
                break;
            }
        }
        if start < idx {
            if let Ok(v) = line[start..idx].parse::<f64>() {
                return Some(v.clamp(0.0, 100.0));
            }
        }
    }
    None
}

/// yt-dlp uses stderr from a side thread; `emit` must run on the main thread or the webview never updates.
fn emit_download_progress(app: &AppHandle, line: &str) {
    if line.is_empty() {
        return;
    }
    let pct = percent_from_line(line);
    let payload = DownloadProgress {
        line: line.to_string(),
        percent: pct,
    };
    let app_sched = app.clone();
    let app_emit = app.clone();
    let _ = app_sched.run_on_main_thread(move || {
        let _ = app_emit.emit("download-progress", payload);
    });
}

#[derive(Clone, Serialize)]
struct DownloadProgress {
    line: String,
    /// `Some(0..=100)` when a percentage was found in the line
    percent: Option<f64>,
}

fn read_stdout_thread(mut pipe: std::process::ChildStdout) -> JoinHandle<Result<Vec<u8>, String>> {
    std::thread::spawn(move || {
        let mut v = Vec::new();
        pipe
            .read_to_end(&mut v)
            .map_err(|e| e.to_string())?;
        Ok(v)
    })
}

fn read_stderr_with_events(
    app: AppHandle,
    pipe: std::process::ChildStderr,
) -> JoinHandle<Result<Vec<u8>, String>> {
    std::thread::spawn(move || {
        let mut collected = Vec::<u8>::new();
        let reader = BufReader::new(pipe);
        for line in reader.lines() {
            let line = line.map_err(|e| e.to_string())?;
            if !line.is_empty() {
                emit_download_progress(&app, &line);
            }
            collected.extend_from_slice(line.as_bytes());
            collected.push(b'\n');
        }
        Ok(collected)
    })
}

/// Spawn, stream stderr to the UI, collect full output for the result log.
fn run_downloader_with_events(
    app: &AppHandle,
    work_dir: PathBuf,
    output_dir: PathBuf,
    tool: PathBuf,
    url: String,
    mode: DownloadMode,
) -> Result<std::process::Output, String> {
    #[cfg(windows)]
    {
        let mut c = std::process::Command::new(&tool);
        c.current_dir(&work_dir);
        apply_mode_and_url(&mut c, mode, &url, &work_dir, &output_dir);
        c.stdin(Stdio::null());
        let child = c
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| e.to_string())?;
        return spawn_drain(app, child);
    }

    #[cfg(unix)]
    {
        if file_looks_like_python_shebang(&tool) {
            let mut last: Option<String> = None;
            for py in ["python3", "python"] {
                let mut c = std::process::Command::new(py);
                c.current_dir(&work_dir).arg(&tool);
                apply_mode_and_url(&mut c, mode, &url, &work_dir, &output_dir);
                c.stdin(Stdio::null());
                match c
                    .stdout(Stdio::piped())
                    .stderr(Stdio::piped())
                    .spawn()
                {
                    Ok(child) => return spawn_drain(app, child),
                    Err(e) if e.kind() == ErrorKind::NotFound => {
                        last = Some(format!("{py} no está en el PATH"));
                        continue;
                    }
                    Err(e) => return Err(e.to_string()),
                }
            }
            return Err(last.unwrap_or_else(|| {
                "No se encuentra python3 ni python en el PATH. Instala Python 3 (solo hace falta para el script antiguo youtube-dl).".to_string()
            }));
        }

        let mut c = std::process::Command::new(&tool);
        c.current_dir(&work_dir);
        apply_mode_and_url(&mut c, mode, &url, &work_dir, &output_dir);
        c.stdin(Stdio::null());
        c.stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| e.to_string())
            .and_then(|child| spawn_drain(app, child))
    }
}

fn spawn_drain(
    app: &AppHandle,
    mut child: std::process::Child,
) -> Result<std::process::Output, String> {
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "downloader: missing stdout pipe".to_string())?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| "downloader: missing stderr pipe".to_string())?;

    let h_out = read_stdout_thread(stdout);
    let app2 = app.clone();
    let h_err = read_stderr_with_events(app2, stderr);

    let out_bytes = h_out
        .join()
        .map_err(|_| "stdout reader thread panicked".to_string())??;
    let err_bytes = h_err
        .join()
        .map_err(|_| "stderr reader thread panicked".to_string())??;

    let status = child
        .wait()
        .map_err(|e| format!("downloader: wait failed: {e}"))?;

    Ok(std::process::Output {
        status,
        stdout: out_bytes,
        stderr: err_bytes,
    })
}

/// (`work_dir`, path to yt-dlp or youtube-dl) for the current OS (`dependencies/mac` or `dependencies/windows`).
fn resolve_downloader(app: &AppHandle) -> Result<(PathBuf, PathBuf), String> {
    #[cfg(debug_assertions)]
    if let Some((dir, file)) = try_dev_dependencies() {
        return Ok((dir, file));
    }

    let sub = deps_platform_dir_name();
    for name in candidate_names() {
        let rel = format!("dependencies/{}/{}", sub, name);
        if let Ok(p) = app.path().resolve(&rel, BaseDirectory::Resource) {
            if p.is_file() {
                if let Some(dir) = p.parent() {
                    return Ok((dir.to_path_buf(), p));
                }
            }
        }
    }

    let names = candidate_names().join(", ");
    #[cfg(debug_assertions)]
    {
        let dev = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../dependencies")
            .join(sub);
        return Err(format!(
            "No hay descargador en {}. Añade uno de [{}] ahí. Se recomienda yt-dlp (https://github.com/yt-dlp/yt-dlp#release-files ).",
            dev.display(),
            names
        ));
    }

    #[cfg(not(debug_assertions))]
    Err(format!(
        "No hay descargador incluido. Debe existir uno de [{}] en Resources, carpeta dependencies/{}/.",
        names, sub
    ))
}

#[derive(Debug, Serialize, Deserialize, Default)]
struct AppConfig {
    #[serde(default)]
    download_dir: Option<String>,
    /// After the UI shows the welcome log once, this is set true.
    #[serde(default)]
    first_run_tip_acknowledged: bool,
}

/// Only YouTube watch / Shorts / Music / playlist pages — not arbitrary sites passed to yt-dlp.
fn ensure_youtube_download_url(url_str: &str) -> Result<(), String> {
    let u = Url::parse(url_str.trim()).map_err(|_| {
        "Usa un enlace completo que empiece por https:// (cópialo de la barra de direcciones o de Compartir en YouTube)."
            .to_string()
    })?;
    match u.scheme() {
        "http" | "https" => {}
        _ => return Err("Solo se permiten enlaces http o https.".into()),
    }
    let host = u
        .host_str()
        .ok_or_else(|| "Ese enlace no incluye el nombre del sitio web.".to_string())?;
    let host = host.trim_end_matches('.').to_ascii_lowercase();

    if host == "youtu.be" {
        return Ok(());
    }
    if host == "youtube.com" || host.ends_with(".youtube.com") {
        return Ok(());
    }
    if host == "youtube-nocookie.com" || host.ends_with(".youtube-nocookie.com") {
        return Ok(());
    }

    Err(
        "Solo funcionan enlaces de YouTube (youtube.com, youtu.be, Música, Shorts). Pega un enlace de YouTube."
            .into(),
    )
}

fn download_error_log_path(app: &AppHandle) -> Result<PathBuf, String> {
    let dir = app.path().app_log_dir().map_err(|e| e.to_string())?;
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir.join("download-errors.log"))
}

fn append_download_failure_log(app: &AppHandle, message: &str) -> Option<PathBuf> {
    let path = download_error_log_path(app).ok()?;
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let block = format!("\n--- {stamp} ---\n{message}\n");
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .and_then(|mut f| f.write_all(block.as_bytes()))
        .ok()?;
    Some(path)
}

fn unquote_ytdlp_path(s: &str) -> PathBuf {
    PathBuf::from(s.trim().trim_matches('"').trim_matches('\''))
}

/// yt-dlp often wraps lines in ANSI color codes; matching raw `[download]` fails without this.
fn strip_ansi(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == 0x1b {
            if i + 1 < bytes.len() && bytes[i + 1] == b'[' {
                i += 2;
                while i < bytes.len() && !(0x40..=0x7e).contains(&bytes[i]) {
                    i += 1;
                }
                if i < bytes.len() {
                    i += 1;
                }
            } else {
                i += 1;
            }
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Best effort: rutas en stdout/stderr (con o sin colores). Incluye “ya descargado” y audio sin reconvertir.
fn infer_output_path_from_downloader_log(stderr: &str, stdout: &str) -> Option<PathBuf> {
    let mut last: Option<PathBuf> = None;
    for line in stderr.lines().chain(stdout.lines()) {
        let t = strip_ansi(line).trim().to_string();
        if let Some(rest) = t.strip_prefix("[download] Destination: ") {
            last = Some(unquote_ytdlp_path(rest));
        } else if t.contains("[download]") {
            if let Some(idx) = t.find("Destination: ") {
                let rest = &t[idx + "Destination: ".len()..];
                last = Some(unquote_ytdlp_path(rest));
            } else if let Some(after_bracket) = t.strip_prefix("[download] ") {
                if let Some(p) = after_bracket.strip_suffix(" has already been downloaded") {
                    last = Some(unquote_ytdlp_path(p));
                } else if let Some(p) = after_bracket.strip_suffix(" has been downloaded") {
                    last = Some(unquote_ytdlp_path(p));
                }
            }
        } else if let Some(rest) = t.strip_prefix("[ExtractAudio] Destination: ") {
            last = Some(unquote_ytdlp_path(rest));
        } else if t.contains("[ExtractAudio]") {
            if let Some(idx) = t.find("Destination: ") {
                let rest = &t[idx + "Destination: ".len()..];
                last = Some(unquote_ytdlp_path(rest));
            } else if let Some(idx) = t.find("Not converting audio ") {
                let after = &t[idx + "Not converting audio ".len()..];
                if let Some(semi) = after.find(';') {
                    last = Some(unquote_ytdlp_path(&after[..semi]));
                }
            }
        } else if let Some(rest) = t.strip_prefix("[Merger] Merging formats into ") {
            last = Some(unquote_ytdlp_path(rest));
        } else if t.contains("[Merger]") && t.contains("Merging formats into") {
            if let Some(idx) = t.find("Merging formats into ") {
                let rest = &t[idx + "Merging formats into ".len()..];
                last = Some(unquote_ytdlp_path(rest));
            }
        }
    }
    last.filter(|p| p.is_file())
}

/// Directory that contains the running executable (same idea as the app install folder on desktop).
fn app_containing_dir() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()))
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_else(|| PathBuf::from("."))
}

fn config_json_path(app: &AppHandle) -> Result<PathBuf, String> {
    let dir = app.path().app_config_dir().map_err(|e| e.to_string())?;
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir.join("config.json"))
}

fn read_app_config(app: &AppHandle) -> AppConfig {
    let Ok(path) = config_json_path(app) else {
        return AppConfig::default();
    };
    let Ok(data) = std::fs::read_to_string(path) else {
        return AppConfig::default();
    };
    serde_json::from_str(&data).unwrap_or_default()
}

fn write_app_config(app: &AppHandle, cfg: &AppConfig) -> Result<(), String> {
    let path = config_json_path(app)?;
    let data = serde_json::to_string_pretty(cfg).map_err(|e| e.to_string())?;
    std::fs::write(path, data).map_err(|e| e.to_string())
}

fn stored_download_dir_path(app: &AppHandle) -> Option<PathBuf> {
    let s = read_app_config(app).download_dir?;
    let p = PathBuf::from(s.trim());
    if p.as_os_str().is_empty() {
        return None;
    }
    Some(p)
}

/// Saved folder when it still exists; otherwise the app’s install directory.
fn effective_download_dir(app: &AppHandle) -> Result<PathBuf, String> {
    if let Some(p) = stored_download_dir_path(app) {
        if p.is_dir() {
            return Ok(p);
        }
    }
    let fallback = app_containing_dir();
    if !fallback.is_dir() {
        return Err(format!(
            "La carpeta de descarga por defecto no es válida: {}.",
            fallback.display()
        ));
    }
    Ok(fallback)
}

fn display_path(p: &Path) -> String {
    p.to_string_lossy().to_string()
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct BootstrapUi {
    download_folder: String,
    show_first_run_tip: bool,
}

#[tauri::command]
fn bootstrap_ui(app: AppHandle) -> Result<BootstrapUi, String> {
    let download_folder = display_path(&effective_download_dir(&app)?);
    let cfg = read_app_config(&app);
    Ok(BootstrapUi {
        download_folder,
        show_first_run_tip: !cfg.first_run_tip_acknowledged,
    })
}

#[tauri::command]
fn acknowledge_first_run_tip(app: AppHandle) -> Result<(), String> {
    let mut cfg = read_app_config(&app);
    cfg.first_run_tip_acknowledged = true;
    write_app_config(&app, &cfg)
}

#[tauri::command]
fn open_downloads_folder(app: AppHandle) -> Result<(), String> {
    let dir = effective_download_dir(&app)?;
    app.opener()
        .open_path(display_path(&dir), None::<&str>)
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn open_downloaded_file(app: AppHandle, path: String) -> Result<(), String> {
    let p = PathBuf::from(path.trim());
    if !p.is_file() {
        return Err("No se encuentra el archivo (quizá se movió o borró).".into());
    }
    let path_s = display_path(&p);
    let (tx, rx) = std::sync::mpsc::channel::<Result<(), String>>();
    let app_c = app.clone();
    app.run_on_main_thread(move || {
        let r = app_c
            .opener()
            .open_path(path_s, None::<&str>)
            .map_err(|e| e.to_string());
        let _ = tx.send(r);
    })
    .map_err(|e| format!("No se pudo abrir: {e}"))?;
    match rx.recv_timeout(Duration::from_secs(20)) {
        Ok(Ok(())) => Ok(()),
        Ok(Err(e)) => Err(e),
        Err(_) => Err(
            "Tiempo de espera al abrir el archivo (intenta de nuevo o usa «Abrir carpeta»)."
                .into(),
        ),
    }
}

#[tauri::command]
fn reveal_downloaded_file(app: AppHandle, path: String) -> Result<(), String> {
    let p = PathBuf::from(path.trim());
    if !p.exists() {
        return Err("Esa ruta ya no existe.".into());
    }
    let path_for_thread = p.clone();
    let (tx, rx) = std::sync::mpsc::channel::<Result<(), String>>();
    let app_c = app.clone();
    app.run_on_main_thread(move || {
        let r = app_c
            .opener()
            .reveal_item_in_dir(&path_for_thread)
            .map_err(|e| e.to_string());
        let _ = tx.send(r);
    })
    .map_err(|e| format!("No se pudo mostrar en el explorador: {e}"))?;
    match rx.recv_timeout(Duration::from_secs(20)) {
        Ok(Ok(())) => Ok(()),
        Ok(Err(e)) => Err(e),
        Err(_) => Err(
            "Tiempo de espera al mostrar la carpeta (usa «Abrir carpeta» en Salida)."
                .into(),
        ),
    }
}

#[tauri::command]
fn get_download_folder(app: AppHandle) -> Result<String, String> {
    Ok(display_path(&effective_download_dir(&app)?))
}

#[tauri::command]
async fn pick_download_folder(app: AppHandle) -> Result<Option<String>, String> {
    let start_dir = app_containing_dir();
    let app_handle = app.clone();
    let picked = tauri::async_runtime::spawn_blocking(move || {
        app_handle
            .dialog()
            .file()
            .set_title("Elegir carpeta de descargas")
            .set_directory(&start_dir)
            .blocking_pick_folder()
    })
    .await
    .map_err(|e| e.to_string())?;

    let Some(fp) = picked else {
        return Ok(None);
    };
    let path = fp
        .as_path()
        .ok_or_else(|| "No se pudo usar esa carpeta como ruta local.".to_string())?
        .to_path_buf();
    if !path.is_dir() {
        return Err("La ruta elegida no es una carpeta.".into());
    }
    let to_store = path.canonicalize().unwrap_or(path);
    let mut cfg = read_app_config(&app);
    cfg.download_dir = Some(display_path(&to_store));
    write_app_config(&app, &cfg)?;
    Ok(Some(display_path(&to_store)))
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct DownloadYoutubeResponse {
    log: String,
    output_path: Option<String>,
}

#[tauri::command]
async fn download_youtube(
    app: AppHandle,
    url: String,
    mode: String,
) -> Result<DownloadYoutubeResponse, String> {
    let url = url.trim().to_string();
    if url.is_empty() {
        return Err("Pega un enlace de YouTube arriba y pulsa Descargar.".into());
    }
    if let Err(e) = ensure_youtube_download_url(&url) {
        let path_note = append_download_failure_log(&app, &e)
            .map(|p| format!("\n\nTambién guardado en:\n{}", p.display()))
            .unwrap_or_default();
        return Err(format!("{e}{path_note}"));
    }
    let mode = DownloadMode::parse(mode.trim())?;

    let (work_dir, tool) = resolve_downloader(&app)?;
    if !tool.is_file() {
        return Err(format!(
            "No se encuentra el descargador en: {}.",
            tool.display()
        ));
    }

    let output_dir = effective_download_dir(&app)?;
    if !output_dir.is_dir() {
        return Err(format!(
            "La carpeta de descarga no es un directorio: {}.",
            output_dir.display()
        ));
    }

    let app_for = app.clone();
    let url_for_task = url.clone();
    let work_for_task = work_dir.clone();
    let output_for_task = output_dir.clone();
    let tool_for = tool.clone();
    let output = tauri::async_runtime::spawn_blocking(move || {
        run_downloader_with_events(
            &app_for,
            work_for_task,
            output_for_task,
            tool_for,
            url_for_task,
            mode,
        )
    })
    .await
    .map_err(|e| e.to_string())??;

    let code = output.status.code().unwrap_or(-1);
    let out = String::from_utf8_lossy(&output.stdout);
    let err = String::from_utf8_lossy(&output.stderr);
    let mut msg = String::new();
    if !out.trim().is_empty() {
        msg.push_str("— salida estándar —\n");
        msg.push_str(&out);
    }
    if !err.trim().is_empty() {
        if !msg.is_empty() {
            msg.push('\n');
        }
        msg.push_str("— registro del descargador —\n");
        msg.push_str(&err);
    }
    if msg.is_empty() {
        msg = "(sin salida)".to_string();
    }

    // yt-dlp 101 = detenido por `--max-downloads` (un solo ítem en listas).
    let err_lc = err.to_ascii_lowercase();
    let max_downloads_stop = code == 101
        || (err_lc.contains("max-downloads")
            && (err_lc.contains("maximum number of downloads")
                || err_lc.contains("stopping due to")));

    if output.status.success() || max_downloads_stop {
        let output_path = infer_output_path_from_downloader_log(&err, &out)
            .map(|p| display_path(&p));
        let log = format!("Listo (código de salida {code}).\n{msg}");
        Ok(DownloadYoutubeResponse { log, output_path })
    } else {
        let err_body = format!("La descarga falló (código de salida {code}).\n{msg}");
        let path_note = append_download_failure_log(&app, &err_body)
            .map(|p| format!("\n\nTambién guardado en:\n{}", p.display()))
            .unwrap_or_default();
        Err(format!("{err_body}{path_note}"))
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![
            download_youtube,
            get_download_folder,
            pick_download_folder,
            bootstrap_ui,
            acknowledge_first_run_tip,
            open_downloads_folder,
            open_downloaded_file,
            reveal_downloaded_file
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
