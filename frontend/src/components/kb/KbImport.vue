<template>
  <section class="card">
    <h3>Importer un dossier de l'ordinateur du moteur</h3>
    <p class="muted small">
      Pour une base volumineuse (procédures exportées d'un intranet, partage de fichiers…). Le moteur lit lui-même le dossier : le chemin est celui
      <b>de la machine où tourne le moteur</b>. Seuls les formats lus sont importés ; les autres fichiers sont comptés mais laissés de côté.
    </p>
    <form class="form" @submit.prevent="start">
      <div class="row">
        <input v-model="path" type="text" placeholder="/home/…/procedures" required autocomplete="off" aria-label="Dossier à importer" :disabled="running" />
        <button v-if="native" type="button" :disabled="running" @click="browse">Parcourir…</button>
      </div>
      <div class="row">
        <label class="inline"><input v-model="recursive" type="checkbox" :disabled="running" /> Inclure les sous-dossiers</label>
        <label class="inline"><input v-model="folderTagsOn" type="checkbox" :disabled="running" /> Étiquettes tirées des noms de dossiers</label>
      </div>
      <div class="row">
        <label class="grow">Étiquettes communes (facultatif)
          <input v-model="tags" type="text" placeholder="procédures, dsi" autocomplete="off" :disabled="running" />
        </label>
        <label class="grow">Adresse de ce dossier sur l'intranet (facultatif)
          <input v-model="baseUrl" type="text" placeholder="https://intranet.exemple.fr/procedures/" autocomplete="off" :disabled="running" />
        </label>
      </div>
      <p class="muted small">Avec l'adresse du dossier, chaque source citée renvoie à la page d'origine sur l'intranet (adresse + chemin du fichier).</p>
      <div class="row">
        <button class="primary" type="submit" :disabled="running || !path.trim()">Importer</button>
        <button v-if="running" type="button" @click="cancel">Annuler</button>
      </div>
    </form>
    <p v-if="problem" class="err" role="alert">{{ problem }}</p>

    <div v-if="status.running || status.finished" class="progress" aria-live="polite">
      <div class="bar"><div :style="{ width: percent + '%' }" /></div>
      <p class="small">
        <template v-if="status.running">Lecture de {{ status.done }} / {{ status.total }} fichiers<template v-if="status.current"> — <code>{{ status.current }}</code></template></template>
        <template v-else>{{ status.cancelled ? 'Import annulé' : 'Import terminé' }} en {{ status.seconds }} s.</template>
      </p>
      <p class="small">
        <span class="ok">{{ status.added }} ajouté{{ status.added > 1 ? 's' : '' }}</span> ·
        {{ status.replaced }} remplacé{{ status.replaced > 1 ? 's' : '' }} ·
        {{ status.unchanged }} inchangé{{ status.unchanged > 1 ? 's' : '' }} ·
        {{ status.duplicates }} doublon{{ status.duplicates > 1 ? 's' : '' }} ·
        <span :class="status.failed ? 'err' : ''">{{ status.failed }} en échec</span> ·
        {{ status.passages }} passage{{ status.passages > 1 ? 's' : '' }}
      </p>
      <p v-if="skippedText" class="muted small">Laissés de côté (formats non lus) : {{ skippedText }}.</p>
      <details v-if="status.errors.length" open>
        <summary>{{ status.errors.length }} fichier{{ status.errors.length > 1 ? 's' : '' }} en échec</summary>
        <ul class="plain small"><li v-for="e in status.errors" :key="e" class="err">{{ e }}</li></ul>
      </details>
      <details v-if="status.warnings.length" open>
        <summary>{{ status.warnings.length }} remarque{{ status.warnings.length > 1 ? 's' : '' }}</summary>
        <ul class="plain small"><li v-for="w in status.warnings" :key="w" class="warn">⚠ {{ w }}</li></ul>
      </details>
    </div>
  </section>
</template>

<script setup>
import { ref, computed, onMounted, onBeforeUnmount } from 'vue'
import axios from 'axios'
import { adminHeaders, errorText, parseTags } from '../../utils/kb.js'

const emit = defineEmits(['changed'])

const path = ref('')
const recursive = ref(true)
const folderTagsOn = ref(true)
const tags = ref('')
const baseUrl = ref('')
const problem = ref('')
const status = ref({ running: false, finished: false, errors: [], warnings: [], skipped: {}, total: 0, done: 0, added: 0, replaced: 0, unchanged: 0, duplicates: 0, failed: 0, passages: 0, seconds: 0, current: '' })
const native = typeof window !== 'undefined' && !!window.repereNative?.pickFolder // application de bureau : boîte de dialogue du système
let timer = null
let lastDone = -1

const running = computed(() => status.value.running)
const percent = computed(() => (status.value.total ? Math.min(100, Math.round((status.value.done / status.value.total) * 100)) : status.value.finished ? 100 : 0))
const skippedText = computed(() => Object.entries(status.value.skipped || {}).map(([ext, n]) => `${n} .${ext}`).join(', '))

async function refresh() {
  try {
    const was = status.value.running
    status.value = (await axios.get('/api/live/kb/import')).data
    if (status.value.done !== lastDone) { lastDone = status.value.done; emit('changed') } // la liste des documents suit l'import
    if (status.value.running) schedule()
    else if (was) emit('changed')
  } catch (e) {
    problem.value = errorText(e)
  }
}

function schedule() {
  clearTimeout(timer)
  timer = setTimeout(refresh, 1000)
}

async function browse() {
  const chosen = await window.repereNative.pickFolder()
  if (chosen) path.value = chosen
}

async function start() {
  problem.value = ''
  try {
    const r = await axios.post('/api/live/kb/import', {
      path: path.value.trim(),
      recursive: recursive.value,
      folder_tags: folderTagsOn.value,
      tags: parseTags(tags.value),
      base_url: baseUrl.value.trim() || null,
    }, { headers: adminHeaders() })
    status.value = r.data
    lastDone = -1
    schedule()
  } catch (e) {
    problem.value = e.response?.status === 401 ? 'Connexion expirée : reconnectez-vous (menu Administration).' : errorText(e)
  }
}

async function cancel() {
  try { await axios.post('/api/live/kb/import/cancel', {}, { headers: adminHeaders() }) } catch (e) { problem.value = errorText(e) }
}

onMounted(refresh)
onBeforeUnmount(() => clearTimeout(timer))
</script>

<style scoped src="../../assets/styles/Kb.css"></style>
<style scoped>
.form { display: flex; flex-direction: column; gap: .6rem; }
.progress { display: flex; flex-direction: column; gap: .35rem; }
.progress p { margin: 0; }
</style>
