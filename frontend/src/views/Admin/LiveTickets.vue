<template>
  <div class="wrap">
    <h2>Tickets</h2>
    <p class="muted">
      Les tickets ouverts depuis la conversation, avec le résumé de l'échange et les sources consultées. Ils sont gardés dans l'application ;
      aucun outil de suivi (GLPI, Teams…) n'y est relié pour l'instant : copiez un ticket pour le transmettre.
    </p>
    <div class="row">
      <label class="inline">Afficher
        <select v-model="statusFilter" aria-label="Filtrer par statut">
          <option value="">tous les statuts</option>
          <option v-for="s in TICKET_STATUSES" :key="s" :value="s">{{ s }}</option>
        </select>
      </label>
      <button type="button" @click="load">Actualiser</button>
      <span class="muted small">{{ shown.length }} ticket{{ shown.length > 1 ? 's' : '' }}</span>
    </div>
    <p v-if="problem" class="err" role="alert">{{ problem }}</p>
    <p v-if="!tickets.length && !problem" class="muted">Aucun ticket pour l'instant.</p>

    <ul class="plain">
      <li v-for="t in shown" :key="t.id" class="card ticket">
        <div class="row">
          <b class="ref">{{ t.reference }}</b>
          <span class="grow title">{{ t.title }}</span>
          <select :value="t.status" :aria-label="`Statut de ${t.reference}`" @change="setStatus(t, $event.target.value)">
            <option v-for="s in TICKET_STATUSES" :key="s" :value="s">{{ s }}</option>
          </select>
        </div>
        <p class="muted small">
          {{ date(t.created) }}<template v-if="t.user_name"> · {{ t.user_name }}</template><template v-if="t.conversation"> · profil « {{ t.conversation }} »</template>
        </p>
        <details>
          <summary>Voir le ticket</summary>
          <pre class="text">{{ t.description }}</pre>
          <p v-if="t.sources.length" class="small">
            Sources consultées :
            <template v-for="(s, i) in t.sources" :key="s.id">
              <button type="button" class="link" @click="viewing = s.id">{{ sourcePath(s) }}</button><span v-if="i < t.sources.length - 1"> · </span>
            </template>
          </p>
          <button type="button" @click="copy(t)">{{ copied === t.id ? 'Copié ✓' : 'Copier le texte' }}</button>
        </details>
      </li>
    </ul>

    <PassageViewer v-if="viewing" :id="viewing" @close="viewing = 0" />
  </div>
</template>

<script setup>
import { ref, computed, onMounted, onBeforeUnmount } from 'vue'
import axios from 'axios'
import PassageViewer from '../../components/PassageViewer.vue'
import { TICKET_STATUSES, adminHeaders, errorText, sourcePath, ticketText } from '../../utils/kb.js'

const tickets = ref([])
const statusFilter = ref('')
const problem = ref('')
const viewing = ref(0)
const copied = ref(0)
let timer = null
let copiedTimer = null

const shown = computed(() => (statusFilter.value ? tickets.value.filter((t) => t.status === statusFilter.value) : tickets.value))
const date = (t) => new Date(t * 1000).toLocaleString('fr-FR', { dateStyle: 'short', timeStyle: 'short' })

async function load() {
  try {
    tickets.value = (await axios.get('/api/live/tickets', { headers: adminHeaders() })).data
    problem.value = ''
  } catch (e) {
    problem.value = e.response?.status === 401 ? 'Connexion expirée : reconnectez-vous (menu Administration).' : e.response ? errorText(e) : 'Moteur live injoignable (live-engine démarré ?)'
  }
}

async function setStatus(t, status) {
  const before = t.status
  t.status = status
  try {
    const r = await axios.patch(`/api/live/tickets/${t.id}`, { status }, { headers: adminHeaders() })
    t.status = r.data.status
  } catch (e) {
    t.status = before
    problem.value = errorText(e)
  }
}

async function copy(t) {
  try {
    await navigator.clipboard.writeText(ticketText(t))
    copied.value = t.id
    clearTimeout(copiedTimer)
    copiedTimer = setTimeout(() => { copied.value = 0 }, 2000)
  } catch { problem.value = "Le navigateur a refusé l'accès au presse-papiers : sélectionnez le texte et copiez-le." }
}

onMounted(() => { load(); timer = setInterval(load, 15000) })
onBeforeUnmount(() => { clearInterval(timer); clearTimeout(copiedTimer) })
</script>

<style scoped src="../../assets/styles/Kb.css"></style>
<style scoped>
.wrap { padding: 1.5rem 2rem; overflow: auto; max-width: 1100px; display: flex; flex-direction: column; gap: 1rem; }
h2 { margin: 0; }
.ticket { gap: .3rem; }
.ticket p { margin: 0; }
.ref { color: #9db4ff; white-space: nowrap; }
.title { overflow-wrap: anywhere; }
@media (max-width: 800px) { .wrap { padding: 1rem; } }
</style>
