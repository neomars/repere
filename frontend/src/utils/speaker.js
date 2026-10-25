// Lecture de la voix de l'IA : file d'attente WebAudio (les phrases s'enchaînent sans coupure) et AnalyserNode
// (niveau du signal, pour qui veut l'afficher).
//
// Les navigateurs ne démarrent le son qu'après un geste de l'utilisateur : `unlock()` est appelé depuis un clic
// (« Commencer », « Envoyer », micro, « Tester la voix »).

// Avance de l'analyseur sur le son : il lit le signal tout de suite, la sortie est retardée d'autant (utile à une animation
// qui doit anticiper le son de 40-60 ms pour paraître synchrone).
const LIP_LEAD_S = 0.05

export class Speaker {
  constructor() {
    this.ctx = null
    this.analyser = null
    this.delay = null
    this.sources = new Set()
    this.nextTime = 0
    this.pending = 0 // morceaux reçus mais pas encore programmés (décodage en cours)
    this.epoch = 0 // incrémenté par stop() : un décodage en cours d'un ancien lot est abandonné
    this.chain = Promise.resolve() // les morceaux sont décodés et programmés dans l'ordre d'arrivée
    this.speaking = false
    /** Appelées par le propriétaire : (analyser) à la création, () au premier son d'un lot, (speaking), (message). */
    this.onReady = null
    this.onStart = null
    this.onChange = null
    this.onError = null
    /** (tag, début sur l'horloge audio, durée) : un morceau vient d'être programmé ; sert à caler une animation sur le son. */
    this.onChunk = null
  }

  _ensure() {
    if (this.ctx) return this.ctx
    const Ctx = window.AudioContext || window.webkitAudioContext
    if (!Ctx) throw new Error("Ce navigateur ne sait pas lire l'audio (WebAudio absent).")
    this.ctx = new Ctx()
    this.analyser = this.ctx.createAnalyser()
    this.analyser.fftSize = 1024
    this.analyser.smoothingTimeConstant = 0.4
    // L'analyseur n'est qu'une prise : le son sort par un retard (DelayNode) vers les haut-parleurs.
    this.delay = this.ctx.createDelay(0.5)
    this.delay.delayTime.value = LIP_LEAD_S
    this.delay.connect(this.ctx.destination)
    this.onReady?.(this.analyser)
    return this.ctx
  }

  /** À appeler dans un geste de l'utilisateur. Ne lève jamais : renvoie false si le son reste bloqué. */
  async unlock() {
    try {
      const ctx = this._ensure()
      if (ctx.state !== 'running') await ctx.resume()
      return ctx.state === 'running'
    } catch (e) {
      this.onError?.(e.message || String(e))
      return false
    }
  }

  /** Ajoute un morceau audio (WAV, MP3, OGG…) à la suite de ce qui joue. */
  enqueue(arrayBuffer, tag = null) {
    const epoch = this.epoch
    this.pending++
    this._setSpeaking()
    this.chain = this.chain.then(() => this._play(arrayBuffer, epoch, tag)).catch(() => {})
  }

  /** Heure actuelle de ce qu'on entend, sur l'horloge audio (compense la latence de sortie et l'avance de la bouche). */
  heardTime() {
    if (!this.ctx) return null
    return this.ctx.currentTime - (this.ctx.outputLatency || 0) - LIP_LEAD_S
  }

  async _play(arrayBuffer, epoch, tag) {
    try {
      const ctx = this._ensure()
      const buffer = await ctx.decodeAudioData(arrayBuffer)
      if (epoch !== this.epoch) return // stop() est passé entre-temps
      if (ctx.state !== 'running') await ctx.resume().catch(() => {})
      if (ctx.state !== 'running') {
        this.onError?.("Le navigateur bloque le son : cliquez sur « Tester la voix » (ou n'importe où dans la page) pour l'autoriser.")
      }
      const src = ctx.createBufferSource()
      src.buffer = buffer
      src.connect(this.analyser)
      src.connect(this.delay)
      const first = this.sources.size === 0
      const at = Math.max(ctx.currentTime + 0.03, this.nextTime)
      this.nextTime = at + buffer.duration
      src.onended = () => {
        this.sources.delete(src)
        this._setSpeaking()
      }
      this.sources.add(src)
      src.start(at)
      this.onChunk?.(tag, at, buffer.duration)
      if (first) this.onStart?.()
    } catch (e) {
      if (epoch === this.epoch) {
        this.onError?.(e && e.name === 'EncodingError' ? "Le son reçu est illisible par le navigateur." : `Lecture impossible : ${e && e.message}`)
      }
    } finally {
      if (epoch === this.epoch) this.pending = Math.max(0, this.pending - 1)
      this._setSpeaking()
    }
  }

  /** Coupe tout de suite ce qui joue et abandonne ce qui attend. */
  stop() {
    this.epoch++
    for (const src of this.sources) {
      src.onended = null
      try { src.stop() } catch { /* déjà terminé */ }
    }
    this.sources.clear()
    this.nextTime = 0
    this.pending = 0
    this._setSpeaking()
  }

  _setSpeaking() {
    const now = this.sources.size > 0 || this.pending > 0
    if (now !== this.speaking) {
      this.speaking = now
      this.onChange?.(now)
    }
  }

  close() {
    this.stop()
    try { this.ctx?.close() } catch { /* sans effet */ }
    this.ctx = null
    this.analyser = null
    this.delay = null
  }
}
