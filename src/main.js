import { createApp } from 'vue'
import { createPinia } from 'pinia'
import App from './App.vue'
import './styles/main.css'  // 直接使用原样式文件

const app = createApp(App)
const pinia = createPinia()

app.use(pinia)
app.mount('#app')
