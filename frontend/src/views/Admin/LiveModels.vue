<template>
  <div class="wrap">
    <h2>Modèles IA</h2>

    <section class="gpu">
      <div v-if="status.gpu">
        <b>{{ status.gpu.name }}</b> — VRAM {{ gb(status.gpu.used_mb * MB) }} / {{ gb(status.gpu.total_mb * MB) }} utilisés
        <div class="bar"><div :style="{ width: pct(status.gpu.used_mb, status.gpu.total_mb) + '%' }" /></div>
      </div>
      <div v-else class="muted">GPU NVIDIA non détecté (<code>nvidia-smi</code> introuvable) — les calculs utilisent {{ status.configured_vram_gb }} Go de VRAM de la configuration.</div>
      <div v-if="status.managed === false" class="warn">Mode manuel (<code>engine.managed = false</code>) : l'app ne lance pas les serveurs, elle se connecte à ceux que vous avez démarrés.</div>
    </section>

    <section class="cards">
      <div v-for="c in comps" :key="c.key" class="card">
        <div class="title">{{ c.label }} <span :class="['pill', stateOf(c.key)]">{{ stateLabel(stateOf(c.key)) }}</span></div>
        <div class="muted">{{ modelName(status[c.key]?.model) || 'aucun modèle chargé' }}</div>
        <div v-if="status[c.key]?.error" class="err">{{ status[c.key].error }}</div>
        <div v-if="status[c.key]?.warning" class="warn">⚠ {{ status[c.key].warning }}</div>
        <div v-if="status[c.key]?.gpu_mb" class="muted small">VRAM utilisée par ce serveur : {{ gb(status[c.key].gpu_mb * MB) }}</div>
        <div v-if="c.key === 'llm' && status.llm?.vision && stateOf('llm') !== 'stopped'" class="muted small">
          👁 Vision active : ce modèle voit les images (lecture des captures d'écran).
        </div>
        <div v-if="c.key === 'llm' && status.llm?.plan" class="muted small">
          {{ status.llm.plan.full_offload ? 'Tout sur le GPU' : 'Offload partiel' }} ·
          {{ status.llm.plan.n_gpu_layers }} couches GPU · contexte {{ status.llm.plan.ctx_tokens }} ·
          VRAM ≈ {{ status.llm.plan.vram_used_gb.toFixed(1) }} / {{ status.llm.plan.vram_budget_gb.toFixed(1) }} Go
          <div v-for="n in status.llm.plan.notes" :key="n">{{ n }}</div>
        </div>
        <div v-if="c.key === 'tts'" class="muted small">
          Moteur de voix : {{ rtLabel }}
          <button v-if="rt === 'absent' || rt === 'error' || rt === 'outdated'" class="primary" @click="installRuntime">{{ rt === 'outdated' ? 'Mettre à jour le moteur de voix' : 'Installer le moteur de voix (≈ 3 Go)' }}</button>
          <div v-if="status.tts_runtime?.error" class="err">{{ status.tts_runtime.error }}</div>
          <div v-if="rt === 'installing'">Installation en cours… (voir les journaux)</div>
        </div>
        <div class="row">
          <button v-if="stateOf(c.key) !== 'stopped'" @click="unload(c.key)">Décharger</button>
          <button @click="toggleLogs(c.key)">{{ logsFor === c.key ? 'Masquer' : 'Journaux' }}</button>
        </div>
      </div>
    </section>
    <section class="cards">
      <div class="card">
        <div class="title">Jeton Hugging Face <span :class="['pill', status.hf_token ? 'ready' : 'stopped']">{{ status.hf_token ? 'enregistré' : 'absent' }}</span></div>
        <div class="muted small">
          Nécessaire pour télécharger les dépôts protégés ou à accès limité. Créez un jeton de type « Read » sur
          <a href="https://huggingface.co/settings/tokens" target="_blank" rel="noopener">huggingface.co/settings/tokens</a>, puis acceptez la licence de
          chaque modèle demandé sur sa page. Le jeton reste sur cet ordinateur et n'est jamais réaffiché.
        </div>
        <form class="row" @submit.prevent="saveToken">
          <input v-model="hfToken" type="password" autocomplete="off" spellcheck="false" placeholder="hf_…" />
          <button class="primary" :disabled="!hfToken.trim()">Enregistrer</button>
          <button v-if="status.hf_token" type="button" @click="clearToken">Effacer</button>
        </form>
      </div>
    </section>
    <pre v-if="logsFor" class="logs">{{ logLines.join('\n') || '(vide)' }}</pre>
    <p v-if="message" class="msg">{{ message }}</p>

    <template v-for="c in comps" :key="'t' + c.key">
      <h3>{{ c.label }} — catalogue</h3>
      <p v-if="c.key === 'llm' && !groupsFor('llm').length" class="muted small">
        Aucun modèle de texte dans le catalogue livré : ajoutez-en un dans <code>catalog.json</code> (dossier de données), par exemple
        <code>[{"id": "mon-modele", "kind": "llm", "name": "Mon modèle", "repo": "auteur/depot-GGUF", "pattern": "Q4_K_M", "size_gb": 5.0}]</code>,
        puis rechargez cette page. Sans modèle de texte, l'assistant ne peut pas répondre.
      </p>
      <template v-for="g in groupsFor(c.key)" :key="g.label">
        <h4 v-if="g.label" class="group">{{ g.label }}</h4>
        <table>
          <tbody>
            <tr v-for="m in g.items" :key="m.entry.id">
              <td class="name">
                <b>{{ m.entry.name }}</b>
                <span v-if="m.vision?.supported" :class="['tag', m.vision.installed ? 'on' : '']" :title="m.vision.installed ? 'Projecteur de vision téléchargé' : 'Ce modèle peut voir les images une fois sa vision téléchargée'">
                  👁 {{ m.vision.installed ? 'vision' : 'vision à ajouter' }}</span>
                <div class="muted small">{{ m.entry.description }}</div>
                <div class="muted small">{{ m.entry.repo }}</div>
              </td>
              <td class="size">
                {{ m.size_bytes ? gb(m.size_bytes) : m.entry.size_gb ? '≈ ' + m.entry.size_gb + ' Go' : '' }}
                <div v-if="c.key === 'llm' && m.entry.size_gb" class="muted small">VRAM ≈ {{ (m.entry.size_gb + 1.2).toFixed(1) }} Go</div>
              </td>
              <td class="act">
                <template v-if="busy(m)">
                  <div class="bar"><div :style="{ width: pct(m.download.downloaded, m.download.total) + '%' }" /></div>
                  <span class="small">{{ m.download.status === 'listing' ? 'recherche du fichier…' : m.download.status === 'verifying' ? 'vérification SHA-256…' : gb(m.download.downloaded) + ' / ' + gb(m.download.total) }}</span>
                  <button @click="cancel(m)">Annuler</button>
                </template>
                <template v-else-if="m.installed">
                  <!-- Modèle qui peut voir les images : avec ou sans sa vision (≈ 1 Go de VRAM en plus) -->
                  <template v-if="m.vision?.installed">
                    <button class="primary" :disabled="isLoaded(m) && !loadedVision" @click="load(m, false)">{{ isLoaded(m) && !loadedVision ? 'Chargé' : 'Charger' }}</button>
                    <button :disabled="isLoaded(m) && loadedVision" title="Le modèle voit les images (≈ 1 Go de VRAM en plus)" @click="load(m, true)">
                      {{ isLoaded(m) && loadedVision ? 'Chargé avec la vision' : 'Charger avec la vision' }}</button>
                  </template>
                  <button v-else class="primary" :disabled="isLoaded(m)" @click="load(m)">{{ isLoaded(m) ? 'Chargé' : 'Charger' }}</button>
                  <button v-if="m.vision?.supported && !m.vision.installed" title="Télécharge le projecteur qui permet au modèle de voir les images" @click="addVision(m)">Ajouter la vision (≈ 1 Go)</button>
                  <button @click="remove(m)">Supprimer</button>
                </template>
                <template v-else>
                  <button class="primary" @click="download(m)">Télécharger</button>
                  <span v-if="m.download?.status === 'error'" class="err small">{{ m.download.error }}</span>
                  <span v-else-if="m.download?.status === 'cancelled'" class="muted small">annulé (reprise possible)</span>
                </template>
              </td>
            </tr>
          </tbody>
        </table>
      </template>
    </template>
    <p class="muted small">
      Les fichiers viennent de Hugging Face (jeton à enregistrer plus haut pour les dépôts protégés). Les téléchargements reprennent là où ils
      se sont arrêtés et sont vérifiés par SHA-256. Ajoutez vos propres modèles dans <code>catalog.json</code> (dossier de données).
    </p>
  </div>
</template>

<script setup>
import { ref, computed, onMounted, onBeforeUnmount } from 'vue'
import axios from 'axios'

const MB = 1024 * 1024
const comps = [
  { key: 'llm', label: 'IA (texte)' },
  { key: 'stt', label: 'Micro (reconnaissance vocale)' },
  { key: 'tts', label: 'Voix de l\'IA' },
]
const status = ref({})
const models = ref([])
const message = ref('')
const logsFor = ref('')
const logLines = ref([])
let timer = null

const headers = () => {
  const t = localStorage.getItem('live_admin_token')
  return t ? { Authorization: `Bearer ${t}` } : {}
}
const gb = (b) => (b / 1024 ** 3).toFixed(b < 1024 ** 3 ? 2 : 1) + ' Go'
const pct = (a, t) => (t ? Math.min(100, Math.round((a / t) * 100)) : 0)
const byKind = (k) => models.value.filter((m) => m.entry.kind === k)
// Les modèles de texte sont groupés par taille de fichier : on voit tout de suite les plus légers.
// (VRAM ≈ fichier + 1,2 Go de cache et tampons ; la voix et le micro s'y ajoutent.)
const SIZE_GROUPS = [
  { max: 3.5, label: 'Mini — fichier ≤ 3,5 Go (≈ 4,7 Go de VRAM au plus)' },
  { max: 6, label: 'Léger — fichier 3,5 à 6 Go (≈ 5 à 7 Go de VRAM ; laisse de la place à la voix et au micro sur 12 Go)' },
  { max: 10, label: 'Standard — fichier 6 à 10 Go (≈ 7 à 11 Go de VRAM)' },
  { max: Infinity, label: 'Grand — fichier de 10 Go et plus (une partie reste en RAM sur une carte de 12 Go)' },
]
function groupsFor(kind) {
  const items = [...byKind(kind)].sort((a, b) => (a.entry.size_gb || 0) - (b.entry.size_gb || 0))
  if (kind !== 'llm') return [{ label: '', items }]
  // Sans taille déclarée (entrée ajoutée dans catalog.json), un modèle ne doit pas passer pour « mini ».
  const known = items.filter((m) => m.entry.size_gb > 0)
  const groups = SIZE_GROUPS.map((g, i) => ({
    label: g.label,
    items: known.filter((m) => m.entry.size_gb < g.max && m.entry.size_gb >= (SIZE_GROUPS[i - 1]?.max ?? 0)),
  }))
  groups.push({ label: 'Taille inconnue (renseigne size_gb dans catalog.json)', items: items.filter((m) => !m.entry.size_gb) })
  return groups.filter((g) => g.items.length)
}
const stateOf = (k) => status.value[k]?.state || 'stopped'
const stateLabel = (s) => ({ stopped: 'arrêté', loading: 'chargement…', ready: 'prêt', error: 'erreur' })[s] || s
const modelName = (id) => models.value.find((m) => m.entry.id === id)?.entry.name
const rt = computed(() => status.value.tts_runtime?.state || 'absent')
const rtLabel = computed(() => ({ absent: 'non installé', outdated: 'mise à jour requise', installing: 'installation…', ready: 'installé', error: 'erreur' })[rt.value] || rt.value)
const installRuntime = () => { logsFor.value = 'tts'; return act(() => axios.post('/api/live/engine/tts-runtime/install', {}, { headers: headers() })) }
const hfToken = ref('')
const saveToken = async () => {
  await act(() => axios.put('/api/live/hf-token', { token: hfToken.value }, { headers: headers() }))
  if (!message.value) hfToken.value = ''
}
const clearToken = () => act(() => axios.delete('/api/live/hf-token', { headers: headers() }))
const busy = (m) => ['listing', 'downloading', 'verifying'].includes(m.download?.status)
const isLoaded = (m) => status.value[m.entry.kind]?.model === m.entry.id && stateOf(m.entry.kind) !== 'stopped'

async function refresh() {
  try {
    const [s, m] = await Promise.all([axios.get('/api/live/engine/status'), axios.get('/api/live/models')])
    status.value = s.data
    models.value = m.data
    if (logsFor.value) logLines.value = (await axios.get(`/api/live/engine/logs/${logsFor.value}`)).data.lines
  } catch { message.value = 'Moteur live injoignable (live-engine démarré ?)' }
}

async function act(fn) {
  message.value = ''
  try { await fn() } catch (e) { message.value = e.response?.data?.message || e.message }
  await refresh()
}

const download = (m) => act(() => axios.post(`/api/live/models/${m.entry.id}/download`, {}, { headers: headers() }))
const cancel = (m) => act(() => axios.post(`/api/live/models/${m.entry.id}/cancel`, {}, { headers: headers() }))
const load = (m, vision) => act(() => axios.post('/api/live/engine/load', { id: m.entry.id, vision }, { headers: headers() }))
const addVision = (m) => act(() => axios.post(`/api/live/models/${m.entry.id}/vision`, {}, { headers: headers() }))
const loadedVision = computed(() => !!status.value.llm?.vision)
const unload = (kind) => act(() => axios.post('/api/live/engine/unload', { kind }, { headers: headers() }))
const remove = (m) => confirm(`Supprimer « ${m.entry.name} » du disque ?`) &&
  act(() => axios.delete(`/api/live/models/${m.entry.id}`, { headers: headers() }))
const toggleLogs = (k) => { logsFor.value = logsFor.value === k ? '' : k; refresh() }

onMounted(() => { refresh(); timer = setInterval(refresh, 1500) })
onBeforeUnmount(() => clearInterval(timer))
</script>

<style scoped>
.wrap { padding: 1.5rem 2rem; overflow: auto; max-width: 1100px; }
.muted { color: #999; } .small { font-size: .82rem; } .err { color: #ff7b7b; } .warn { color: #e6b450; margin-top: .5rem; }
.gpu { background: #1b1b1b; border: 1px solid #333; border-radius: 10px; padding: .8rem 1rem; margin-bottom: 1rem; }
.bar { height: 8px; background: #2a2a2a; border-radius: 4px; overflow: hidden; margin: .4rem 0; min-width: 160px; }
.bar > div { height: 100%; background: #4a6cf7; transition: width .4s; }
.cards { display: grid; grid-template-columns: repeat(auto-fit, minmax(280px, 1fr)); gap: 1rem; }
.card { background: #1b1b1b; border: 1px solid #333; border-radius: 10px; padding: .8rem 1rem; display: flex; flex-direction: column; gap: .35rem; }
.title { font-weight: 600; display: flex; justify-content: space-between; gap: .5rem; }
.pill { font-size: .75rem; padding: .1rem .55rem; border-radius: 999px; background: #444; font-weight: 400; }
.pill.ready { background: #1f5a34; } .pill.loading { background: #6b5a1f; } .pill.error { background: #5a2a2a; }
.row { display: flex; gap: .5rem; margin-top: .3rem; }
.logs { background: #0d0d0d; border: 1px solid #333; border-radius: 8px; padding: .6rem; max-height: 260px; overflow: auto; font-size: .78rem; white-space: pre-wrap; }
table { width: 100%; border-collapse: collapse; }
.group { margin: 1rem 0 .2rem; color: #9db4ff; font-weight: 600; font-size: .95rem; }
td { padding: .55rem .4rem; border-bottom: 1px solid #2a2a2a; vertical-align: middle; }
.name { width: 55%; } .size { white-space: nowrap; color: #bbb; } .act { display: flex; gap: .5rem; align-items: center; flex-wrap: wrap; }
.msg { color: #ff9b7b; }
button { cursor: pointer; padding: .4rem .8rem; border-radius: 6px; border: 1px solid #555; background: #2a2a2a; color: #eee; }
button:disabled { opacity: .5; cursor: default; }
.primary { background: #4a6cf7; border-color: #4a6cf7; }
.tag { margin-left: .5rem; font-size: .72rem; padding: .05rem .45rem; border-radius: 999px; background: #3a3a3a; color: #bbb; white-space: nowrap; }
.tag.on { background: #1f4a5a; color: #cfefff; }
code { background: #222; padding: 0 .3rem; border-radius: 4px; }
</style>
