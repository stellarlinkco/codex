import { defineConfig } from 'vite'
import react from '@vitejs/plugin-react'
import { resolve } from 'node:path'

const base = process.env.VITE_BASE_URL || '/'
const backend = process.env.VITE_BACKEND_URL || 'http://127.0.0.1:3006'
const appVersion = process.env.VITE_APP_VERSION || 'dev'

export default defineConfig({
    define: {
        __APP_VERSION__: JSON.stringify(appVersion),
    },
    server: {
        host: true,
        proxy: {
            '/api': {
                target: backend,
                changeOrigin: true
            },
            '/ws': {
                target: backend,
                ws: true
            }
        }
    },
    plugins: [
        react(),
    ],
    base,
    resolve: {
        alias: {
            '@': resolve(__dirname, 'src')
        }
    },
    build: {
        outDir: 'dist',
        emptyOutDir: true
    }
})
