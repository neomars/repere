<template>
  <section class="card">
    <div class="row">
      <h3 class="grow">Documents importés</h3>
      <input v-model="filter" type="search" placeholder="Filtrer par titre, fichier ou étiquette" aria-label="Filtrer les documents" />
    </div>
    <p v-if="!documents.length" class="muted">
      Aucun document pour l'instant : l'assistant répondra qu'il ne sait pas. Ajoutez vos guides et procédures ci-dessus.
    </p>
    <p v-else-if="!shown.length" class="muted">Aucun document ne correspond à « {{ filter }} ».</p>
    <p v-if="problem" class="err" role="alert">{{ problem }}</p>

    <table v-if="shown.length">
      <thead>
        <tr><th>Document</th><th>Format</th><th class="num">Passages</th><th class="num">Taille</th><th>Importé le</th><th><span class="sr">Actions</span></th></tr>
      </thead>
      <tbody>
        <template v-for="d in shown" :key="d.id">
          <tr>
            <td>
              <b>{{ d.title }}</b>
              <div class="muted small name">{{ d.name }}</div>
              <div v-if="d.tags.length" class="tags"><span v-for="t in d.tags" :key="t" class="pill">{{ t }}</span></div>
              <div v-if="d.secrets" class="small"><span class="pill bad">mots de passe repérés</span> <span class="muted">téléchargement bloqué, passages masqués</span></div>
              <details v-if="remarks(d).length" class="small">
                <summary>⚠ {{ remarks(d).length }} remarque{{ remarks(d).length > 1 ? 's' : '' }}</summary>
                <ul class="plain"><li v-for="w in remarks(d)" :key="w" class="warn">{{ w }}</li></ul>
              </details>
            </td>
            <td><span class="pill">{{ formatLabel(d.format) }}</span></td>
            <td class="num">{{ d.passages }}</td>
            <td class="num">{{ formatBytes(d.bytes) }}</td>
            <td class="muted small">{{ date(d.imported_at) }}</td>
            <td class="actions">
              <button type="button" :aria-expanded="open === d.id" @click="toggle(d)">{{ open === d.id ? 'Masquer' : 'Passages' }}</button>
              <a v-if="d.has_file && !d.secrets" :href="`/api/live/kb/documents/${d.id}/file`" download class="dl">Télécharger</a>
              <template v-if="confirming === d.id">
                <button type="button" class="danger" :disabled="deleting" @click="remove(d)">Confirmer la suppression</button>
                <button type="button" @click="confirming = 0">Annuler</button>
              </template>
              <button v-else type="button" class="danger" @click="confirming = d.id">Supprimer</button>
            </td>
          </tr>
          <tr v-if="open === d.id" class="passages">
            <td colspan="6">
              <p v-if="loading" class="muted">Chargement…</p>
              <ol v-else class="plain">
                <li v-for="p in passages" :key="p.id">
                  <button type="button" class="link" @click="$emit('open', p.id)">{{ p.section || p.title }}{{ p.page ? ` (p. ${p.page})` : '' }}</button>
                  <span class="muted small"> — {{ excerpt(p) }}</span>
                </li>
              </ol>
            </td>
          </tr>
        </template>
      </tbody>
    </table>
  </section>
</template>

<script setup>
import { ref, computed } from 'vue'
import axios from 'axios'
import { adminHeaders, errorText, formatBytes, formatLabel } from '../../utils/kb.js'

const props = defineProps({ documents: { type: Array, default: () => [] } })
const emit = defineEmits(['open', 'changed'])

const filter = ref('')
const open = ref(0)
const passages = ref([])
const loading = ref(false)
const confirming = ref(0)
const deleting = ref(false)
const problem = ref('')

const fold = (s) => String(s || '').normalize('NFD').replace(/[̀-ͯ]/g, '').toLowerCase()
const shown = computed(() => {
  const f = fold(filter.value).trim()
  const list = [...props.documents].sort((a, b) => a.title.localeCompare(b.title, 'fr'))
  return f ? list.filter((d) => fold(`${d.title} ${d.name} ${d.tags.join(' ')}`).includes(f)) : list
})

// L'avertissement sur les mots de passe a son propre repère : on ne le répète pas dans les remarques.
const remarks = (d) => (d.warnings || []).filter((w) => !(d.secrets && /^ce document semble contenir/.test(w)))
const date = (t) => new Date(t * 1000).toLocaleDateString('fr-FR', { day: '2-digit', month: '2-digit', year: 'numeric' })
const excerpt = (p) => {
  const t = (p.description || '').replace(/\s+/g, ' ').trim()
  return t.length > 140 ? `${t.slice(0, 137).trimEnd()}…` : t
}

async function toggle(d) {
  if (open.value === d.id) { open.value = 0; return }
  open.value = d.id
  loading.value = true
  passages.value = []
  problem.value = ''
  try {
    passages.value = (await axios.get(`/api/live/kb/documents/${d.id}`)).data.passages
  } catch (e) {
    problem.value = errorText(e)
  } finally {
    loading.value = false
  }
}

async function remove(d) {
  deleting.value = true
  problem.value = ''
  try {
    await axios.delete(`/api/live/kb/documents/${d.id}`, { headers: adminHeaders() })
    confirming.value = 0
    if (open.value === d.id) open.value = 0
    emit('changed')
  } catch (e) {
    problem.value = e.response?.status === 401 ? 'Connexion expirée : reconnectez-vous (menu Administration).' : errorText(e)
  } finally {
    deleting.value = false
  }
}
</script>

<style scoped src="../../assets/styles/Kb.css"></style>
<style scoped>
.name { overflow-wrap: anywhere; }
.tags { display: flex; flex-wrap: wrap; gap: .25rem; margin-top: .3rem; }
.actions { display: flex; gap: .4rem; flex-wrap: wrap; align-items: center; }
.dl { color: #8fb0ff; font-size: .9rem; }
.passages td { background: #151515; }
.passages ol { gap: .4rem; }
.sr { position: absolute; width: 1px; height: 1px; overflow: hidden; clip: rect(0, 0, 0, 0); }
</style>
