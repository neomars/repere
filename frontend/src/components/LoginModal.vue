
<template>
  <div
    v-if="isOpen"
    class="modal-overlay"
    @click.self="close"
    role="dialog"
    aria-modal="true"
    aria-labelledby="modal-title"
  >
    <div class="modal-content">
      <div class="modal-header">
        <h2 id="modal-title">Connexion administrateur</h2>
        <button class="close-btn" @click="close" aria-label="Fermer" title="Fermer">
          <span aria-hidden="true">&times;</span>
        </button>
      </div>
      <form @submit.prevent="handleLogin" class="login-form">
        <div class="form-group">
          <label for="username">Nom d'utilisateur</label>
          <input
            ref="usernameInput"
            type="text"
            id="username"
            v-model="username"
            required
            aria-required="true"
            placeholder="admin"
            autocomplete="username"
            :disabled="isLoading"
          >
        </div>
        <div class="form-group">
          <label for="password">Mot de passe</label>
          <div class="password-wrapper">
            <input
              :type="showPassword ? 'text' : 'password'"
              id="password"
              v-model="password"
              required
              aria-required="true"
              autocomplete="current-password"
              :disabled="isLoading"
            >
            <button
              type="button"
              class="password-toggle"
              @click="showPassword = !showPassword"
              :aria-label="showPassword ? 'Masquer le mot de passe' : 'Afficher le mot de passe'"
              :title="showPassword ? 'Masquer le mot de passe' : 'Afficher le mot de passe'"
            >
              {{ showPassword ? '🙈' : '👁️' }}
            </button>
          </div>
        </div>
        <div v-if="errorMessage" class="error-message" role="alert">
          {{ errorMessage }}
        </div>
        <p class="login-hint">Identifiants par défaut : <code>admin</code> / <code>admin</code></p>
        <button type="submit" class="submit-btn" :disabled="isLoading">
          {{ isLoading ? 'Connexion…' : 'Se connecter' }}
        </button>
      </form>
    </div>
  </div>
</template>

<script setup>
import { ref, watch, onMounted, onUnmounted, nextTick } from 'vue';
import axios from 'axios';

const props = defineProps({
  isOpen: Boolean
});

const emit = defineEmits(['close', 'login-success']);

const username = ref('');
const password = ref('');
const showPassword = ref(false);
const isLoading = ref(false);
const errorMessage = ref('');
const usernameInput = ref(null);
const lastActiveElement = ref(null);

const close = () => {
  if (isLoading.value) return;
  errorMessage.value = '';
  username.value = '';
  password.value = '';
  emit('close');
};

const handleKeydown = (e) => {
  if (props.isOpen && e.key === 'Escape') {
    close();
  }
};

watch(() => props.isOpen, (newVal) => {
  if (newVal) {
    lastActiveElement.value = document.activeElement;
    nextTick(() => {
      usernameInput.value?.focus();
    });
  } else {
    if (lastActiveElement.value) {
      lastActiveElement.value.focus();
    }
  }
});

onMounted(() => {
  window.addEventListener('keydown', handleKeydown);
});

onUnmounted(() => {
  window.removeEventListener('keydown', handleKeydown);
});

const handleLogin = async () => {
  isLoading.value = true;
  errorMessage.value = '';
  try {
    const response = await axios.post('/api/auth/login', {
      username: username.value,
      password: password.value
    });
    emit('login-success', response.data);
    close();
  } catch (err) {
    errorMessage.value = err.response?.data?.message || 'Une erreur est survenue pendant la connexion.';
  } finally {
    isLoading.value = false;
  }
};
</script>

<style scoped src="../assets/styles/LoginModal.css"></style>
