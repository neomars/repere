<template>
  <section class="card">
    <h3>Essayer une question</h3>
    <p class="muted small">
      Voyez ce que l'assistant recevrait pour une question : les passages retenus, dans l'ordre. Les {{ offeredCount }} premiers lui sont proposés ;
      s'il n'y en a aucun, il répond qu'il ne sait pas et propose d'ouvrir un ticket.
    </p>
    <form class="row" @submit.prevent="run">
      <input v-model="question" type="search" placeholder="Ex. : j'ai oublié mon mdp" autocomplete="off" aria-label="Question à essayer" />
      <button class="primary" type="submit" :disabled="!question.trim() || busy">Chercher</button>
    </form>
    <p v-if="problem" class="err" role="alert">{{ problem }}</p>

    <div v-if="result" class="result" aria-live="polite">
      <p v-if="result.concepts.length" class="small">
        Jargon reconnu (dictionnaire de synonymes) : <span v-for="c in result.concepts" :key="c" class="pill info chip">{{ c }}</span>
      </p>
      <p v-if="result.unknown.length" class="muted small">
        Mots que la base de connaissances ne contient nulle part : {{ result.unknown.join(', ') }}.
      </p>
      <p v-if="!result.hits.length" class="warn">
        Aucun passage ne serait proposé à l'assistant pour cette question : il répondrait qu'il ne sait pas.
      </p>
      <ol v-else class="plain hits">
        <li v-for="(h, i) in result.hits" :key="h.source.id" :class="{ off: !h.offered }">
          <div class="row">
            <b>{{ i + 1 }}.</b>
            <button v-if="h.source.kind === 'passage'" type="button" class="link" @click="$emit('open', h.source.id)">{{ label(h.source) }}</button>
            <span v-else>{{ h.source.title }} <span class="pill">image ou vidéo</span></span>
            <span v-if="h.offered" class="pill on">proposé à l'assistant</span>
            <span v-else class="pill" title="Le haut du classement seulement est transmis à l'assistant">au-delà des {{ offeredCount }} premiers</span>
          </div>
          <p class="muted small">score {{ h.score }} · {{ h.matched }} mot{{ h.matched > 1 ? 's' : '' }} de la question retrouvé{{ h.matched > 1 ? 's' : '' }} · {{ excerpt(h.source) }}</p>
        </li>
      </ol>
    </div>
  </section>
</template>

<script setup>
import { ref } from 'vue'
import axios from 'axios'
import { errorText, sourcePath } from '../../utils/kb.js'

defineEmits(['open'])

const offeredCount = 4 // MAX_PICKS du moteur
const question = ref('')
const result = ref(null)
const busy = ref(false)
const problem = ref('')

const label = (s) => sourcePath(s)
const excerpt = (s) => {
  const t = (s.description || '').replace(/\s+/g, ' ').trim()
  return t.length > 160 ? `${t.slice(0, 157).trimEnd()}…` : t
}

async function run() {
  if (!question.value.trim() || busy.value) return
  busy.value = true
  problem.value = ''
  try {
    result.value = (await axios.get('/api/live/kb/search', { params: { q: question.value.trim(), limit: 8 } })).data
  } catch (e) {
    result.value = null
    problem.value = errorText(e)
  } finally {
    busy.value = false
  }
}
</script>

<style scoped src="../../assets/styles/Kb.css"></style>
<style scoped>
.result { display: flex; flex-direction: column; gap: .5rem; }
.result p { margin: 0; }
.chip { margin-right: .3rem; }
.hits { gap: .6rem; }
.hits li.off { opacity: .65; }
</style>
