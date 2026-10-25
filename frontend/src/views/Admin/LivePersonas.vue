<template>
  <div class="wrap">
    <h2>Profils d'assistant</h2>
    <div class="layout">
      <ul class="list">
        <li v-for="p in personas" :key="p.id" :class="{ active: p.id === form?.id }" @click="edit(p)">{{ p.name }}</li>
        <li class="new" @click="create">+ Nouveau profil</li>
      </ul>

      <form v-if="form" class="form" @submit.prevent="save">
        <fieldset class="block">
          <legend>Identité</legend>
          <label>Nom <input v-model="form.name" required placeholder="ex. Repère" /></label>
          <label>Identifiant <span class="hint">(généré à partir du nom ; lettres, chiffres, - et _)</span>
            <input v-model="form.id" :disabled="existing" pattern="[A-Za-z0-9_\-]+" required @input="idTouched = true" /></label>
          <label>Présentation <span class="hint">(affichée sur l'écran de départ)</span>
            <textarea v-model="form.summary" rows="2" /></label>
        </fieldset>

        <fieldset class="block">
          <legend>Posture et style</legend>
          <label>Posture <span class="hint">(comment l'assistant se comporte, ce qui le définit)</span>
            <textarea v-model="form.personality" rows="3" /></label>
          <div class="chips">
            <button type="button" v-for="t in traits" :key="t" class="chip" @click="addTrait(t)">+ {{ t }}</button>
          </div>
          <label>Contexte <span class="hint">(organisation, périmètre, outils en place)</span>
            <textarea v-model="form.background" rows="3" placeholder="ex. Support de premier niveau de la DSI ; postes Windows 11, messagerie Outlook, VPN d'entreprise." /></label>
          <label>Style de réponse <span class="hint">(longueur, vouvoiement, mise en forme)</span>
            <textarea v-model="form.speaking_style" rows="3" /></label>
        </fieldset>

        <fieldset class="block">
          <legend>Champs libres</legend>
          <p class="hint">Ajoutez les rubriques que vous voulez (service, horaires, outils…) : l'IA les lit comme le reste du profil.</p>
          <div v-for="(f, i) in form.custom_fields" :key="i" class="custom">
            <input v-model="f.label" list="field-labels" placeholder="Rubrique (ex. Horaires)" />
            <textarea v-model="f.value" rows="2" placeholder="Contenu" />
            <button type="button" @click="form.custom_fields.splice(i, 1)" title="Retirer ce champ">✕</button>
          </div>
          <datalist id="field-labels"><option v-for="o in fieldLabels" :key="o" :value="o" /></datalist>
          <div><button type="button" @click="form.custom_fields.push({ label: '', value: '' })">+ Ajouter un champ</button></div>
        </fieldset>

        <fieldset class="block">
          <legend>Consignes</legend>
          <label>Consignes supplémentaires <span class="hint">(en plus des règles du moteur, qui ne se modifient pas)</span>
            <textarea v-model="form.scenario" rows="3" /></label>
          <label>Limites supplémentaires <span class="hint">(sujets que ce profil ne traite pas)</span>
            <textarea v-model="form.boundaries" rows="2" /></label>
          <label>Message d'accueil <span class="hint">(peut contenir des boutons de réponse rapide : <code>[[replies: Choix 1 | Choix 2]]</code> ; vide : l'assistant ouvre lui-même la conversation)</span>
            <textarea v-model="form.first_message" rows="3" /></label>
        </fieldset>
        <fieldset class="voice">
          <legend>Voix</legend>
          <label>Voix de référence
            <select v-model="form.voice">
              <option value="">(voix par défaut du modèle)</option>
              <option v-for="v in voices" :key="v" :value="v">{{ v }}</option>
            </select>
          </label>
          <div class="upload">
            <input v-model="newVoice" placeholder="nom de la nouvelle voix (ex. marie)" pattern="[A-Za-z0-9_\-]+" />
            <input type="file" accept="audio/*" ref="fileEl" />
            <button type="button" @click="uploadVoice">Ajouter</button>
            <button type="button" v-if="form.voice" @click="deleteVoice">Supprimer « {{ form.voice }} »</button>
          </div>
          <p class="hint">Échantillon de 10 à 20 secondes, une seule personne, sans bruit ni musique, d'un ton posé et neutre.
            Utilisez votre propre voix ou un enregistrement dont vous détenez les droits.</p>
          <label>Expressivité : <b>{{ form.tts_exaggeration }}</b> <span class="hint">(0,5 = neutre)</span>
            <input type="range" min="0.25" max="1.5" step="0.05" v-model.number="form.tts_exaggeration" /></label>
          <label>Rythme : <b>{{ form.tts_cfg_weight }}</b> <span class="hint">(plus bas = plus lent et posé)</span>
            <input type="range" min="0" max="1" step="0.05" v-model.number="form.tts_cfg_weight" /></label>
          <div class="upload">
            <input v-model="previewText" />
            <button type="button" :disabled="previewing" @click="preview">{{ previewing ? '…' : 'Écouter' }}</button>
          </div>
        </fieldset>
        <fieldset class="block">
          <legend>Réglages du modèle de texte</legend>
          <p class="hint">Réglages d'écriture pour ce profil. Choisissez un style, puis ajustez si besoin ; sans réglage,
            les valeurs par défaut de l'app s'appliquent. Pour du support, gardez la température basse (0,2 à 0,4) :
            l'assistant doit coller à ses sources.</p>
          <div class="chips">
            <button v-for="(p, k) in PRESETS" :key="k" type="button" :class="['chip', presetKey === k && 'on']" @click="applyPreset(k)">{{ p.label }}</button>
          </div>
          <p v-if="PRESETS[presetKey]" class="hint">{{ PRESETS[presetKey].hint }}</p>
          <div class="grid">
            <label>Température <input type="number" step="0.05" min="0.1" max="1.5" v-model.number="form.temperature" /></label>
            <label :class="{ off: s.mirostat }" :title="s.mirostat ? 'Ignoré avec Mirostat' : ''">Top-p <input type="number" step="0.01" min="0.5" max="1" v-model.number="s.top_p" :disabled="s.mirostat" /></label>
            <label :class="{ off: s.mirostat }" :title="s.mirostat ? 'Ignoré avec Mirostat' : ''">Min-p <input type="number" step="0.01" min="0" max="0.5" v-model.number="s.min_p" :disabled="s.mirostat" /></label>
            <label>Pénalité de répétition <input type="number" step="0.01" min="1" max="1.5" v-model.number="s.repeat_penalty" /></label>
            <label>Pénalité de présence <input type="number" step="0.05" min="0" max="1" v-model.number="s.presence_penalty" /></label>
            <label>Longueur max (jetons) <input type="number" step="1" min="64" max="2048" v-model.number="s.max_tokens" placeholder="illimitée" /></label>
          </div>
          <p class="hint">Une réponse de support dépasse rarement 400 jetons ; au-delà de la limite, elle est coupée net (« … »).</p>
          <label class="check"><input type="checkbox" v-model="s.mirostat" /> Mirostat 2 (règle seul la diversité ; remplace top-p et min-p)</label>
          <div v-if="s.mirostat" class="grid">
            <label>Tau (4,5 à 5) <input type="number" step="0.1" min="1" max="10" v-model.number="s.mirostat_tau" /></label>
            <label>Eta <input type="number" step="0.01" min="0.01" max="1" v-model.number="s.mirostat_eta" /></label>
          </div>
          <label class="check"><input type="checkbox" v-model="s.dry" /> DRY léger : évite les phrases qui se répètent (utile sur les petits modèles)</label>
          <label class="check"><input type="checkbox" v-model="s.xtc" /> XTC léger : écarte parfois le mot le plus probable (plus de variété, moins de cohérence : à éviter pour du support)</label>
        </fieldset>
        <div class="row">
          <button class="primary" type="submit">Enregistrer</button>
          <button type="button" v-if="existing" @click="remove">Supprimer</button>
          <span class="msg">{{ message }}</span>
        </div>
      </form>
    </div>
  </div>
</template>

<script setup>
import { ref, computed, watch, onMounted } from 'vue'
import axios from 'axios'

const personas = ref([])
const form = ref(null)
const message = ref('')
const voices = ref([])
const newVoice = ref('')
const fileEl = ref(null)
const previewText = ref('Bonjour, je suis l\'assistant de support informatique. Comment puis-je vous aider ?')
const previewing = ref(false)
const existing = computed(() => !!form.value && personas.value.some((p) => p.id === form.value.id))

const headers = () => {
  const t = localStorage.getItem('live_admin_token')
  return t ? { Authorization: `Bearer ${t}` } : {}
}

const idTouched = ref(false)

const fieldLabels = ['Service', 'Horaires', 'Outils', 'Périmètre', 'Public', 'Site de la DSI']
const traits = ['courtois', 'calme', 'précis', 'patient', 'pédagogue', 'concis', 'rassurant', 'direct']

function addTrait(t) {
  const cur = form.value.personality.trim()
  form.value.personality = cur ? `${cur.replace(/[.\s]+$/, '')}, ${t}.` : `${t[0].toUpperCase()}${t.slice(1)}.`
}

const slug = (n) => n.normalize('NFD').replace(/[̀-ͯ]/g, '').toLowerCase().replace(/[^a-z0-9]+/g, '-').replace(/^-+|-+$/g, '')
// Nouveau profil : l'identifiant suit le nom tant qu'on ne l'a pas modifié à la main.
watch(() => form.value?.name, (n) => {
  if (form.value && !existing.value && !idTouched.value) form.value.id = slug(n || '')
})

const blank = () => ({
  id: '', name: '', background: '', custom_fields: [], sampling: { ...SAMPLING_BLANK },
  summary: '', personality: '', speaking_style: '', scenario: '',
  boundaries: '', first_message: '', language: 'fr', voice: '', temperature: 0.3,
  tts_exaggeration: 0.5, tts_cfg_weight: 0.5,
})

// Styles d'écriture. Les noms des réglages sont ceux de llama-server ; avec Mirostat, llama.cpp ignore top-p et min-p (on ne les envoie donc pas).
const PRESETS = {
  default: {
    label: 'Par défaut', hint: 'Réglages de l\'app : température basse (0,3).',
    temperature: 0.3, sampling: { top_p: 0.95, min_p: 0.05, repeat_penalty: 1.08, presence_penalty: 0, max_tokens: null, mirostat: false },
  },
  factual: {
    label: 'Factuel', hint: 'Réponses les plus stables et les plus proches des sources : température basse, tri strict, réponses de 500 jetons au plus.',
    temperature: 0.2, sampling: { top_p: 0.90, min_p: 0.05, repeat_penalty: 1.05, presence_penalty: 0, max_tokens: 500, mirostat: false },
  },
  flexible: {
    label: 'Plus souple', hint: 'Un peu plus de variété dans la formulation, sans s\'éloigner des sources : température 0,5, réponses de 600 jetons au plus.',
    temperature: 0.5, sampling: { top_p: 0.95, min_p: 0.05, repeat_penalty: 1.08, presence_penalty: 0, max_tokens: 600, mirostat: false },
  },
}
const SAMPLING_BLANK = { top_p: null, min_p: null, repeat_penalty: null, presence_penalty: null, max_tokens: null, mirostat: false, mirostat_tau: null, mirostat_eta: null, dry: false, xtc: false }
const s = computed(() => form.value.sampling)
const near = (a, b) => (a ?? null) === (b ?? null) || (a != null && b != null && Math.abs(a - b) < 1e-6)
// Le style actif est reconnu d'après les valeurs : modifier un champ à la main le désélectionne.
const presetKey = computed(() => {
  const f = form.value
  if (!f) return ''
  return Object.keys(PRESETS).find((k) => {
    const p = PRESETS[k]
    return near(f.temperature, p.temperature) && Object.entries(p.sampling).every(([key, v]) => (typeof v === 'boolean' ? !!f.sampling[key] === v : near(f.sampling[key], v)))
      && !f.sampling.dry && !f.sampling.xtc
  }) || ''
})
function applyPreset(k) {
  const p = PRESETS[k]
  form.value.temperature = p.temperature
  form.value.sampling = { ...SAMPLING_BLANK, ...p.sampling }
}
function cleanSampling(x) {
  const n = (v) => (v === '' || v == null || Number.isNaN(v) ? null : v)
  return {
    top_p: n(x.top_p), min_p: n(x.min_p), repeat_penalty: n(x.repeat_penalty), presence_penalty: n(x.presence_penalty),
    max_tokens: n(x.max_tokens), mirostat: !!x.mirostat, mirostat_tau: n(x.mirostat_tau), mirostat_eta: n(x.mirostat_eta),
    dry: !!x.dry, xtc: !!x.xtc,
  }
}

async function load() {
  personas.value = (await axios.get('/api/live/personas')).data
  await loadVoices()
}
async function loadVoices() {
  try { voices.value = (await axios.get('/api/live/voices')).data.voices } catch { voices.value = [] }
}
function edit(p) {
  idTouched.value = true
  form.value = {
    ...blank(), ...p, voice: p.voice || '', custom_fields: (p.custom_fields || []).map((f) => ({ ...f })), sampling: { ...SAMPLING_BLANK, ...(p.sampling || {}) },
    tts_exaggeration: p.tts_exaggeration ?? 0.5, tts_cfg_weight: p.tts_cfg_weight ?? 0.5,
  }
  message.value = ''
}
function create() { idTouched.value = false; form.value = blank(); message.value = '' }

async function save() {
  const body = {
    ...form.value, voice: form.value.voice || null,
    custom_fields: form.value.custom_fields.filter((f) => f.label.trim() && f.value.trim()),
    sampling: cleanSampling(form.value.sampling),
  }
  try {
    await axios.put(`/api/live/personas/${form.value.id}`, body, { headers: headers() })
    message.value = 'Enregistré'
    await load()
  } catch (e) { message.value = e.response?.data?.message || e.message }
}

async function uploadVoice() {
  const file = fileEl.value?.files?.[0]
  if (!file || !newVoice.value) { message.value = 'Choisissez un fichier audio et donnez un nom à la voix.'; return }
  try {
    await axios.put(`/api/live/voices/${encodeURIComponent(newVoice.value)}`, file,
      { headers: { ...headers(), 'Content-Type': 'application/octet-stream' } })
    await loadVoices()
    form.value.voice = newVoice.value
    newVoice.value = ''
    fileEl.value.value = ''
    message.value = 'Voix ajoutée'
  } catch (e) { message.value = e.response?.data?.message || e.message }
}

async function deleteVoice() {
  if (!confirm(`Supprimer la voix « ${form.value.voice} » ?`)) return
  try {
    await axios.delete(`/api/live/voices/${encodeURIComponent(form.value.voice)}`, { headers: headers() })
    form.value.voice = ''
    await loadVoices()
  } catch (e) { message.value = e.response?.data?.message || e.message }
}

async function preview() {
  previewing.value = true
  message.value = ''
  try {
    const { data } = await axios.post('/api/live/tts/preview', {
      text: previewText.value, voice: form.value.voice || null,
      exaggeration: form.value.tts_exaggeration, cfg_weight: form.value.tts_cfg_weight,
    }, { headers: headers(), responseType: 'blob' })
    const url = URL.createObjectURL(data)
    const a = new Audio(url)
    a.onended = () => URL.revokeObjectURL(url)
    await a.play()
  } catch (e) {
    let msg = e.message
    try { msg = JSON.parse(await e.response.data.text()).message } catch { /* corps non JSON */ }
    message.value = `Écoute impossible : ${msg} (la voix est-elle chargée dans « Modèles IA » ?)`
  } finally { previewing.value = false }
}

async function remove() {
  if (!confirm(`Supprimer ${form.value.name} ?`)) return
  await axios.delete(`/api/live/personas/${form.value.id}`, { headers: headers() })
  form.value = null
  await load()
}

onMounted(load)
</script>

<style scoped>
.wrap { padding: 1.5rem 2rem; overflow: auto; }
.layout { display: flex; gap: 2rem; align-items: flex-start; }
.list { list-style: none; padding: 0; margin: 0; min-width: 180px; }
.list li { padding: .5rem .8rem; cursor: pointer; border-radius: 6px; }
.list li:hover, .list li.active { background: #2a2a2a; }
.list .new { color: #8fb0ff; }
.form { flex: 1; max-width: 720px; display: flex; flex-direction: column; gap: .8rem; }
label { display: flex; flex-direction: column; gap: .25rem; font-size: .9rem; color: #bbb; }
input, textarea { padding: .5rem; background: #1b1b1b; color: #eee; border: 1px solid #444; border-radius: 6px; font: inherit; }
.row { display: flex; gap: .6rem; align-items: center; }
button { cursor: pointer; padding: .5rem 1rem; border-radius: 6px; border: 1px solid #555; background: #2a2a2a; color: #eee; }
.primary { background: #4a6cf7; border-color: #4a6cf7; }
.msg { color: #9c9; }
.voice { border: 1px solid #333; border-radius: 8px; padding: .8rem; display: flex; flex-direction: column; gap: .7rem; }
.voice legend { color: #bbb; padding: 0 .4rem; }
.upload { display: flex; gap: .5rem; align-items: center; flex-wrap: wrap; }
.upload input[type=text], .upload input:not([type]) { flex: 1; min-width: 180px; }
.hint { color: #888; font-size: .82rem; margin: 0; font-weight: normal; }
.block { border: 1px solid #333; border-radius: 8px; padding: .8rem; display: flex; flex-direction: column; gap: .7rem; }
.block legend { color: #bbb; padding: 0 .4rem; }
.block .grid { display: grid; grid-template-columns: repeat(auto-fit, minmax(170px, 1fr)); gap: .6rem .9rem; }
.block .grid label.off { opacity: .45; }
.chips { display: flex; flex-wrap: wrap; gap: .4rem; }
.chip { padding: .3rem .7rem; border-radius: 999px; border: 1px solid #555; background: #222; color: #ddd; cursor: pointer; }
.chip.on { background: #2a4d8f; border-color: #3a64b8; color: #fff; }
.check { flex-direction: row !important; align-items: center; gap: .4rem !important; }
.grid { display: grid; grid-template-columns: 1fr 1fr; gap: .8rem; }
.chips { display: flex; flex-wrap: wrap; gap: .35rem; }
.chip { padding: .2rem .6rem; border-radius: 999px; font-size: .8rem; background: #222; border-color: #3a3a3a; color: #aab; }
.chip:hover { background: #2f3550; }
.custom { display: grid; grid-template-columns: 200px 1fr auto; gap: .5rem; align-items: start; }
@media (max-width: 760px) { .layout { flex-direction: column; } .grid { grid-template-columns: 1fr; } .custom { grid-template-columns: 1fr; } }
select { padding: .5rem; background: #1b1b1b; color: #eee; border: 1px solid #444; border-radius: 6px; }
</style>
