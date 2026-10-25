
<template>
  <div class="user-management">
    <div class="header-container">
      <h1>Gestion des utilisateurs</h1>
    </div>

    <!-- Changement de mot de passe -->
    <section class="admin-section">
      <h2>Changer mon mot de passe</h2>
      <form @submit.prevent="changePassword" class="settings-form">
        <div class="form-group">
          <label for="old-password">Ancien mot de passe</label>
          <input type="password" id="old-password" v-model="passChange.oldPassword" required />
        </div>
        <div class="form-group">
          <label for="new-password">Nouveau mot de passe</label>
          <input type="password" id="new-password" v-model="passChange.newPassword" required />
        </div>
        <button type="submit" class="button" :disabled="isChangingPass">
          {{ isChangingPass ? 'Modification…' : 'Modifier le mot de passe' }}
        </button>
      </form>
    </section>

    <hr class="separator" />

    <!-- Liste des utilisateurs et ajout -->
    <section class="admin-section">
      <h2>Utilisateurs</h2>
      <form @submit.prevent="createUser" class="add-user-form">
        <label for="new-username" class="sr-only">Nom d'utilisateur</label>
        <input type="text" id="new-username" v-model="newUser.username" placeholder="Nom d'utilisateur" required />
        <label for="new-password-field" class="sr-only">Mot de passe</label>
        <input type="password" id="new-password-field" v-model="newUser.password" placeholder="Mot de passe" required />
        <button type="submit" class="button" :disabled="isCreatingUser">
          {{ isCreatingUser ? 'Ajout…' : 'Ajouter' }}
        </button>
      </form>

      <ul class="user-list">
        <li v-for="user in users" :key="user.id">
          <div class="user-info">
            <strong>{{ user.username }}</strong>
            <span v-if="user.id === auth.currentUser.value?.id" class="self-badge">Moi</span>
          </div>
          <div class="user-actions">
            <button
              @click="deleteUser(user)"
              class="button-delete"
              :disabled="user.id === auth.currentUser.value?.id || deletingUserId === user.id"
              :title="deletingUserId === user.id ? 'Suppression…' : (user.id === auth.currentUser.value?.id ? 'Vous ne pouvez pas vous supprimer vous-même' : 'Supprimer l\'utilisateur : ' + user.username)"
              :aria-label="deletingUserId === user.id ? 'Suppression de l\'utilisateur : ' + user.username : (user.id === auth.currentUser.value?.id ? 'Vous ne pouvez pas vous supprimer vous-même' : 'Supprimer l\'utilisateur : ' + user.username)"
              >
                <span v-if="deletingUserId === user.id" aria-hidden="true">...</span>
                <span v-else aria-hidden="true">&times;</span>
              </button>
          </div>
        </li>
      </ul>
    </section>
  </div>
</template>

<script setup>
import { ref, onMounted, inject } from 'vue';
import axios from 'axios';

const auth = inject('auth');

const users = ref([]);
const passChange = ref({ oldPassword: '', newPassword: '' });
const newUser = ref({ username: '', password: '' });

const isChangingPass = ref(false);
const isCreatingUser = ref(false);
const deletingUserId = ref(null);

const fetchUsers = async () => {
  try {
    const res = await axios.get('/api/admin/users');
    users.value = res.data;
  } catch (err) {
    console.error('Impossible de charger les utilisateurs');
  }
};

const changePassword = async () => {
  isChangingPass.value = true;
  try {
    await axios.post('/api/admin/change-password', passChange.value);
    alert('Mot de passe modifié.');
    passChange.value = { oldPassword: '', newPassword: '' };
  } catch (err) {
    alert(err.response?.data?.message || err.message || 'Échec du changement de mot de passe.');
  } finally {
    isChangingPass.value = false;
  }
};

const createUser = async () => {
  isCreatingUser.value = true;
  try {
    await axios.post('/api/admin/users', newUser.value);
    newUser.value = { username: '', password: '' };
    fetchUsers();
  } catch (err) {
    alert(err.response?.data?.message || err.message || 'Échec de la création de l\'utilisateur.');
  } finally {
    isCreatingUser.value = false;
  }
};

const deleteUser = async (user) => {
  if (confirm(`Supprimer l'utilisateur « ${user.username} » ?`)) {
    deletingUserId.value = user.id;
    try {
      await axios.delete(`/api/admin/users/${user.id}`);
      fetchUsers();
    } catch (err) {
      alert(err.response?.data?.message || err.message || 'Échec de la suppression.');
    } finally {
      deletingUserId.value = null;
    }
  }
};

onMounted(fetchUsers);
</script>

<style scoped src="../../assets/styles/UserManagement.css"></style>
