<template>
  <section class="card">
    <h3>Ajouter des documents</h3>
    <p class="muted small">
      Markdown, HTML, Word (.docx), PDF ou texte, jusqu'à {{ formatBytes(MAX_BYTES) }} par fichier. Chaque document est découpé en passages que l'assistant peut citer.
      Un fichier déjà importé est remplacé s'il a changé.
    </p>

    <div :class="['drop', over && 'over']" @dragover.prevent="over = true" @dragleave="over = false" @drop.prevent="onDrop">
      <span>Glissez vos fichiers ici, ou</span>
      <div class="row">
        <button type="button" class="primary" :disabled="sending" @click="picker.click()">Choisir des fichiers…</button>
        <button type="button" :disabled="sending" title="Envoie tous les documents d'un dossier, sous-dossiers compris" @click="folderPicker.click()">Choisir un dossier…</button>
      </div>
      <input ref="picker" type="file" multiple :accept="ACCEPT" hidden @change="onPick" />
      <input ref="folderPicker" type="file" webkitdirectory multiple hidden @change="onPick" />
    </div>

    <div class="row">
      <label class="grow">Étiquettes communes (facultatif)
        <input v-model="tags" type="text" placeholder="vpn, réseau" autocomplete="off" />
      </label>
      <label class="inline" title="Un fichier de « Guides/VPN/client.docx » reçoit les étiquettes « Guides » et « VPN »">
        <input v-model="withFolders" type="checkbox" /> Étiquettes tirées des noms de dossiers
      </label>
    </div>

    <div v-if="items.length" class="results">
      <div class="row">
        <b>{{ doneCount }} / {{ items.length }}</b>
        <span v-if="sending" class="muted small">envoi en cours…</span>
        <button v-if="sending" type="button" @click="cancel">Annuler</button>
      </div>
      <ul class="plain" aria-live="polite">
        <li v-for="it in items" :key="it.name" class="item">
          <span class="name">{{ it.name }}</span>
          <span class="muted small">{{ formatBytes(it.size) }}</span>
          <span :class="['pill', stateClass(it)]">{{ stateText(it) }}</span>
          <ul v-if="it.warnings.length" class="plain warnings">
            <li v-for="w in it.warnings" :key="w" class="warn small">⚠ {{ w }}</li>
          </ul>
        </li>
      </ul>
    </div>
    <p v-if="skipped" class="muted small">{{ skipped }} fichier{{ skipped > 1 ? 's' : '' }} ignoré{{ skipped > 1 ? 's' : '' }} : format non pris en charge.</p>
  </section>
</template>

<script setup>
import { ref, computed } from 'vue'
import axios from 'axios'
import { ACCEPT, MAX_BYTES, adminHeaders, errorText, folderTags, formatBytes, isReadable, outcomeView, parseTags, uploadName, uploadQuery } from '../../utils/kb.js'

const emit = defineEmits(['changed'])

const tags = ref('')
const withFolders = ref(true)
const over = ref(false)
const items = ref([]) // { file, name, size, state: waiting | sending | done | cancelled, tone, text, warnings, progress }
const sending = ref(false)
const skipped = ref(0)
const picker = ref(null)
const folderPicker = ref(null)
let abort = null
let stop = false

const doneCount = computed(() => items.value.filter((i) => i.state === 'done').length)

function onPick(e) { add([...e.target.files]); e.target.value = '' }
function onDrop(e) { over.value = false; add([...(e.dataTransfer?.files || [])]) }

function add(files) {
  if (sending.value || !files.length) return
  const readable = files.filter((f) => isReadable(f.name))
  skipped.value = files.length - readable.length
  if (!readable.length) { items.value = []; return }
  items.value = readable.map((file) => ({ file, name: uploadName(file), size: file.size, state: 'waiting', tone: '', text: '', warnings: [], progress: 0 }))
  run()
}

function finish(it, outcome) {
  const v = outcomeView(outcome)
  it.tone = v.tone
  it.text = v.text
  it.warnings = outcome?.warnings || []
  it.state = 'done'
}

async function run() {
  sending.value = true
  stop = false
  const common = parseTags(tags.value)
  for (const it of items.value) {
    if (stop) break
    if (it.size > MAX_BYTES) { finish(it, { status: 'failed', error: `fichier trop volumineux (${formatBytes(it.size)}, maximum ${formatBytes(MAX_BYTES)})` }); continue }
    it.state = 'sending'
    abort = new AbortController()
    try {
      const own = withFolders.value ? folderTags(it.name) : []
      const r = await axios.put(`/api/live/kb/documents?${uploadQuery(it.name, [...common, ...own])}`, it.file, {
        headers: { 'Content-Type': 'application/octet-stream', ...adminHeaders() },
        signal: abort.signal,
        onUploadProgress: (p) => { if (p.total) it.progress = Math.round((p.loaded / p.total) * 100) },
      })
      finish(it, r.data)
      if (r.data.status === 'added') emit('changed')
    } catch (e) {
      if (axios.isCancel(e)) { it.state = 'cancelled'; break }
      finish(it, { status: 'failed', error: e.response?.status === 401 ? 'connexion expirée : reconnectez-vous (menu Administration)' : errorText(e) })
    }
  }
  for (const it of items.value) if (it.state === 'waiting' || it.state === 'sending') it.state = 'cancelled'
  sending.value = false
  emit('changed')
}

function cancel() { stop = true; abort?.abort() }

const stateText = (it) => ({ waiting: 'en attente', sending: `envoi… ${it.progress}%`, cancelled: 'annulé' })[it.state] || it.text
const stateClass = (it) => (it.state !== 'done' ? '' : { ok: 'on', same: 'info', bad: 'bad' }[it.tone])
</script>

<style scoped src="../../assets/styles/Kb.css"></style>
<style scoped>
.item { display: flex; flex-wrap: wrap; align-items: center; gap: .3rem .7rem; padding: .3rem 0; border-bottom: 1px solid #2a2a2a; }
.name { overflow-wrap: anywhere; }
.warnings { flex: 1 1 100%; padding-left: 1rem; }
</style>
