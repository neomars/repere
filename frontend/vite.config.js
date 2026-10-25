import { fileURLToPath, URL } from 'node:url'

import { defineConfig } from 'vite'
import vue from '@vitejs/plugin-vue'
import vueDevTools from 'vite-plugin-vue-devtools'

// https://vite.dev/config/
export default defineConfig({
  plugins: [
    vue(),
    vueDevTools(),
  ],
  resolve: {
    alias: {
      '@': fileURLToPath(new URL('./src', import.meta.url))
    }
  },
  server: {
    proxy: {
      // Moteur live (Rust) : doit précéder la règle générique /api
      '/api/live': {
        target: 'http://127.0.0.1:3001',
        changeOrigin: true,
        ws: true,
      },

      // Une seule règle pour toutes les routes de l'API : le serveur attend déjà le préfixe /api, aucune réécriture.
      '/api': {
        target: 'http://127.0.0.1:3000',
        changeOrigin: true,
      },

      // Médias de la base de connaissances
      '/videos': {
        target: 'http://127.0.0.1:3000',
        changeOrigin: true,
      },
      '/thumbnails': {
        target: 'http://127.0.0.1:3000',
        changeOrigin: true,
      },
      '/photos': {
        target: 'http://127.0.0.1:3001',
        changeOrigin: true,
      }
    }
  }
})
