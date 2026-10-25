
import { createRouter, createWebHistory } from 'vue-router'
import axios from 'axios'
import Home from '../views/Home.vue'

// Chaque écran est chargé à la demande : l'accueil n'embarque ni la conversation, ni l'administration.
const Live = () => import('../views/Live.vue')
const UserManagement = () => import('../views/Admin/UserManagement.vue')
const LivePersonas = () => import('../views/Admin/LivePersonas.vue')
const LiveModels = () => import('../views/Admin/LiveModels.vue')

const router = createRouter({
  history: createWebHistory(import.meta.env.BASE_URL),
  routes: [
    {
      path: '/',
      name: 'home',
      component: Home
    },
    {
      path: '/live',
      name: 'live',
      component: Live
    },
    {
      path: '/admin/live/models',
      name: 'admin-live-models',
      component: LiveModels
    },
    {
      path: '/admin/live/personas',
      name: 'admin-live-personas',
      component: LivePersonas
    },
    {
      path: '/admin/users',
      name: 'admin-users',
      component: UserManagement
    },
  ]
})

router.beforeEach(async (to, from, next) => {
  if (to.path.startsWith('/admin')) {
    try {
      await axios.get('/api/auth/me');
      next();
    } catch (err) {
      // Non connecté : retour à l'accueil. Le lien « Administration » de l'en-tête ouvre la fenêtre de connexion (App.vue) ;
      // ce garde sert surtout à l'accès direct par l'adresse.
      next('/');
    }
  } else {
    next();
  }
});

// Une nouvelle version de l'app change le nom des fichiers : un onglet resté ouvert ne retrouve plus l'écran demandé (404 sur son
// fichier). On recharge alors la page sur la destination pour récupérer la version courante — une seule fois, pas de boucle.
router.onError((err, to) => {
  const missing = /Failed to fetch dynamically imported module|Importing a module script failed|error loading dynamically imported module/i.test(err?.message || '')
  if (!missing) return
  try {
    if (sessionStorage.getItem('repere.reloaded') === to.fullPath) return
    sessionStorage.setItem('repere.reloaded', to.fullPath)
  } catch { /* stockage indisponible : on recharge quand même */ }
  window.location.assign(to.fullPath)
})

// Une fois l'accueil affiché, on charge la conversation pendant que l'utilisateur lit : le clic suivant est immédiat.
router.isReady().then(() => {
  const warm = () => { Live() }
  if ('requestIdleCallback' in window) window.requestIdleCallback(warm, { timeout: 4000 })
  else setTimeout(warm, 1500)
})

export default router
