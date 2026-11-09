import { defineConfig } from 'vitest/config';

// Dev: Vite on 8794 with /api proxied to the AgentFX API (the real service or web/dev/mock_api.py).
const API = process.env.AGENTFX_API ?? 'http://127.0.0.1:8790';

export default defineConfig({
  esbuild: { jsx: 'automatic', jsxImportSource: 'preact' },
  resolve: { alias: { react: 'preact/compat', 'react-dom': 'preact/compat' } },
  server: {
    host: '127.0.0.1',
    port: Number(process.env.PORT ?? 8794),
    strictPort: true,
    proxy: { '/api': { target: API, changeOrigin: true } },
    fs: { allow: ['..'] }, // the brand fonts come from ../assets/fonts
  },
  preview: { host: '127.0.0.1', port: Number(process.env.PORT ?? 8794), proxy: { '/api': { target: API, changeOrigin: true } } },
  build: { target: 'es2022', sourcemap: true },
  test: { include: ['test/**/*.test.ts'], testTimeout: 30000 },
});
