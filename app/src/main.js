const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

const FIRST_RUN_TIP = `Bienvenido/a — unas indicaciones rápidas:

• Pega un enlace de YouTube (vídeo, Shorts, Música o una lista), elige Audio o Vídeo y pulsa Descargar.
• Los archivos se guardan en la carpeta que ves en Salida. Puedes abrirla cuando quieras con «Abrir carpeta».
• Solo se aceptan enlaces reales de YouTube (youtube.com, youtu.be, etc.).
• Si falla una descarga, el mismo mensaje se guarda también en un archivo pequeño en la carpeta de registros de la app (útil si necesitas ayuda).

Listo/a: pega un enlace cuando quieras empezar.`;

window.addEventListener("DOMContentLoaded", () => {
  const form = document.querySelector("#dl-form");
  const input = document.querySelector("#url-input");
  const log = document.querySelector("#log");
  const submitBtn = document.querySelector("#submit-btn");
  const formatInputs = form.querySelectorAll('input[name="format"]');
  const progressWrap = document.querySelector("#progress-wrap");
  const progressBar = document.querySelector("#progress-bar");
  const progressLabel = document.querySelector("#progress-label");
  const folderPathEl = document.querySelector("#folder-path");
  const pickFolderBtn = document.querySelector("#pick-folder-btn");
  const openFolderBtn = document.querySelector("#open-folder-btn");
  const copyLogBtn = document.querySelector("#copy-log-btn");
  const postDownloadWrap = document.querySelector("#post-download-wrap");
  const openFileBtn = document.querySelector("#open-file-btn");
  const revealFileBtn = document.querySelector("#reveal-file-btn");

  let lastOutputPath = null;

  function setPostDownload(pathOrNull) {
    lastOutputPath = pathOrNull;
    if (pathOrNull) {
      postDownloadWrap.hidden = false;
    } else {
      postDownloadWrap.hidden = true;
    }
  }

  async function loadBootstrap() {
    try {
      const data = await invoke("bootstrap_ui");
      folderPathEl.textContent = data.downloadFolder;
      folderPathEl.title = data.downloadFolder;
      if (data.showFirstRunTip) {
        log.textContent = FIRST_RUN_TIP;
        await invoke("acknowledge_first_run_tip");
      }
    } catch (err) {
      folderPathEl.textContent =
        typeof err === "string" ? err : String(err);
      folderPathEl.title = "";
    }
  }

  pickFolderBtn.addEventListener("click", async () => {
    pickFolderBtn.disabled = true;
    try {
      const chosen = await invoke("pick_download_folder");
      if (chosen) {
        folderPathEl.textContent = chosen;
        folderPathEl.title = chosen;
      }
    } catch (err) {
      log.textContent = typeof err === "string" ? err : String(err);
    } finally {
      pickFolderBtn.disabled = false;
    }
  });

  openFolderBtn.addEventListener("click", async () => {
    openFolderBtn.disabled = true;
    try {
      await invoke("open_downloads_folder");
    } catch (err) {
      log.textContent = typeof err === "string" ? err : String(err);
    } finally {
      openFolderBtn.disabled = false;
    }
  });

  openFileBtn.addEventListener("click", async () => {
    if (!lastOutputPath) return;
    openFileBtn.disabled = true;
    try {
      await invoke("open_downloaded_file", { path: lastOutputPath });
    } catch (err) {
      log.textContent = typeof err === "string" ? err : String(err);
    } finally {
      openFileBtn.disabled = false;
    }
  });

  revealFileBtn.addEventListener("click", async () => {
    if (!lastOutputPath) return;
    revealFileBtn.disabled = true;
    try {
      await invoke("reveal_downloaded_file", { path: lastOutputPath });
    } catch (err) {
      log.textContent = typeof err === "string" ? err : String(err);
    } finally {
      revealFileBtn.disabled = false;
    }
  });

  copyLogBtn.addEventListener("click", async () => {
    const text = log.textContent || "";
    const prevLabel = copyLogBtn.textContent;
    try {
      await navigator.clipboard.writeText(text);
      copyLogBtn.textContent = "¡Copiado!";
      setTimeout(() => {
        copyLogBtn.textContent = prevLabel;
      }, 1800);
    } catch {
      copyLogBtn.textContent = "Selecciona y copia";
      setTimeout(() => {
        copyLogBtn.textContent = prevLabel;
      }, 2200);
    }
  });

  loadBootstrap();

  function selectedMode() {
    for (const el of formatInputs) {
      if (el.checked) return el.value;
    }
    return "audio";
  }

  function resetProgress() {
    progressBar.removeAttribute("value");
    progressBar.classList.add("indeterminate");
    progressLabel.textContent = "Iniciando…";
  }

  function setProgress(percent) {
    progressBar.classList.remove("indeterminate");
    if (typeof percent === "number" && !Number.isNaN(percent)) {
      const p = Math.min(100, Math.max(0, percent));
      progressBar.value = p;
      progressLabel.textContent = `${Math.round(p)}%`;
    }
  }

  form.addEventListener("submit", async (e) => {
    e.preventDefault();
    const url = input.value.trim();
    const mode = selectedMode();
    log.textContent = "";
    setPostDownload(null);
    progressWrap.hidden = false;
    resetProgress();
    submitBtn.disabled = true;

    let unlisten = null;
    try {
      if (typeof listen === "function") {
        unlisten = await listen("download-progress", (e) => {
          const payload = e?.payload ?? e;
          let p = payload?.percent;
          if (typeof p === "string") {
            const n = parseFloat(p);
            p = Number.isFinite(n) ? n : null;
          }
          if (typeof p === "number" && !Number.isNaN(p)) {
            setProgress(p);
          } else {
            const line = payload?.line || "";
            if (line) {
              const short = line.length > 70 ? line.slice(0, 70) + "…" : line;
              progressLabel.textContent = short;
            }
          }
        });
      }
      const result = await invoke("download_youtube", { url, mode });
      log.textContent = result.log;
      const outPath = result.outputPath ?? result.output_path ?? null;
      setPostDownload(outPath || null);
      progressBar.classList.remove("indeterminate");
      progressBar.value = 100;
      progressLabel.textContent = "Hecho";
    } catch (err) {
      log.textContent = typeof err === "string" ? err : String(err);
      setPostDownload(null);
      progressLabel.textContent = "Error";
    } finally {
      if (unlisten) {
        unlisten();
      }
      submitBtn.disabled = false;
      setTimeout(() => {
        progressWrap.hidden = true;
      }, 400);
    }
  });
});
