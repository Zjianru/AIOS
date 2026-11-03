import { fileURLToPath, URL } from 'node:url';
import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';
import tailwindcss from '@tailwindcss/vite';

// T007.2 开发期代理：aios-dev serve 的 /api、/ws 代理到这个地址（默认 4097，
// 起服务时用 --port 4097；构建产物由 serve 同源托管，开发时页面走 Vite）。
const serviceOrigin = process.env.AIOS_SERVE_ORIGIN ?? 'http://127.0.0.1:4097';

export default defineConfig({
  plugins: [react(), tailwindcss()],
  resolve: {
    alias: { '@': fileURLToPath(new URL('./src', import.meta.url)) },
  },
  build: { target: 'es2022', sourcemap: false },
  // 测速要量冷启动到首屏，禁用 dev 的额外包装只影响 dev；build 是纯静态页
  server: {
    port: 5184,
    strictPort: true,
    proxy: {
      '/api': serviceOrigin,
      '/ws': { target: serviceOrigin.replace('http', 'ws'), ws: true },
    },
  },
});
