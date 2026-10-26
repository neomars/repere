<template>
  <div class="overlay" @click.self="emit('close')">
    <div ref="box" class="dialog" role="dialog" aria-modal="true" :aria-labelledby="titleId" tabindex="-1">
      <header>
        <div class="head">
          <h3 :id="titleId">{{ data ? data.passage.title : 'Extrait de la base de connaissances' }}</h3>
          <p v-if="data" class="where">
            <span v-if="data.passage.section">{{ data.passage.section }}</span>
            <span v-if="data.passage.page"> · page {{ data.passage.page }}</span>
            <span v-if="data.document"> · {{ formatLabel(data.document.format) }}</span>
          </p>
        </div>
        <button type="button" class="close" aria-label="Fermer" @click="emit('close')">✕</button>
      </header>

      <p v-if="error" class="error" role="alert">{{ error }}</p>
      <p v-else-if="!data" class="muted" role="status">Chargement…</p>
      <template v-else>
        <!-- Pendant qu'on passe d'un extrait à l'autre, l'ancien reste affiché : les boutons gardent le focus -->
        <div :class="['text', loading && 'busy']" :aria-busy="loading">{{ data.passage.description }}</div>
        <footer>
          <div class="nav">
            <button type="button" :disabled="loading || !data.previous" :title="data.previous?.section" @click="go(data.previous.id)">← Passage précédent</button>
            <button type="button" :disabled="loading || !data.next" :title="data.next?.section" @click="go(data.next.id)">Passage suivant →</button>
          </div>
          <a v-if="webLink" :href="webLink" target="_blank" rel="noopener noreferrer">Ouvrir le document d'origine ↗</a>
          <a v-else-if="fileLink" :href="fileLink" download>Télécharger le document d'origine</a>
          <span v-else class="muted small">Le fichier d'origine n'est pas disponible.</span>
        </footer>
      </template>
    </div>
  </div>
</template>

<script setup>
import { ref, computed, watch, onMounted, onBeforeUnmount } from 'vue'
import axios from 'axios'
import { errorText, formatLabel, safeHref } from '../utils/kb.js'

const props = defineProps({ id: { type: Number, required: true } })
const emit = defineEmits(['close'])

const titleId = `passage-${Math.random().toString(36).slice(2, 8)}`
const box = ref(null)
const data = ref(null)
const loading = ref(true)
const error = ref('')
const current = ref(props.id)
let seq = 0 // une réponse qui arrive après une navigation plus récente est ignorée
let before = null

const webLink = computed(() => safeHref(data.value?.passage.link || data.value?.document?.url))
const fileLink = computed(() => {
  const d = data.value?.document
  return d && d.has_file && !d.secrets ? `/api/live/kb/documents/${d.id}/file` : ''
})

async function load(id) {
  const mine = ++seq
  loading.value = true
  error.value = ''
  try {
    const r = await axios.get(`/api/live/kb/passages/${id}`)
    if (mine !== seq) return
    data.value = r.data
    box.value?.querySelector('.text')?.scrollTo?.(0, 0)
  } catch (e) {
    if (mine !== seq) return
    data.value = null
    error.value = e.response?.status === 404 ? "Cet extrait n'est plus dans la base de connaissances (le document a été supprimé ou remplacé)." : errorText(e)
  } finally {
    if (mine === seq) loading.value = false
  }
}

function go(id) { current.value = id }

// Échap ferme la fenêtre où que soit le focus (un bouton qui disparaît en le perdant ne doit pas la rendre sourde au clavier).
function onKey(e) { if (e.key === 'Escape') emit('close') }

watch(current, load)
watch(() => props.id, (id) => { current.value = id })
onMounted(() => {
  before = document.activeElement
  document.addEventListener('keydown', onKey)
  load(current.value)
  box.value?.focus()
})
onBeforeUnmount(() => { seq++; document.removeEventListener('keydown', onKey); before?.focus?.() })
</script>

<style scoped>
.overlay { position: fixed; inset: 0; z-index: 50; display: flex; align-items: center; justify-content: center; padding: 1rem; background: rgba(0,0,0,.65); }
.dialog { display: flex; flex-direction: column; gap: .8rem; width: min(760px, 100%); max-height: 90vh; padding: 1.1rem 1.3rem; background: #1b1b1b; border: 1px solid #444; border-radius: 12px; color: #eee; outline: none; }
header { display: flex; justify-content: space-between; align-items: flex-start; gap: 1rem; }
.head { min-width: 0; }
h3 { margin: 0; font-size: 1.1rem; overflow-wrap: anywhere; }
.where { margin: .2rem 0 0; color: #9db4ff; font-size: .85rem; }
.close { flex: none; padding: .2rem .55rem; }
.text.busy { opacity: .55; }
.text { overflow-y: auto; white-space: pre-wrap; overflow-wrap: anywhere; line-height: 1.5; background: #121212; border: 1px solid #2c2c2c; border-radius: 8px; padding: .8rem 1rem; }
footer { display: flex; flex-wrap: wrap; align-items: center; justify-content: space-between; gap: .6rem 1rem; }
.nav { display: flex; gap: .5rem; flex-wrap: wrap; }
footer a { color: #8fb0ff; }
button { cursor: pointer; padding: .4rem .8rem; border-radius: 6px; border: 1px solid #555; background: #2a2a2a; color: #eee; }
button:disabled { opacity: .45; cursor: default; }
.muted { color: #999; margin: 0; }
.small { font-size: .85rem; }
.error { color: #ff7b7b; margin: 0; }
</style>
