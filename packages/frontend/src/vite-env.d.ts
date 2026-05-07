/// <reference types="vite/client" />

interface ImportMetaEnv {
  readonly VITE_WORLD_WS_URL?: string;
  readonly VITE_OPERATOR_TOKEN?: string;
}

interface ImportMeta {
  readonly env: ImportMetaEnv;
}
