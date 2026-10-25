<template>
  <div id="app">
    <a href="#main-content" class="skip-link">Aller au contenu principal</a>
    <header>
      <div class="header-left">
        <router-link to="/" class="header-title">
          <h1>Repère</h1>
        </router-link>
      </div>
      <nav aria-label="Navigation principale">
        <router-link to="/live">Assistant</router-link>
        <router-link to="/admin/users" @click="handleAdminClick">Administration</router-link>
        <button v-if="isAuthenticated" @click="handleLogout" class="logout-link">Déconnexion ({{ currentUser?.username }})</button>
      </nav>
    </header>
    <!-- Menu d'administration : utilisateurs et réglages de l'assistant (modèles, profils) -->
    <nav v-if="inAdmin" class="admin-nav" aria-label="Menu d'administration">
      <router-link to="/admin/users">Utilisateurs</router-link>
      <router-link to="/admin/live/models">⚙ Modèles IA</router-link>
      <router-link to="/admin/live/personas">Profils d'assistant</router-link>
    </nav>
    <main id="main-content" tabindex="-1">
      <router-view/>
    </main>
    <LoginModal
      :is-open="isLoginModalOpen"
      @close="isLoginModalOpen = false"
      @login-success="onLoginSuccess"
    />
  </div>
</template>

<script setup>
import { ref, computed, onMounted, provide } from 'vue';
import { RouterLink, RouterView, useRouter, useRoute } from 'vue-router'
import axios from 'axios';
import LoginModal from './components/LoginModal.vue';

const router = useRouter();
const route = useRoute();
const inAdmin = computed(() => route.path.startsWith('/admin'));
const isAuthenticated = ref(false);
const currentUser = ref(null);
const isLoginModalOpen = ref(false);

// Provide auth state to child components
provide('auth', {
  isAuthenticated,
  currentUser,
  checkAuth
});

async function checkAuth() {
  try {
    const response = await axios.get('/api/auth/me');
    isAuthenticated.value = true;
    currentUser.value = response.data;
    return true;
  } catch (err) {
    isAuthenticated.value = false;
    currentUser.value = null;
    return false;
  }
}

const handleAdminClick = (e) => {
  if (!isAuthenticated.value) {
    e.preventDefault();
    isLoginModalOpen.value = true;
  }
};

const onLoginSuccess = (user) => {
  isAuthenticated.value = true;
  currentUser.value = user;
  isLoginModalOpen.value = false;
  router.push('/admin/users');
};

const handleLogout = async () => {
  try {
    await axios.post('/api/auth/logout');
    isAuthenticated.value = false;
    currentUser.value = null;
    if (router.currentRoute.value.path.startsWith('/admin')) {
      router.push('/');
    }
  } catch (err) {
    console.error('Échec de la déconnexion :', err);
  }
};

onMounted(() => {
  checkAuth();
});
</script>

<style src="./assets/styles/App.css"></style>
