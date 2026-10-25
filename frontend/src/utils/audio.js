// Enregistrement micro → WAV PCM 16 bits, 16 kHz, mono (format attendu par whisper.cpp).

function encodeWav(samples, sampleRate) {
  const buf = new ArrayBuffer(44 + samples.length * 2)
  const v = new DataView(buf)
  const str = (o, s) => [...s].forEach((c, i) => v.setUint8(o + i, c.charCodeAt(0)))
  str(0, 'RIFF'); v.setUint32(4, 36 + samples.length * 2, true); str(8, 'WAVE')
  str(12, 'fmt '); v.setUint32(16, 16, true); v.setUint16(20, 1, true); v.setUint16(22, 1, true)
  v.setUint32(24, sampleRate, true); v.setUint32(28, sampleRate * 2, true)
  v.setUint16(32, 2, true); v.setUint16(34, 16, true)
  str(36, 'data'); v.setUint32(40, samples.length * 2, true)
  for (let i = 0; i < samples.length; i++) {
    const s = Math.max(-1, Math.min(1, samples[i]))
    v.setInt16(44 + i * 2, s < 0 ? s * 0x8000 : s * 0x7fff, true)
  }
  return buf
}

/** Décode l'enregistrement du navigateur (webm/opus…), le passe en mono 16 kHz et renvoie aussi son crête. */
async function toWav16k(blob) {
  const ctx = new (window.AudioContext || window.webkitAudioContext)()
  let decoded
  try {
    decoded = await ctx.decodeAudioData(await blob.arrayBuffer())
  } finally {
    ctx.close()
  }
  const length = Math.max(1, Math.ceil(decoded.duration * 16000))
  const off = new OfflineAudioContext(1, length, 16000)
  const src = off.createBufferSource()
  src.buffer = decoded
  src.connect(off.destination) // le mixage mono est fait par le contexte hors-ligne
  src.start()
  const rendered = await off.startRendering()
  const samples = rendered.getChannelData(0)
  let peak = 0
  for (let i = 0; i < samples.length; i++) peak = Math.max(peak, Math.abs(samples[i]))
  return { wav: encodeWav(samples, 16000), seconds: decoded.duration, peak }
}

/** Message lisible pour une erreur de getUserMedia. */
export function micErrorMessage(e) {
  switch (e && e.name) {
    case 'NotAllowedError':
    case 'SecurityError':
      return "Accès au micro refusé : autorisez-le (paramètres de confidentialité d'Ubuntu → Microphone, ou l'icône du micro dans la barre d'adresse du navigateur)."
    case 'NotFoundError':
    case 'DevicesNotFoundError':
    case 'OverconstrainedError':
      return 'Aucun micro détecté sur cet ordinateur.'
    case 'NotReadableError':
    case 'AbortError':
      return 'Le micro est inaccessible (utilisé par un autre programme, ou bloqué par le système).'
    default:
      return `Micro indisponible : ${(e && e.message) || e}`
  }
}

export class Recorder {
  constructor() {
    this.rec = null
    this.chunks = []
    this.stream = null
    this.ctx = null
    this.analyser = null
  }

  async start() {
    if (!navigator.mediaDevices || !navigator.mediaDevices.getUserMedia) {
      throw new Error("Le micro exige une page sécurisée : ouvrez l'application par http://localhost ou http://127.0.0.1 (pas par l'adresse IP du réseau).")
    }
    try {
      this.stream = await navigator.mediaDevices.getUserMedia({
        audio: { echoCancellation: true, noiseSuppression: true },
      })
    } catch (e) {
      throw new Error(micErrorMessage(e))
    }
    this.chunks = []
    this.rec = new MediaRecorder(this.stream)
    this.rec.ondataavailable = (e) => e.data.size && this.chunks.push(e.data)
    this.rec.start()
    try { // indicateur de niveau : montre tout de suite si le micro capte quelque chose
      this.ctx = new (window.AudioContext || window.webkitAudioContext)()
      this.analyser = this.ctx.createAnalyser()
      this.analyser.fftSize = 512
      this.ctx.createMediaStreamSource(this.stream).connect(this.analyser)
    } catch { this.analyser = null }
  }

  /** Niveau instantané du micro, de 0 à 1. */
  level() {
    if (!this.analyser) return 0
    const d = new Uint8Array(this.analyser.fftSize)
    this.analyser.getByteTimeDomainData(d)
    let sum = 0
    for (const x of d) sum += ((x - 128) / 128) ** 2
    return Math.min(1, Math.sqrt(sum / d.length) * 4)
  }

  _release() {
    this.stream?.getTracks().forEach((t) => t.stop())
    try { this.ctx?.close() } catch { /* sans effet */ }
    this.ctx = null
    this.analyser = null
  }

  /**
   * Arrête l'enregistrement. Renvoie `{ wav, seconds, peak }`, ou `null` s'il n'y avait pas d'enregistrement.
   * Rejette avec un message lisible si l'enregistrement est illisible.
   */
  stop() {
    return new Promise((resolve, reject) => {
      if (!this.rec) return resolve(null)
      this.rec.onstop = async () => {
        this._release()
        const blob = new Blob(this.chunks, { type: this.rec.mimeType })
        this.rec = null
        try {
          resolve(await toWav16k(blob))
        } catch {
          reject(new Error("Enregistrement illisible par le navigateur (format audio non pris en charge)."))
        }
      }
      this.rec.stop()
    })
  }

  /** Abandonne sans rien produire (changement de page, erreur). */
  cancel() {
    if (this.rec) { this.rec.onstop = null; try { this.rec.stop() } catch { /* déjà arrêté */ } }
    this.rec = null
    this._release()
  }
}
