// Les lignes de la base de connaissances que l'on modifie à la main : ce qu'il faut envoyer au moteur, et comment une liste rechargée se fond avec ce
// qui est à l'écran. Pure logique (pas de DOM) : testée par `npm test`.
//
// La liste se recharge toute seule toutes les quelques secondes pendant une analyse ou un import. Ce rechargement ne doit JAMAIS toucher à une
// ligne que l'on est en train de modifier : la réponse du moteur peut dater d'avant la frappe (le texte reviendrait en arrière) et rend le texte
// « normalisé » (tags en minuscules, virgules et espaces nettoyés), qui changerait sous les doigts.

/** Les champs que l'on modifie à la main, tels que la ligne les porte (`tagsText` : les tags séparés par des virgules). */
export const EDITABLE = ['title', 'description', 'tagsText']

export const parseTags = (text) => String(text ?? '').split(',').map((t) => t.trim()).filter(Boolean)

/** Les valeurs de la ligne telles que le moteur les connaît. */
export const baseOf = (m) => ({ title: m.title, description: m.description, tagsText: m.tagsText })

/** Une ligne telle que le moteur la renvoie, prête à être modifiée. */
export function rowFromServer(m) {
  const row = { ...m, tagsText: (m.tags || []).join(', '), _open: false, _save: 'idle', _err: '' }
  row._base = baseOf(row)
  return row
}

/**
 * Ce qui a changé depuis la dernière version connue du moteur : `patch` à lui envoyer, `sent` à retenir ensuite comme nouvelle version
 * connue. Rien d'autre n'est envoyé : une analyse qui écrit la description pendant que l'on corrige le titre n'est pas écrasée.
 */
export function changes(m) {
  const b = m._base
  const patch = {}
  const sent = {}
  if (m.title !== b.title) { patch.title = m.title; sent.title = m.title }
  if (m.description !== b.description) { patch.description = m.description; sent.description = m.description }
  if (m.tagsText !== b.tagsText) { patch.tags = parseTags(m.tagsText); sent.tagsText = m.tagsText }
  return { patch, sent }
}

/**
 * Une ligne rechargée (`fresh`, issue de `rowFromServer`) et la même avant le rechargement (`old`) : que montrer ?
 * La version du moteur, sauf pour une ligne que l'on est en train de modifier, qui garde ce qui est à l'écran :
 *  - le curseur est dans la ligne ;
 *  - un champ diffère de ce que le moteur connaît (modifié, pas encore enregistré), ou un enregistrement est en attente, en cours ou en échec ;
 *  - l'utilisateur ou un enregistrement a bougé depuis `since`, l'instant où la liste a été demandée : la réponse du moteur est périmée.
 * Une fois le curseur parti et tout enregistré, le rechargement suivant montre la version du moteur (normalisée).
 *
 * @param {{ focused?: boolean, dirty?: boolean, lastActivity?: number }} activity  ce que l'on sait de la ligne (curseur, enregistreur)
 */
export function mergeReloaded(fresh, old, { focused = false, dirty = false, lastActivity = 0 } = {}, since = Infinity) {
  fresh._open = !!old?._open // une ligne dépliée le reste
  const editing = !!old?._base && (focused || dirty || lastActivity > since || EDITABLE.some((f) => old[f] !== old._base[f]))
  if (editing) {
    for (const f of EDITABLE) fresh[f] = old[f]
    fresh._base = { ...old._base } // ce que le moteur connaissait (et ce qu'on lui a envoyé) : seul ce qui en diffère sera renvoyé
  }
  return fresh
}
