// 设计令牌必须先于任何组件/窗口样式引入
import './styles/tokens.css'
import { createApp } from 'vue'
import FloatingWindow from './components/floating/FloatingWindow.vue'
import './styles/floating.css'

createApp(FloatingWindow).mount('#app')
