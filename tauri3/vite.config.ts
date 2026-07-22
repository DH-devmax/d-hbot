import { fileURLToPath, URL } from 'node:url'
import { defineConfig, loadEnv } from 'vite'
import react from '@vitejs/plugin-react'

export default defineConfig(({ mode }) => {
  const env = loadEnv(mode, process.cwd(), '')
  const developer = env.VITE_DH_FIXTURE === '1' || mode === 'style'
  return {
    plugins: [react()],
    resolve: {
      alias: {
        '@runtime-controls': fileURLToPath(new URL(
          developer ? './src/RuntimeControls.developer.tsx' : './src/RuntimeControls.production.tsx',
          import.meta.url,
        )),
      },
    },
    clearScreen: false,
    server: {
      host: '127.0.0.1',
      strictPort: true,
    },
  }
})
