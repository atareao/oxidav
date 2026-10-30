import react from '@vitejs/plugin-react'
import { defineConfig } from 'vitest/config'

export default defineConfig({
  base: './',
  plugins: [react()],
  build: {
    outDir: 'dist',
    rollupOptions: {
      output: {
        codeSplitting: {
          groups: [
            // Keep the React runtime in its own long-lived chunk: it changes
            // rarely and is shared by every route, so isolating it maximizes
            // cache reuse across deploys.
            {
              name: 'react-vendor',
              test: /node_modules[\\/](react|react-dom|react-router|react-router-dom|scheduler)[\\/]/,
            },
          ],
        },
      },
    },
  },
  server: {
    proxy: {
      '/api': 'http://localhost:8080',
      '/dav': 'http://localhost:8080',
    },
  },
  test: {
    environment: 'jsdom',
    setupFiles: ['./src/test/setup.ts'],
    globals: true,
    // Los tests de integración con antd (p. ej. el diálogo de eventos de F26)
    // encadenan muchas interacciones de usuario y en un runner de CI lento
    // rozan —y superan— el límite por defecto de 5000 ms. Se sube el margen
    // para absorber esa lentitud sin enmascarar cuelgues reales: un test que se
    // cuelga de verdad (sin aserciones que resuelvan) sigue fallando.
    testTimeout: 15_000,
  },
})
