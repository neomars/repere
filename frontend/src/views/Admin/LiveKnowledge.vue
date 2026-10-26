<template>
  <div class="wrap">
    <h2>Base de connaissances</h2>
    <p class="muted">
      L'assistant ne répond qu'à partir des documents d'ici : à chaque question, il reçoit les passages les plus proches et ne peut citer qu'eux.
      Sans passage pertinent, il dit qu'il ne sait pas et propose d'ouvrir un ticket.
    </p>
    <p class="stats" aria-live="polite">
      <b>{{ stats.documents }}</b> document{{ stats.documents > 1 ? 's' : '' }} · <b>{{ stats.passages }}</b> passage{{ stats.passages > 1 ? 's' : '' }}
      <template v-if="stats.images || stats.videos"> · {{ stats.images }} image{{ stats.images > 1 ? 's' : '' }} · {{ stats.videos }} vidéo{{ stats.videos > 1 ? 's' : '' }}</template>
    </p>
    <p v-if="problem" class="err" role="alert">{{ problem }}</p>

    <KbUpload @changed="reload" />
    <KbImport @changed="reload" />
    <KbSearch @open="viewing = $event" />
    <KbDocuments :documents="documents" @open="viewing = $event" @changed="reload" />
    <KbSynonyms />

    <PassageViewer v-if="viewing" :id="viewing" @close="viewing = 0" />
  </div>
</template>

<script setup>
import { ref, onMounted, onBeforeUnmount } from 'vue'
import axios from 'axios'
import KbUpload from '../../components/kb/KbUpload.vue'
import KbImport from '../../components/kb/KbImport.vue'
import KbSearch from '../../components/kb/KbSearch.vue'
import KbDocuments from '../../components/kb/KbDocuments.vue'
import KbSynonyms from '../../components/kb/KbSynonyms.vue'
import PassageViewer from '../../components/PassageViewer.vue'
import { errorText } from '../../utils/kb.js'

const documents = ref([])
const stats = ref({ documents: 0, passages: 0, images: 0, videos: 0 })
const problem = ref('')
const viewing = ref(0)
let timer = null
let last = 0

async function load() {
  try {
    const r = (await axios.get('/api/live/kb/documents')).data
    documents.value = r.documents
    stats.value = r.stats
    problem.value = ''
  } catch (e) {
    problem.value = e.response ? errorText(e) : 'Moteur live injoignable (live-engine démarré ?)'
  }
}

/** Recharge la liste, au plus une fois toutes les 1,5 s (un import de milliers de fichiers signale chaque document). */
function reload() {
  if (timer) return
  timer = setTimeout(async () => { timer = null; last = Date.now(); await load() }, Math.max(0, last + 1500 - Date.now()))
}

onMounted(() => { last = Date.now(); load() })
onBeforeUnmount(() => clearTimeout(timer))
</script>

<style scoped>
.wrap { padding: 1.5rem 2rem; overflow: auto; max-width: 1100px; display: flex; flex-direction: column; gap: 1rem; }
h2 { margin: 0; }
.muted { color: #999; margin: 0; line-height: 1.4; }
.err { color: #ff7b7b; margin: 0; }
.stats { margin: 0; font-size: 1.05rem; }
@media (max-width: 800px) { .wrap { padding: 1rem; } }
</style>
