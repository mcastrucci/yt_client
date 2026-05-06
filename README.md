# Descargador YouTube

Aplicación de escritorio (**Tauri 2**) para descargar audio (MP3) o vídeo desde YouTube. Interfaz en español.

## Requisitos para desarrollo

- [Rust](https://rustup.rs/) y [Node.js](https://nodejs.org/)
- En `dependencies/mac/` y `dependencies/windows/` hay **README** con enlaces: copia ahí **yt-dlp** y, si quieres MP3/vídeo con calidad completa, **ffmpeg** y **ffprobe** (no van en el repo por tamaño).

## Ejecutar en desarrollo

```bash
cd app && npm install && npm run dev
```

## Compilar instalador

```bash
cd app && npm run build
```

## CI (GitHub Actions)

El workflow [.github/workflows/build-desktop.yml](.github/workflows/build-desktop.yml) descarga en cada ejecución:

| Pieza | Origen |
|--------|--------|
| **yt-dlp** | [yt-dlp/yt-dlp releases](https://github.com/yt-dlp/yt-dlp/releases) → `yt-dlp.exe` / `yt-dlp_macos` |
| **youtube-dl** (solo Windows, opcional) | [ytdl-org/youtube-dl releases](https://github.com/ytdl-org/youtube-dl/releases) → `youtube-dl.exe` |
| **FFmpeg Windows** | Tu zip [BtbN FFmpeg-Builds](https://github.com/BtbN/FFmpeg-Builds/releases) `win64-gpl-shared` → `bin/` completo (exe + dll) |
| **FFmpeg macOS** | `brew install ffmpeg` en el runner |

Se dispara con **workflow_dispatch** (botón *Run workflow* en GitHub) o al pushear un tag **`v*`** (ej. `v0.1.1`). Los instaladores salen como **artifacts** del job.

## Legal

Úsalo solo con contenido que tengas derecho a descargar. Este proyecto no está afiliado a YouTube.

## Autor

[Maximiliano Castrucci (@mcastrucci)](https://github.com/mcastrucci)
