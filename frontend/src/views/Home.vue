<template>
  <div class="home-container">
    <h2>Repère</h2>
    <p class="lead">Votre assistant de support informatique : posez votre question, il cherche la réponse dans la base de connaissances de la DSI.</p>

    <div class="cards">
      <router-link to="/live" class="card">
        <span class="card-title">💬 Assistant</span>
        <span>Poser une question à l’assistant de support</span>
      </router-link>
    </div>

    <p class="admin-links">
      Administration :
      <router-link to="/admin/users">utilisateurs</router-link> ·
      <router-link to="/admin/live/knowledge">base de connaissances</router-link> ·
      <router-link to="/admin/live/tickets">tickets</router-link> ·
      <router-link to="/admin/live/models">modèles</router-link> ·
      <router-link to="/admin/live/personas">profils d’assistant</router-link>
    </p>
  </div>
</template>

<script setup>
import { RouterLink } from 'vue-router'
</script>

<style scoped src="../assets/styles/Home.css"></style>
