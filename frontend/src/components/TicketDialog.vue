<template>
  <div class="overlay" @click.self="closeIfUntouched">
    <div ref="box" class="dialog" role="dialog" aria-modal="true" :aria-labelledby="titleId" tabindex="-1">
      <header>
        <h3 :id="titleId">Ouvrir un ticket</h3>
        <button type="button" class="close" aria-label="Fermer" @click="emit('close')">✕</button>
      </header>

      <p v-if="loading" class="muted" role="status">Je prépare le ticket à partir de votre conversation…</p>
      <template v-else-if="error">
        <p class="error" role="alert">{{ error }}</p>
        <div class="actions"><button type="button" @click="emit('close')">Fermer</button></div>
      </template>

      <div v-else-if="saved" class="done">
        <p class="ok" role="status">✔ Ticket <b>{{ saved.reference }}</b> enregistré.</p>
        <p class="muted">
          Le support le retrouve dans son écran de suivi (Administration › Tickets). Gardez la référence <b>{{ saved.reference }}</b> ;
          vous pouvez aussi copier le texte pour l'envoyer par courriel ou le coller dans votre outil de tickets habituel.
        </p>
        <div class="actions">
          <button type="button" @click="copy">{{ copied ? 'Copié ✓' : 'Copier le texte' }}</button>
          <button type="button" class="primary" @click="emit('close')">Fermer</button>
        </div>
      </div>

      <form v-else-if="draft" @submit.prevent="save">
        <p class="muted">
          {{ draft.summarized ? 'Voici un résumé de votre conversation.' : "Le résumé automatique n'était pas disponible : voici vos messages." }}
          Relisez-le et corrigez-le au besoin : ce texte part tel quel au support.
        </p>
        <label>Titre
          <input v-model="title" maxlength="200" required autocomplete="off" />
        </label>
        <label>Description
          <textarea v-model="description" rows="12" maxlength="8000" required />
        </label>
        <p v-if="draft.sources.length" class="muted small">
          Sources consultées dans la base de connaissances, jointes au ticket : {{ draft.sources.map((s) => sourcePath(s)).join(' · ') }}
        </p>
        <p v-if="problem" class="error" role="alert">{{ problem }}</p>
        <div class="actions">
          <button type="button" @click="copy">{{ copied ? 'Copié ✓' : 'Copier le texte' }}</button>
          <span class="spacer" />
          <button type="button" @click="emit('close')">Annuler</button>
          <button type="submit" class="primary" :disabled="saving || !title.trim() || !description.trim()">{{ saving ? 'Enregistrement…' : 'Enregistrer le ticket' }}</button>
        </div>
      </form>
    </div>
  </div>
</template>

<script setup>
import { ref, computed, watch, onMounted, onBeforeUnmount } from 'vue'
import axios from 'axios'
import { errorText, sourcePath, ticketText } from '../utils/kb.js'

const props = defineProps({
  /** Le brouillon préparé par le moteur : { title, description, sources, summarized }. */
  draft: { type: Object, default: null },
  loading: { type: Boolean, default: false },
  error: { type: String, default: '' },
  userName: { type: String, default: '' },
  /** Profil d'assistant de la conversation (noté dans le ticket). */
  conversation: { type: String, default: '' },
})
const emit = defineEmits(['close', 'saved'])

const titleId = `ticket-${Math.random().toString(36).slice(2, 8)}`
const box = ref(null)
const title = ref('')
const description = ref('')
const saving = ref(false)
const saved = ref(null)
const problem = ref('')
const copied = ref(false)
let before = null
let copiedTimer = null

watch(() => props.draft, (d) => {
  if (!d) return
  title.value = d.title
  description.value = d.description
}, { immediate: true })

async function save() {
  if (saving.value) return
  saving.value = true
  problem.value = ''
  try {
    const r = await axios.post('/api/live/tickets', {
      title: title.value.trim(),
      description: description.value.trim(),
      user_name: props.userName,
      conversation: props.conversation,
      sources: props.draft?.sources || [],
    })
    saved.value = r.data
    emit('saved', r.data)
  } catch (e) {
    problem.value = `Le ticket n'a pas pu être enregistré : ${errorText(e)}. Votre texte est conservé ; vous pouvez le copier.`
  } finally {
    saving.value = false
  }
}

async function copy() {
  const text = ticketText(saved.value ? { ...saved.value } : { title: title.value, description: description.value })
  try {
    await navigator.clipboard.writeText(text)
  } catch {
    // presse-papiers refusé : on sélectionne le texte pour que Ctrl+C fonctionne
    const area = box.value?.querySelector('textarea')
    if (area) { area.focus(); area.select() }
    return
  }
  copied.value = true
  clearTimeout(copiedTimer)
  copiedTimer = setTimeout(() => { copied.value = false }, 2000)
}

// Le texte a été modifié et pas encore enregistré : un clic à côté ou la touche Échap ne doivent pas le faire perdre.
const dirty = computed(() => !!props.draft && !saved.value && (title.value !== props.draft.title || description.value !== props.draft.description))
function closeIfUntouched() { if (!dirty.value) emit('close') }
function onKey(e) { if (e.key === 'Escape') closeIfUntouched() }

onMounted(() => { before = document.activeElement; document.addEventListener('keydown', onKey); box.value?.focus() })
onBeforeUnmount(() => { clearTimeout(copiedTimer); document.removeEventListener('keydown', onKey); before?.focus?.() })
</script>

<style scoped>
.overlay { position: fixed; inset: 0; z-index: 50; display: flex; align-items: center; justify-content: center; padding: 1rem; background: rgba(0,0,0,.65); }
.dialog { display: flex; flex-direction: column; gap: .8rem; width: min(680px, 100%); max-height: 92vh; overflow-y: auto; padding: 1.1rem 1.3rem; background: #1b1b1b; border: 1px solid #444; border-radius: 12px; color: #eee; outline: none; }
header { display: flex; justify-content: space-between; align-items: center; gap: 1rem; }
h3 { margin: 0; font-size: 1.1rem; }
form { display: flex; flex-direction: column; gap: .7rem; }
label { display: flex; flex-direction: column; gap: .25rem; color: #ccc; font-size: .9rem; }
input, textarea { padding: .5rem .6rem; border-radius: 8px; border: 1px solid #444; background: #121212; color: #eee; font: inherit; }
textarea { resize: vertical; min-height: 8rem; line-height: 1.4; }
.actions { display: flex; gap: .5rem; align-items: center; flex-wrap: wrap; }
.spacer { flex: 1; }
button { cursor: pointer; padding: .45rem .9rem; border-radius: 8px; border: 1px solid #555; background: #2a2a2a; color: #eee; }
button:disabled { opacity: .5; cursor: default; }
.primary { background: #4a6cf7; border-color: #4a6cf7; }
.close { padding: .2rem .55rem; }
.muted { color: #999; margin: 0; line-height: 1.4; }
.small { font-size: .85rem; }
.error { color: #ff7b7b; margin: 0; }
.ok { color: #7fd99b; margin: 0; font-size: 1.05rem; }
.done { display: flex; flex-direction: column; gap: .8rem; }
</style>
