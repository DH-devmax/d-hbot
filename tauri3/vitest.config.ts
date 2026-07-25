import { fileURLToPath, URL } from 'node:url'
import { defineConfig } from 'vitest/config'
import react from '@vitejs/plugin-react'

export default defineConfig({
  plugins: [react()],
  resolve: {
    alias: {
      '@runtime-controls': fileURLToPath(new URL('./src/RuntimeControls.developer.tsx', import.meta.url)),
      '@calibration-controls': fileURLToPath(new URL('./src/CalibrationControls.developer.tsx', import.meta.url)),
    },
  },
  test: {
    environment: 'jsdom',
    setupFiles: ['./src/test/setup.ts'],
    include: ['src/**/*.test.{ts,tsx}'],
    exclude: ['e2e/**'],
    restoreMocks: true,
    clearMocks: true,
  },
})
