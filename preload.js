// Pont minimal entre la page et Electron : seule la boîte de dialogue « choisir un dossier » est exposée.
const { contextBridge, ipcRenderer } = require('electron');

contextBridge.exposeInMainWorld('repereNative', {
  /** Boîte de dialogue du système ; renvoie le chemin du dossier choisi, ou null si annulé. */
  pickFolder: () => ipcRenderer.invoke('repere:pick-folder'),
});
