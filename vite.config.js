import { defineConfig } from 'vite'
import vue from '@vitejs/plugin-vue'
import { resolve } from 'path'

export default defineConfig({
  plugins: [vue()],
  base: './',
  resolve: {
    alias: {
      '@': resolve(__dirname, 'src')
    }
  },
  root: 'src',
  publicDir: '../public',
  build: {
    outDir: '../dist',
    emptyOutDir: true,
    assetsDir: 'assets',
    rollupOptions: {
      input: {
        main: resolve(__dirname, 'src/index.html'),
        floating: resolve(__dirname, 'src/floating.html'),
        clipboardOverlay: resolve(__dirname, 'src/clipboard-overlay.html'),
        selectionToolbar: resolve(__dirname, 'src/selection-toolbar.html')
      },
      output: {
        manualChunks: {
          vue: ['vue', 'pinia']
        }
      }
    }
  },
  server: {
    port: 5173,
    strictPort: true  // 如果端口被占用则报错，而不是使用其他端口
  }
})
