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
        clipboardPanel: resolve(__dirname, 'src/clipboard-panel.html')
      },
      output: {
        manualChunks: {
          vue: ['vue', 'pinia']
        }
      }
    }
  },
  server: {
    // 显式绑定 IPv4 回环地址：默认 'localhost' 在不同环境下可能解析到
    // IPv6 [::1] 或 IPv4 127.0.0.1，导致壳与页面按另一栈访问时连接被拒
    host: '127.0.0.1',
    port: 8090,
    strictPort: true  // 如果端口被占用则报错，而不是使用其他端口
  }
})
