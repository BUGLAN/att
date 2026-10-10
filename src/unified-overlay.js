// 设计令牌必须先于任何组件/窗口样式引入
import './styles/tokens.css'
import { createApp } from 'vue'
import UnifiedOverlayWindow from './components/overlay/UnifiedOverlayWindow.vue'
import './styles/unified-overlay.css'

createApp(UnifiedOverlayWindow).mount('#app')
