<template>
  <section class="card">
    <h3>Dictionnaire de synonymes</h3>
    <p class="muted small">
      Rapproche le jargon de vos utilisateurs de ce qu'écrivent vos documents : « mdp » et « mot de passe », « visio » et « Teams ». Une ligne = un concept,
      des mots de <b>même sens</b> séparés par des virgules ; après un « ~ », des mots seulement <b>liés</b> (ils classent mieux les passages mais ne déclenchent rien).
      Accents, majuscules et pluriels n'ont pas d'importance. Évitez les mots courants de la conversation (« problème », « demande »…).
    </p>
    <template v-if="info">
      <p class="small">{{ info.concepts }} concepts en vigueur (dictionnaire de base + le vôtre).</p>
      <label>Vos synonymes
        <textarea v-model="text" rows="8" spellcheck="false" :disabled="!info.editable" placeholder="glpi, ticket glpi, outil de tickets ~ incident, demande&#10;citrix, bureau virtuel, workspace" />
      </label>
      <p v-if="!info.editable" class="warn small">Cette installation n'a pas de dossier de données : le dictionnaire ne peut pas être modifié.</p>
      <div class="row">
        <button type="button" class="primary" :disabled="!info.editable || saving || !changed" @click="save">{{ saving ? 'Enregistrement…' : 'Enregistrer' }}</button>
        <span v-if="saved" class="ok small" role="status">Enregistré : pris en compte tout de suite.</span>
      </div>
      <ul v-if="problems.length" class="plain" role="alert">
        <li v-for="p in problems" :key="p" class="warn small">⚠ {{ p }}</li>
      </ul>
      <details>
        <summary>Voir le dictionnaire de base (lecture seule)</summary>
        <pre class="text">{{ info.builtin }}</pre>
      </details>
    </template>
    <p v-if="problem" class="err" role="alert">{{ problem }}</p>
  </section>
</template>

<script setup>
import { ref, computed, onMounted } from 'vue'
import axios from 'axios'
import { adminHeaders, errorText } from '../../utils/kb.js'

const info = ref(null)
const text = ref('')
const original = ref('')
const problems = ref([])
const problem = ref('')
const saving = ref(false)
const saved = ref(false)

const changed = computed(() => text.value !== original.value)

async function load() {
  try {
    info.value = (await axios.get('/api/live/kb/synonyms')).data
    text.value = original.value = info.value.custom
    problems.value = info.value.problems || []
  } catch (e) {
    problem.value = errorText(e)
  }
}

async function save() {
  saving.value = true
  saved.value = false
  problem.value = ''
  try {
    const r = await axios.put('/api/live/kb/synonyms', { text: text.value }, { headers: adminHeaders() })
    original.value = text.value
    info.value = { ...info.value, concepts: r.data.concepts }
    problems.value = r.data.problems || []
    saved.value = true
  } catch (e) {
    problem.value = e.response?.status === 401 ? 'Connexion expirée : reconnectez-vous (menu Administration).' : errorText(e)
  } finally {
    saving.value = false
  }
}

onMounted(load)
</script>

<style scoped src="../../assets/styles/Kb.css"></style>
