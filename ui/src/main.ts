import { createApp } from 'vue'
import 'vue-sonner/style.css'
import { Toaster } from 'vue-sonner'
import App from './App.vue'
import router from './router'

const app = createApp(App)
app.use(router)
app.mount('#app')
