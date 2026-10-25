// Réglages mémorisés dans le navigateur (localStorage). Il peut être absent ou refuser d'écrire (navigation privée, données bloquées) :
// la lecture rend alors `null`, l'écriture ne fait rien, et l'écran fonctionne quand même.
export const store = {
  get(key) {
    try { return JSON.parse(localStorage.getItem(key) || 'null') } catch { return null }
  },
  set(key, value) {
    try { localStorage.setItem(key, JSON.stringify(value)) } catch { /* stockage indisponible : sans effet */ }
  },
}
