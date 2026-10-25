import { createApp } from 'vue'
import axios from 'axios'
import App from './App.vue'
import router from './router'

// Configure axios globally
axios.defaults.withCredentials = true

const app = createApp(App)

// Disable Vue Devtools in production
if (import.meta.env.PROD) {
  app.config.devtools = false;
}

app.use(router)

app.mount('#app')
