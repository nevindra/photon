import { createApp } from 'vue'
import '@fontsource-variable/inter'
import '@fontsource/jetbrains-mono/400.css'
import '@fontsource/jetbrains-mono/500.css'
import '@fontsource/jetbrains-mono/700.css'
import './styles/tokens.css'
import './styles/base.css'
import { VueQueryPlugin, QueryClient } from '@tanstack/vue-query'
import App from './App.vue'
import { router } from './router/index.js'
import { seedContextFromUrl, startContextUrlSync } from '@/lib/core/context'

const queryClient = new QueryClient({
  defaultOptions: { queries: { refetchOnWindowFocus: false, retry: 1 } },
})

// Time is global (Task 5): seed the app-wide time/scope context from the current URL, then
// keep it synced back to the URL as it changes — before the app mounts so the first render
// already reflects any range/from/to/scope query params.
seedContextFromUrl()
startContextUrlSync()

createApp(App).use(router).use(VueQueryPlugin, { queryClient }).mount('#app')
