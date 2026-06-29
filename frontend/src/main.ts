import { createApp } from 'vue'
import { createPinia } from 'pinia'
import App from './App.vue'
import { createAppRouter } from './router'
import './style.css'

createApp(App).use(createPinia()).use(createAppRouter()).mount('#app')
