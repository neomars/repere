// Logique pure des écrans de la base de connaissances, des sources citées par l'assistant et du ticket (pas de DOM) : testée par `npm test`.

/** Formats que le moteur lit, par la clé qu'il enregistre (`format` d'un document). */
export const FORMATS = { md: 'Markdown', html: 'HTML', docx: 'Word', pdf: 'PDF', txt: 'Texte' }

export const formatLabel = (key) => FORMATS[key] || String(key || '?').toUpperCase()

/** Extensions proposées dans les boîtes « choisir un fichier » (le moteur reconnaît aussi un HTML sans extension, par son contenu). */
export const EXTENSIONS = ['md', 'markdown', 'html', 'htm', 'docx', 'pdf', 'txt']
export const ACCEPT = EXTENSIONS.map((e) => `.${e}`).join(',')

/** Taille maximale d'un document (la même que celle du moteur). */
export const MAX_BYTES = 50 * 1024 * 1024

/** Le nom de fichier a-t-il une extension que le moteur lit ? (Un dossier choisi contient d'autres fichiers : on les laisse de côté.) */
export function isReadable(name) {
  const ext = String(name || '').split('.').pop().toLowerCase()
  return String(name || '').includes('.') && EXTENSIONS.includes(ext)
}

/** 1 536 octets → « 1,5 Ko ». */
export function formatBytes(n) {
  const b = Number(n) || 0
  const unit = (v, u) => `${v.toLocaleString('fr-FR', { maximumFractionDigits: v >= 10 ? 0 : 1 })} ${u}`
  if (b < 1024) return `${b} o`
  if (b < 1024 ** 2) return unit(b / 1024, 'Ko')
  if (b < 1024 ** 3) return unit(b / 1024 ** 2, 'Mo')
  return unit(b / 1024 ** 3, 'Go')
}

/** Une adresse à ouvrir dans un lien : http(s) seulement (jamais « javascript: » ni « data: »). Chaîne vide sinon. */
export function safeHref(url) {
  const u = String(url || '').trim()
  return /^https?:\/\/[^\s]+$/i.test(u) ? u : ''
}

/** Le chemin complet d'une source : « Client VPN › Les erreurs courantes › Erreur 809 (p. 3) ». */
export function sourcePath(s) {
  if (!s) return ''
  const name = s.section ? `${s.title} › ${s.section}` : s.title || 'Document'
  return s.page ? `${name} (p. ${s.page})` : name
}

/**
 * Le nom court d'une source, pour un lien : « Client VPN › Erreur 809 ». On garde le dernier niveau du chemin (c'est lui qui distingue
 * deux passages d'un même document) et on raccourcit plutôt le titre du document si la place manque.
 */
export function sourceLabel(s, max = 64) {
  if (!s) return ''
  const cut = (text, n) => (text.length > n ? `${text.slice(0, Math.max(1, n - 1)).trimEnd()}…` : text)
  const title = s.title || 'Document'
  const leaf = String(s.section || '').split(' › ').pop().trim()
  if (!leaf) return cut(title, max)
  const room = max - 3 - Math.min(leaf.length, max - 15) // la fin du chemin passe avant le titre, qui garde au moins 12 caractères
  return `${cut(title, Math.max(12, room))} › ${cut(leaf, max - 3 - Math.min(title.length, Math.max(12, room)))}`
}

/** Ajoute une source à la liste de la réponse en cours, sans doublon (par numéro). Rend une nouvelle liste. */
export function addSource(list, source) {
  const items = list || []
  if (!source || items.some((x) => x.id === source.id)) return items
  return [...items, source]
}

/** Ce qu'un envoi de document a donné, pour l'afficher : `tone` = ok | same | bad. */
export function outcomeView(o) {
  if (!o) return { tone: 'bad', text: 'Pas de réponse du moteur.' }
  switch (o.status) {
    case 'added': {
      const n = `${o.passages} passage${o.passages > 1 ? 's' : ''}`
      return { tone: 'ok', text: o.replaced ? `Remplacé (${n})` : `Ajouté (${n})` }
    }
    case 'unchanged': return { tone: 'same', text: 'Déjà importé, inchangé' }
    case 'duplicate': return { tone: 'same', text: `Même contenu que « ${o.title} » : ignoré` }
    default: return { tone: 'bad', text: o.error || o.message || 'Refusé' }
  }
}

/** Nom sous lequel un fichier choisi est enregistré : son chemin dans le dossier choisi, à défaut son nom. */
export const uploadName = (file) => String(file?.webkitRelativePath || file?.name || '')

/** Les noms de dossiers du chemin d'un fichier (sans le fichier) : des étiquettes pour le retrouver. « Guides/VPN/client.docx » → Guides, VPN. */
export function folderTags(path) {
  const parts = String(path || '').split('/').map((p) => p.trim()).filter(Boolean)
  return parts.slice(0, -1)
}

/** Étiquettes saisies, séparées par des virgules. */
export const parseTags = (text) => String(text ?? '').split(',').map((t) => t.trim()).filter(Boolean)

/** Le paramètre d'adresse pour envoyer un fichier : `?name=…&tags=…`. */
export function uploadQuery(path, tags = []) {
  const q = new URLSearchParams({ name: path })
  if (tags.length) q.set('tags', tags.join(','))
  return q.toString()
}

/** Le ticket en texte, à copier dans un courriel ou un autre outil de suivi. */
export function ticketText(t) {
  const head = [t.reference, t.title].filter(Boolean).join(' — ')
  return [head, '', String(t.description || '').trim()].join('\n').trim()
}

/** Une réponse rapide ouvre-t-elle le ticket ? (le moteur donne l'intitulé du bouton qui le fait) */
export const isTicketReply = (reply, ticketLabel) => !!ticketLabel && reply === ticketLabel

/** Statuts d'un ticket, dans l'ordre où il avance. */
export const TICKET_STATUSES = ['nouveau', 'pris en charge', 'clos']

/** En-têtes d'une écriture dans l'administration du moteur (jeton d'administration facultatif, mémorisé par la page des modèles). */
export function adminHeaders() {
  try {
    const t = localStorage.getItem('live_admin_token')
    return t ? { Authorization: `Bearer ${t}` } : {}
  } catch { return {} }
}

/** Le message d'erreur d'une requête axios, en français si le moteur en a donné un. */
export const errorText = (e) => e?.response?.data?.message || e?.message || 'Erreur inconnue'
