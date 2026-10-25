<template>
  <div :class="['live', started && 'split', started && main && 'with-stage']">
    <div v-if="confirmation" class="confirm" role="alertdialog" aria-label="Confirmation" @keydown.esc="answer(false)">
      <div class="box">
        <p>{{ confirmation.text }}</p>
        <div class="row">
          <button type="button" class="primary" @click="answer(true)">Confirmer</button>
          <button type="button" @click="answer(false)">Annuler</button>
        </div>
      </div>
    </div>
    <!-- Sources : la capture d'écran, le schéma ou la vidéo que l'assistant affiche. Le panneau n'apparaît (à droite) que lorsqu'il en montre une. -->
    <div v-if="main" class="stage" ref="stageEl">
      <Transition name="fade">
        <!-- Une seule lecture, et pas pendant que l'assistant parle : la vidéo démarre quand sa voix s'est tue (voir `quiet`) -->
        <video v-if="main && main.kind === 'video'" :key="main.id" ref="mainVideo" :src="main.url" class="layer main" preload="auto"
               controls controlslist="nofullscreen noremoteplayback" playsinline :muted="!mediaSound"
               @canplay="playIfQuiet" @volumechange="onMediaVolume" />
        <img v-else-if="main" :key="main.id" :src="main.url" class="layer main" :alt="caption || 'Source affichée par l\'assistant'" />
      </Transition>

      <p v-if="caption" class="caption">{{ caption }}</p>

      <button type="button" class="fs" :title="fullscreen ? 'Quitter le plein écran' : 'Plein écran'"
              @click="toggleFullscreen">{{ fullscreen ? '⤡' : '⤢' }}</button>
    </div>

    <!-- Choix du profil d'assistant -->
    <section v-if="!started" class="panel start">
      <h2>Assistant de support</h2>
      <label>Profil d'assistant
        <select v-model="personaId">
          <option v-for="p in personas" :key="p.id" :value="p.id">{{ p.name }}</option>
        </select>
      </label>
      <p v-if="selected" class="muted">{{ selected.summary }}</p>
      <input v-model="userName" class="name" placeholder="Votre prénom (comment l'assistant vous appelle) — facultatif" maxlength="60" />

      <p v-if="savedConv" class="saved">
        💾 Conversation sauvegardée ({{ savedConv.messages }} messages, {{ fmtDate(savedConv.updated) }}) : elle sera reprise.
        <button type="button" class="small" :disabled="!connected" @click="resetSaved">Repartir de zéro</button>
      </p>
      <p v-if="connected && !hello.llm" class="error">
        L'IA n'est pas chargée. <router-link to="/admin/live/models">Télécharger / charger un modèle</router-link>
      </p>
      <p class="status">
        <span :class="['pill', hello.llm && 'ok']">IA</span>
        <span :class="['pill', hello.stt && 'ok']" :title="micProblem || 'Reconnaissance vocale prête'">Micro</span>
        <span :class="['pill', voice.ready && 'ok']" :title="voice.problem || 'La voix de l\'assistant répond'">Voix</span>
        <span v-if="!connected" class="muted"> connexion…</span>
      </p>
      <p v-if="connected && micProblem" class="warn">
        🎤 {{ micProblem }} <router-link to="/admin/live/models">Ouvrir Modèles IA</router-link>
      </p>
      <p v-if="connected && voiceProblem" class="warn">
        🔇 {{ voiceProblem }} <router-link to="/admin/live/models">Ouvrir Modèles IA</router-link>
      </p>
      <p v-else-if="voiceNote" class="muted">{{ voiceNote }}</p>
      <div class="actions">
        <button class="primary" :disabled="!personaId || !connected" @click="start()">Commencer</button>
        <button type="button" :disabled="!connected || testing" @click="testVoice"
                title="Fait lire une phrase par la voix du profil choisi">🔊 Tester la voix</button>
      </div>
      <p class="muted">
        <router-link to="/admin/live/personas">{{ personas.length ? 'Créer ou modifier un profil d\'assistant' : 'Créer un profil d\'assistant' }}</router-link>
      </p>
    </section>

    <!-- Conversation -->
    <section v-else class="chat">
      <div class="head">
        <div class="toolbar">
          <strong>{{ persona?.name }}</strong>
          <span v-if="speaking" class="pill ok">🔊 parle…</span>
          <button type="button" class="small" :aria-expanded="showSettings" @click="showSettings = !showSettings">⚙ Réglages</button>
          <button type="button" class="small" @click="restart">Changer</button>
          <template v-if="showSettings">
          <label class="inline"><input type="checkbox" v-model="voiceOn" @change="pushSettings" /> Voix de l'assistant
            <span :class="['dot', voice.ready && 'ok']" :title="voice.ready ? 'Voix prête' : voice.problem" />
          </label>
          <button type="button" class="small" :disabled="testing" @click="testVoice" title="Fait lire une phrase d'essai">🔊 Tester</button>
          <label class="inline"><input type="checkbox" v-model="mediaSound" /> Son des vidéos</label>
          <button type="button" class="small" title="Efface la mémoire de cette conversation et recommence" @click="newConversation">↺ Nouvelle conversation</button>
          </template>
        </div>
      </div>

      <div class="log" ref="logEl" aria-live="polite">
        <div v-for="(m, i) in messages" :key="i" :class="['msg', m.role]">
          <span v-html="render(m.text)" />
          <button v-if="m.role === 'assistant' && m.text.trim()" type="button" class="listen" :disabled="!voice.enabled"
                  title="Écouter ce message" @click="listen(m)">🔊</button>
        </div>
      </div>

      <div class="foot">
        <div v-if="replies.length" class="replies">
          <button v-for="(r, n) in replies" :key="n" type="button" :title="r" @click="sendText(r)">{{ r }}</button>
        </div>

        <form class="input" @submit.prevent="sendText(draft)">
          <button type="button" :class="['mic', micState === 'recording' && 'rec']" :disabled="micBusy || (micState === 'idle' && !hello.stt)"
                  :title="micTitle" @click="toggleMic">{{ micState === 'recording' ? '■' : micBusy ? '…' : '🎤' }}</button>
          <span v-if="micState === 'recording'" class="meter" :title="'Niveau du micro'">
            <i :style="{ width: Math.round(micLevel * 100) + '%' }" /><b>{{ recSeconds.toFixed(0) }} s</b>
          </span>
          <input v-model="draft" placeholder="Décrivez votre problème…" autocomplete="off" aria-label="Votre message" />
          <button class="primary" type="submit" :disabled="!draft.trim() || !connected">Envoyer</button>
          <button type="button" v-if="busy || speaking" @click="interrupt" title="Couper la parole">⏹</button>
        </form>
        <p v-if="micState === 'sending'" class="muted">Transcription en cours…</p>
        <p v-if="!connected" class="warn">Connexion au moteur interrompue… reconnexion en cours.</p>
        <p v-if="micNote" class="warn">🎤 {{ micNote }}</p>
        <p v-if="!hello.stt && micProblem" class="warn">🎤 {{ micProblem }} <router-link to="/admin/live/models">Modèles IA</router-link></p>
        <p v-if="error" class="error">{{ error }}</p>
        <p v-if="voiceProblem" class="warn">
          🔇 {{ voiceProblem }} <router-link to="/admin/live/models">Ouvrir Modèles IA</router-link>
        </p>
      </div>
    </section>
  </div>
</template>

<script setup>
import { ref, computed, watch, onMounted, onBeforeUnmount, nextTick } from 'vue'
import axios from 'axios'
import { Recorder } from '../utils/audio.js'
import { Speaker } from '../utils/speaker.js'
import { store } from '../utils/storage.js'

const personas = ref([])
const personaId = ref('')
const persona = ref(null)
const selected = computed(() => personas.value.find((p) => p.id === personaId.value))
const started = ref(false)
const connected = ref(false)
const hello = ref({})
const voice = ref({ enabled: true, ready: false, problem: null })
const mic = ref({ enabled: true, ready: false, problem: null })
const voiceError = ref('')   // dernière erreur de synthèse renvoyée par le moteur : reste affichée jusqu'au prochain son entendu
const playError = ref('')    // lecture refusée par le navigateur / audio illisible
const voiceNote = ref('')
const testing = ref(false)
const speaking = ref(false)
const voiceOn = ref(true)
const mediaSound = ref(true) // le son des vidéos affichées est lu (décochable)
const stageEl = ref(null)
const fullscreen = ref(false)
const messages = ref([])
const replies = ref([])
const draft = ref('')
const busy = ref(false)
const micState = ref('idle') // idle | starting | recording | stopping | sending
const micLevel = ref(0)
const recSeconds = ref(0)
const micNote = ref('')
const error = ref('')
const main = ref(null) // la source affichée à droite
const logEl = ref(null)
const conversations = ref([]) // conversations sauvegardées par le moteur (reprises automatiquement)
const savedConv = computed(() => conversations.value.find((c) => c.key === personaId.value && c.messages > 0))
const showSettings = ref(false) // réglages repliés
const userName = ref('')

// Ce que la source affichée représente : sa description, à défaut son titre.
const caption = computed(() => {
  const m = main.value
  if (!m) return ''
  const t = (m.description || m.title || '').trim()
  return t.length > 160 ? `${t.slice(0, 157).trimEnd()}…` : t
})

// Raisons affichées quand l'assistant ne parle pas ou que le micro ne marche pas.
// Une erreur réelle (synthèse, lecture) s'affiche toujours, même voix décochée : c'est le cas de « Tester » et de 🔊.
const voiceProblem = computed(() =>
  voiceError.value || playError.value || (voiceOn.value && !voice.value.ready ? voice.value.problem || '' : ''))
const micBusy = computed(() => ['starting', 'stopping', 'sending'].includes(micState.value))
const micProblem = computed(() => (mic.value.ready ? '' : mic.value.problem || ''))
const micTitle = computed(() => {
  if (!hello.value.stt) return micProblem.value || 'Micro indisponible'
  return micState.value === 'recording' ? 'Cliquez pour arrêter et envoyer' : 'Parler (clic pour démarrer, puis clic pour envoyer)'
})

let ws = null
let currentGen = 0 // génération dont on affiche le texte (-1 : aucune, ses jetons seraient ceux d'une génération interrompue)
let audioGen = 0   // génération (ou lecture « réécouter ») dont on joue le son
let bubble = -1    // indice de la bulle de la génération en cours
let pendingAudio = null
let hadConnection = false
let levelTimer = null
let poll = null
let recStart = 0
const recorder = new Recorder()
const speaker = new Speaker()
speaker.onStart = () => {
  voiceError.value = ''; playError.value = '' // un son vient d'être joué : l'ancienne erreur n'a plus lieu d'être
  if (testing.value) voiceNote.value = "Lecture de la phrase d'essai : si vous l'entendez, la voix fonctionne."
}
speaker.onChange = (s) => {
  speaking.value = s
  if (!s) { testing.value = false; if (voiceNote.value.startsWith('Lecture')) voiceNote.value = '' }
}
speaker.onError = (m) => { playError.value = m; testing.value = false }

function connect() {
  const proto = location.protocol === 'https:' ? 'wss' : 'ws'
  ws = new WebSocket(`${proto}://${location.host}/api/live/ws`)
  ws.binaryType = 'arraybuffer'
  ws.onopen = () => {
    connected.value = true
    // Le serveur a perdu la conversation avec la connexion : mieux vaut le dire que de laisser écrire dans le vide.
    if (hadConnection && started.value) error.value = 'La connexion au moteur a été interrompue : la conversation est à recommencer (bouton « Changer »).'
    hadConnection = true
  }
  ws.onclose = () => {
    connected.value = false
    busy.value = false
    if (micState.value === 'sending') micState.value = 'idle' // la transcription ne viendra pas
    setTimeout(() => ws && connect(), 2000)
  }
  ws.onmessage = onMessage
}

function applyStatus(m) {
  if (m.voice) voice.value = m.voice
  if (m.mic) mic.value = m.mic
}

function onMessage(e) {
  if (typeof e.data !== 'string') {
    // L'essai de la voix (test) ne dépend d'aucune génération en cours.
    if (pendingAudio && (pendingAudio.test || pendingAudio.gen === audioGen)) speaker.enqueue(e.data, { gen: pendingAudio.gen, seq: pendingAudio.seq })
    pendingAudio = null
    return
  }
  const m = JSON.parse(e.data)
  switch (m.type) {
    case 'hello': hello.value = m; applyStatus(m); break
    case 'status': hello.value = { ...hello.value, llm: m.llm, stt: m.stt, tts: m.tts }; applyStatus(m); break
    case 'tts_error': voiceError.value = m.message; testing.value = false; voiceNote.value = ''; break
    case 'persona': persona.value = m.persona; break
    case 'restored': // conversation sauvegardée reprise : on la réaffiche, l'assistant ne rouvre pas la conversation
      messages.value = m.messages.map((x) => ({ role: x.role, text: x.text }))
      replies.value = []; bubble = -1
      if (!userName.value && m.user_name) userName.value = m.user_name
      scroll()
      break
    case 'reset_done': loadConversations(); break
    case 'transcript':
      micState.value = 'idle'
      if (m.text) { pushMessage('user', m.text); busy.value = true }
      else micNote.value = "Je n'ai rien compris : parlez plus près du micro, un peu plus fort, puis réessayez."
      break
    case 'gen_start': // le moteur annonce la génération AVANT ses jetons
      currentGen = audioGen = m.gen
      busy.value = true
      messages.value.push({ role: 'assistant', text: '' })
      bubble = messages.value.length - 1
      break
    case 'token':
      if (m.gen !== currentGen || !messages.value[bubble]) break // jeton d'une génération interrompue
      messages.value[bubble].text += m.text
      scroll()
      break
    case 'media': main.value = m.item; break
    case 'replies': replies.value = m.items; scroll(); break
    case 'speak_start': audioGen = m.gen; break // « réécouter » : l'audio qui suit appartient à cette lecture
    case 'audio': pendingAudio = m; break
    case 'done':
      if (m.gen === currentGen) { busy.value = false; dropEmptyBubble() }
      break
    case 'error': error.value = m.message; busy.value = false; micState.value = 'idle'; dropEmptyBubble(); break
  }
}

/** Une génération sans texte (erreur du LLM, seulement des directives) ne laisse pas de bulle vide. */
function dropEmptyBubble() {
  const m = messages.value[bubble]
  if (m && m.role === 'assistant' && !m.text.trim()) messages.value.splice(bubble, 1)
  bubble = -1
}

function pushMessage(role, text) {
  messages.value.push({ role, text })
  scroll()
}

/** Texte du message en HTML : tout est échappé d'abord, puis **gras**, *italique* et `code` sont reconnus. */
function render(t) {
  const esc = t.replace(/[&<>]/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;' })[c])
  return esc
    .replace(/\*\*([^*]+)\*\*/g, '<strong>$1</strong>')
    .replace(/\*([^*]+)\*/g, '<em>$1</em>')
    .replace(/`([^`]+)`/g, '<code>$1</code>')
}

function scroll() { nextTick(() => logEl.value && (logEl.value.scrollTop = logEl.value.scrollHeight)) }

function stopAudio() { speaker.stop() }

function sendJson(o) { ws && ws.readyState === 1 && ws.send(JSON.stringify(o)) }

async function testVoice() {
  testing.value = true
  voiceError.value = ''; playError.value = ''
  voiceNote.value = "Synthèse de la phrase d'essai… (la première fois, quelques secondes)"
  await speaker.unlock() // ce clic débloque le son avant que l'audio n'arrive
  sendJson({ type: 'voice_test', persona: personaId.value })
  setTimeout(() => { testing.value = false }, 20000)
}

/** Réécoute un message déjà écrit : le moteur le lit avec la voix du profil. */
async function listen(m) {
  stopAudio()
  audioGen = -2 // le son de ce qui parlait est abandonné ; celui de la lecture demandée arrive après « speak_start »
  voiceError.value = ''; playError.value = ''
  await speaker.unlock()
  sendJson({ type: 'speak', text: m.text, persona: persona.value?.id || personaId.value })
}

async function start(resume = true) {
  messages.value = []; replies.value = []; main.value = null; error.value = ''
  voiceError.value = ''; playError.value = ''; voiceNote.value = ''; micNote.value = ''
  currentGen = audioGen = 0
  bubble = -1
  store.set('repere.live.userName', userName.value.trim())
  started.value = true
  await speaker.unlock() // « Commencer » est un clic : le son est autorisé pour toute la conversation
  sendJson({ type: 'start', persona: personaId.value, tts: voiceOn.value, user_name: userName.value.trim(), resume })
}

async function loadConversations() {
  try { conversations.value = (await axios.get('/api/live/conversations')).data } catch { conversations.value = [] }
}

const fmtDate = (t) => new Date(t * 1000).toLocaleString('fr-FR', { dateStyle: 'short', timeStyle: 'short' })

/** Depuis l'écran de départ : supprime la sauvegarde de ce profil. */
function resetSaved() {
  ask('Supprimer la conversation sauvegardée et repartir de zéro ?', () => sendJson({ type: 'reset', persona: personaId.value }))
}

// Confirmation dans la page : les boîtes natives (confirm) peuvent, dans Electron, laisser les champs de texte sans clavier.
const confirmation = ref(null) // { text, action }
function ask(text, action) { confirmation.value = { text, action } }
function answer(yes) {
  const c = confirmation.value
  confirmation.value = null
  if (yes && c) c.action()
}

/** En conversation : efface la mémoire et recommence (l'assistant rouvre la conversation). */
function newConversation() {
  ask('Effacer cette conversation et recommencer à zéro ?', () => { interrupt(); start(false) })
}

function restart() { loadConversations(); interrupt(); recorder.cancel(); micState.value = 'idle'; clearInterval(levelTimer); micLevel.value = 0; started.value = false }

function pushSettings() { sendJson({ type: 'settings', tts: voiceOn.value }) }

function sendText(text) {
  text = (text || '').trim()
  if (!text) return
  if (!ws || ws.readyState !== 1) { error.value = 'Connexion au moteur interrompue : réessayez dans un instant (votre texte est conservé).'; return }
  stopAudio()
  speaker.unlock()
  currentGen = audioGen = -1 // texte et son de la génération précédente ignorés jusqu'à « gen_start » de la nouvelle
  replies.value = []; error.value = ''; micNote.value = ''
  pushMessage('user', text)
  busy.value = true // l'assistant va répondre : une vidéo affichée entre-temps attend la fin de sa réponse
  draft.value = ''
  sendJson({ type: 'user', text })
}

function interrupt() { stopAudio(); currentGen = audioGen = -1; busy.value = false; sendJson({ type: 'interrupt' }) }

async function toggleMic() {
  error.value = ''; micNote.value = ''
  if (micState.value === 'idle') {
    stopAudio(); speaker.unlock(); sendJson({ type: 'interrupt' }); currentGen = audioGen = -1
    micState.value = 'starting' // la demande d'accès au micro peut durer : les clics suivants sont ignorés
    try {
      await recorder.start()
    } catch (e) { error.value = e.message; micState.value = 'idle'; return }
    if (micState.value !== 'starting') { recorder.cancel(); return } // « Changer » pressé entre-temps
    micState.value = 'recording'
    recStart = performance.now(); recSeconds.value = 0
    levelTimer = setInterval(() => { micLevel.value = recorder.level(); recSeconds.value = (performance.now() - recStart) / 1000 }, 80)
  } else if (micState.value === 'recording') {
    micState.value = 'stopping' // décodage de l'enregistrement en cours
    clearInterval(levelTimer); micLevel.value = 0
    try {
      const r = await recorder.stop()
      if (micState.value !== 'stopping') return // « Changer » pressé entre-temps
      if (!r) { micState.value = 'idle'; return }
      if (r.seconds < 0.4) { micState.value = 'idle'; micNote.value = 'Enregistrement trop court : parlez, puis cliquez de nouveau sur le micro.'; return }
      if (r.peak < 0.01) {
        micState.value = 'idle'
        micNote.value = "Aucun son capté : le micro est coupé, ou ce n'est pas le bon périphérique (réglages sonores d'Ubuntu → Entrée)."
        return
      }
      if (!ws || ws.readyState !== 1) { micState.value = 'idle'; error.value = 'Connexion au moteur interrompue : réessayez dans un instant.'; return }
      replies.value = []
      micState.value = 'sending'
      ws.send(r.wav)
    } catch (e) { micState.value = 'idle'; error.value = e.message }
  }
}

function onFullscreenChange() { fullscreen.value = !!document.fullscreenElement && document.fullscreenElement === stageEl.value }

function toggleFullscreen() {
  if (document.fullscreenElement) document.exitFullscreen?.()
  else stageEl.value?.requestFullscreen?.().catch(() => { /* refusé par le navigateur : sans effet */ })
}

/** Le curseur de volume de la vidéo reflète la case « Son des vidéos ». */
function onMediaVolume(e) { mediaSound.value = !e.target.muted && e.target.volume > 0 }

// Une vidéo affichée attend que l'assistant ait fini d'écrire ET que sa voix se soit tue (un court silence entre deux phrases ne compte pas).
const mainVideo = ref(null)
const quiet = ref(true)
let quietTimer = null
watch([busy, speaking], () => {
  clearTimeout(quietTimer)
  if (busy.value || speaking.value) quiet.value = false
  else quietTimer = setTimeout(() => { quiet.value = true }, 800)
})
watch(quiet, (q) => { if (q) playMain() })
function playMain() {
  const v = mainVideo.value
  if (v && v.paused && !v.ended) v.play().catch(() => { /* lecture refusée : les commandes de la vidéo restent là */ })
}
function playIfQuiet() { if (quiet.value) playMain() }

onMounted(async () => {
  document.addEventListener('fullscreenchange', onFullscreenChange)
  loadConversations()
  userName.value = store.get('repere.live.userName') || ''
  try {
    personas.value = (await axios.get('/api/live/personas')).data
    personaId.value = personas.value[0]?.id || ''
  } catch { error.value = 'Moteur live injoignable (live-engine démarré ?)' }
  connect()
  // Rafraîchit les pastilles (IA / micro / voix) : on peut charger un modèle dans un autre onglet pendant ce temps.
  poll = setInterval(() => sendJson({ type: 'status' }), 5000)
})
onBeforeUnmount(() => {
  clearTimeout(quietTimer)
  document.removeEventListener('fullscreenchange', onFullscreenChange)
  clearInterval(poll); clearInterval(levelTimer)
  const s = ws; ws = null; s && s.close()
  recorder.cancel(); speaker.close()
})
</script>

<style scoped>
.live { position: relative; flex: 1; min-height: 0; display: flex; flex-direction: column; justify-content: flex-end; overflow: hidden; background: #000; }
.stage { position: relative; flex: 1 1 0; min-width: 0; min-height: 0; background: #000; }
.layer { position: absolute; inset: 0; width: 100%; height: 100%; object-fit: contain; background: #000; } /* jamais rognées : les proportions des images et des vidéos sont respectées */
.fade-enter-active, .fade-leave-active { transition: opacity .8s; }
.fade-enter-from, .fade-leave-to { opacity: 0; }
.panel { position: relative; margin: auto; width: min(440px, 92%); max-height: 94%; overflow-y: auto; padding: 1.5rem; background: rgba(20,20,20,.88); border: 1px solid #333; border-radius: 12px; display: flex; flex-direction: column; gap: .9rem; }
.chat { position: relative; display: flex; flex-direction: column; gap: .6rem; min-height: 0; box-sizing: border-box; overflow: hidden; }
.head { flex: none; display: flex; flex-direction: column; gap: .6rem; align-items: flex-start; }
.toolbar { flex: none; width: 100%; display: flex; gap: .4rem .8rem; align-items: center; flex-wrap: wrap; font-size: .85rem; }
.inline { display: flex; align-items: center; gap: .4rem; }
.log { flex: 1 1 auto; min-height: 4rem; overflow-y: auto; display: flex; flex-direction: column; gap: .5rem; padding-right: .3rem; }
.foot { flex: none; display: flex; flex-direction: column; gap: .5rem; }
.foot p { margin: 0; }
.msg { max-width: 85%; padding: .55rem .85rem; border-radius: 14px; line-height: 1.4; background: rgba(30,30,30,.82); backdrop-filter: blur(6px); white-space: pre-line; }
.msg.user { align-self: flex-end; background: rgba(60,80,140,.82); }
.msg :deep(em) { color: #b9b9c9; }
.msg :deep(code) { background: rgba(255,255,255,.1); padding: 0 .3rem; border-radius: 4px; }
.replies { display: flex; flex-wrap: wrap; gap: .4rem; max-height: 6.4rem; overflow-y: auto; }
.replies button { text-align: left; max-width: 100%; display: -webkit-box; -webkit-line-clamp: 2; -webkit-box-orient: vertical; overflow: hidden; border-radius: 14px; background: rgba(40,40,40,.85); border: 1px solid #555; color: #eee; padding: .4rem .9rem; cursor: pointer; }
.replies button:hover { background: #3a3a3a; }
.input { display: flex; gap: .5rem; }
.input input { flex: 1; padding: .6rem .8rem; border-radius: 8px; border: 1px solid #444; background: rgba(20,20,20,.9); color: #eee; }
button { cursor: pointer; padding: .5rem .9rem; border-radius: 8px; border: 1px solid #555; background: #2a2a2a; color: #eee; }
button:disabled { opacity: .5; cursor: not-allowed; }
.primary { background: #4a6cf7; border-color: #4a6cf7; }
.mic.rec { background: #c0392b; border-color: #c0392b; animation: pulse 1s infinite; }
@keyframes pulse { 50% { opacity: .6; } }
.muted { color: #999; font-size: .9rem; }
.error { color: #ff7b7b; margin: 0; }
.pill { padding: .15rem .6rem; border-radius: 999px; background: #5a2a2a; font-size: .8rem; margin-right: .3rem; }
.pill.ok { background: #1f5a34; }
.dot { display: inline-block; width: .6rem; height: .6rem; border-radius: 50%; background: #a33; }
.dot.ok { background: #2d9d57; }
.warn { color: #ffcf7a; margin: 0; font-size: .9rem; line-height: 1.35; }
.warn a, .muted a { color: #8fb0ff; }
.actions { display: flex; gap: .6rem; }
.actions .primary { flex: 1; }
.small { padding: .25rem .6rem; font-size: .8rem; }
.name { padding: .45rem .6rem; background: #1b1b1b; color: #eee; border: 1px solid #444; border-radius: 8px; font: inherit; }
.saved { margin: 0; font-size: .85rem; color: #9fd8b0; display: flex; gap: .5rem; align-items: center; flex-wrap: wrap; }
.listen { margin-left: .4rem; padding: 0 .35rem; font-size: .8rem; background: transparent; border: none; opacity: .55; vertical-align: middle; }
.listen:hover:not(:disabled) { opacity: 1; }
.meter { position: relative; display: inline-flex; align-items: center; width: 90px; height: 1.2rem; background: #222; border: 1px solid #444; border-radius: 6px; overflow: hidden; }
.meter i { position: absolute; left: 0; top: 0; bottom: 0; background: #2d9d57; transition: width .08s; }
.meter b { position: relative; margin-left: auto; padding-right: .3rem; font-size: .7rem; font-weight: normal; color: #eee; }
select, label { color: #eee; }
select { width: 100%; padding: .5rem; margin-top: .3rem; background: #1b1b1b; border: 1px solid #444; border-radius: 8px; }

.confirm { position: absolute; inset: 0; z-index: 10; display: flex; align-items: center; justify-content: center; background: rgba(0,0,0,.6); }
.confirm .box { background: #1b1b1b; border: 1px solid #444; border-radius: 12px; padding: 1.2rem 1.5rem; max-width: min(420px, 90%); }
.confirm .box p { margin: 0 0 1rem; }
.confirm .row { display: flex; gap: .6rem; justify-content: flex-end; }
/* Conversation : le chat seul, centré ; quand l'assistant affiche une source, le chat passe à gauche et la source à droite (en colonne sur petit écran) */
.live.split { flex-direction: row; justify-content: center; align-items: stretch; }
.live.split .chat { flex: 1 1 auto; width: 100%; max-width: 820px; margin: 0 auto; padding: .8rem 1rem; background: rgba(14,14,14,.96); }
.live.split .msg { max-width: 92%; }
.live.split.with-stage { flex-direction: row-reverse; justify-content: flex-start; }
.live.split.with-stage .chat { flex: 0 0 42%; min-width: 340px; max-width: 640px; margin: 0; border-right: 1px solid #2a2a2a; }
.caption { position: absolute; left: .6rem; right: 3.4rem; bottom: .6rem; z-index: 2; margin: 0; padding: .3rem .7rem; border-radius: 8px; background: rgba(0,0,0,.65); color: #ddd; font-size: .85rem; line-height: 1.35; pointer-events: none; }
.fs { position: absolute; top: .6rem; right: .6rem; z-index: 2; font-size: 1rem; padding: .3rem .6rem; background: rgba(0,0,0,.6); }
@media (max-width: 800px) {
  .live.split.with-stage { flex-direction: column; }
  .live.split.with-stage .stage { flex: 0 0 34%; max-height: 40vh; }
  .live.split.with-stage .chat { flex: 1 1 0; min-width: 0; max-width: none; border-right: none; border-top: 1px solid #2a2a2a; }
}
</style>
