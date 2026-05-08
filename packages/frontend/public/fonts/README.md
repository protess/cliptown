# IBM Plex fonts

Phase 0 self-hosted IBM Plex. Download these woff2 files from
https://github.com/IBM/plex/releases and place them in this directory:

- IBMPlexSans-Regular.woff2
- IBMPlexSans-Medium.woff2
- IBMPlexSans-Bold.woff2
- IBMPlexMono-Regular.woff2
- IBMPlexMono-Medium.woff2

Until the files are present, the page falls back to system-ui sans/mono
(via the @font-face fallbacks in `src/styles/fonts.css`).
