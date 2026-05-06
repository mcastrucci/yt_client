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

## Legal

Úsalo solo con contenido que tengas derecho a descargar. Este proyecto no está afiliado a YouTube.

## Autor

[Maximiliano Castrucci (@mcastrucci)](https://github.com/mcastrucci)
