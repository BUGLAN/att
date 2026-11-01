// 设计令牌必须先于任何组件/窗口样式引入，三个渲染入口保持一致顺序
import './styles/tokens.css'
import { createApp } from 'vue'
import { createPinia } from 'pinia'
import App from './App.vue'
import './styles/main.css'
import './styles/page-atoms.css'
import './styles/desktop-overrides.css'

const app = createApp(App)
const pinia = createPinia()

app.use(pinia)
app.mount('#app')
