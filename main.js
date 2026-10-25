const { app, BrowserWindow, dialog, ipcMain } = require('electron');
const { spawn } = require('child_process');
const path = require('path');
const fs = require('fs');
const os = require('os');
const http = require('http');

// Ubuntu 23.10+/24.04 : AppArmor interdit le bac à sable Chromium (SUID/userns) aux AppImage, qui ne peuvent
// pas installer chrome-sandbox ; sans cette option l'app ne démarre pas. Le .deb garde le bac à sable.
// L'app ne charge que sa propre page locale (http://localhost), jamais de contenu distant.
if (process.env.APPIMAGE) app.commandLine.appendSwitch('no-sandbox');

const ENGINE_PORT = 3001;
const WEB_PORT = 3000;

// ---- Emplacements --------------------------------------------------------------------------
// App empaquetée (AppImage/deb) : le dossier de l'app est en lecture seule → tout ce qui est
// modifiable (base, médias, modèles IA) vit dans le dossier utilisateur. Il s'appelle « repere », sans accent, quel que soit
// le nom affiché « Repère » (Electron prendrait sinon le productName).
app.setPath('userData', path.join(app.getPath('appData'), 'repere'));
const userData = app.getPath('userData'); // ~/.config/repere
const dataDir = process.env.REPERE_DATA_DIR || userData;
const modelsDir =
  process.env.REPERE_MODELS_DIR ||
  path.join(process.env.XDG_DATA_HOME || path.join(os.homedir(), '.local', 'share'), 'repere', 'models');
const resources = app.isPackaged ? process.resourcesPath : __dirname;
const engineBin = app.isPackaged
  ? path.join(resources, 'engine', 'live-engine')
  : path.join(__dirname, 'live-engine', 'target', 'release', 'live-engine');
const binDir = app.isPackaged ? path.join(resources, 'bin') : path.join(__dirname, 'bin');

let mainWindow = null;
let engine = null;
let quitting = false;

// L'AppImage ajoute ses propres bibliothèques à LD_LIBRARY_PATH : elles ne doivent pas polluer llama-server,
// whisper-server ni Python (même précaution que `unset LD_LIBRARY_PATH` pour ffmpeg dans le backend).
function cleanLdPath() {
  const appdir = process.env.APPDIR;
  return (process.env.LD_LIBRARY_PATH || '')
    .split(':')
    .filter((p) => p && !(appdir && p.startsWith(appdir)))
    .join(':');
}

// ffmpeg embarqué (paquet ffmpeg-static, décompressé hors de l'archive asar) ; à défaut, celui du système.
function ffmpegBin() {
  try {
    const p = require('ffmpeg-static').replace('app.asar', 'app.asar.unpacked');
    if (fs.existsSync(p)) return p;
  } catch { /* paquet absent : on essaie celui du système */ }
  return 'ffmpeg';
}

function logStream(name) {
  fs.mkdirSync(path.join(userData, 'logs'), { recursive: true });
  return fs.createWriteStream(path.join(userData, 'logs', name), { flags: 'a' });
}

// ---- Moteur IA (Rust) ----------------------------------------------------------------------
function startEngine() {
  if (!fs.existsSync(engineBin)) {
    console.warn(`[live] moteur introuvable (${engineBin}) : mode IA indisponible. Compile-le avec scripts/build-linux.sh`);
    return;
  }
  const log = logStream('live-engine.log');
  fs.mkdirSync(modelsDir, { recursive: true });
  engine = spawn(engineBin, [], {
    // stdin en tube : si l'app meurt, le tube se ferme et le moteur décharge les modèles tout seul.
    stdio: ['pipe', 'pipe', 'pipe'],
    env: {
      ...process.env,
      LD_LIBRARY_PATH: cleanLdPath(),
      LIVE_BIND: `127.0.0.1:${ENGINE_PORT}`,
      LIVE_DATA_DIR: path.join(dataDir, 'live'),
      LIVE_MODELS_DIR: modelsDir,
      LIVE_BIN_DIR: binDir,
      LIVE_UPLOADS_DIR: path.join(dataDir, 'uploads'),
      LIVE_FRONTEND_DIR: path.join(dataDir, 'no-frontend'), // l'interface est servie par Express
      LIVE_EXIT_ON_STDIN_EOF: '1',
      REPERE_FFMPEG: ffmpegBin(), // miniatures de l'import massif
    },
  });
  engine.stdout.pipe(log);
  engine.stderr.pipe(log);
  engine.on('exit', (code, signal) => {
    console.warn(`[live] moteur arrêté (code ${code}, signal ${signal})`);
    engine = null;
  });
  engine.on('error', (e) => console.error('[live] impossible de lancer le moteur :', e.message));
}

function stopEngine() {
  if (!engine) return Promise.resolve();
  const e = engine;
  return new Promise((resolve) => {
    const kill = setTimeout(() => e.kill('SIGKILL'), 8000);
    e.once('exit', () => { clearTimeout(kill); resolve(); });
    e.kill('SIGTERM'); // le moteur décharge les modèles (libère le GPU) avant de quitter
  });
}

// ---- Fenêtre -------------------------------------------------------------------------------
function waitForServer(port, tries = 60) {
  return new Promise((resolve) => {
    const attempt = (n) => {
      const req = http.get({ host: '127.0.0.1', port, path: '/', timeout: 1000 }, (res) => { res.resume(); resolve(true); });
      req.on('error', () => (n > 0 ? setTimeout(() => attempt(n - 1), 250) : resolve(false)));
      req.on('timeout', () => req.destroy());
    };
    attempt(tries);
  });
}

async function createWindow() {
  mainWindow = new BrowserWindow({
    width: 1280,
    height: 800,
    autoHideMenuBar: true,
    webPreferences: { nodeIntegration: false, contextIsolation: true, preload: path.join(__dirname, 'preload.js') },
  });
  // Micro pour la reconnaissance vocale : autorisé uniquement pour notre propre page locale.
  mainWindow.webContents.session.setPermissionRequestHandler((wc, permission, cb) => {
    cb(permission === 'media' && wc.getURL().startsWith(`http://127.0.0.1:${WEB_PORT}`));
  });
  if (!(await waitForServer(WEB_PORT))) {
    dialog.showErrorBox('Repère', `Le serveur local n'a pas démarré sur le port ${WEB_PORT}.`);
    return;
  }
  mainWindow.loadURL(`http://127.0.0.1:${WEB_PORT}`); // IPv4 imposée : « localhost » peut résoudre en ::1
}

// Import massif : choix du dossier à importer (réservé à notre propre page locale).
ipcMain.handle('repere:pick-folder', async (event) => {
  if (!event.senderFrame.url.startsWith(`http://127.0.0.1:${WEB_PORT}`)) return null;
  const r = await dialog.showOpenDialog(mainWindow, {
    title: 'Dossier à importer',
    properties: ['openDirectory'],
  });
  return r.canceled || !r.filePaths[0] ? null : r.filePaths[0];
});

// ---- Cycle de vie --------------------------------------------------------------------------
if (!app.requestSingleInstanceLock()) {
  app.quit(); // une seule instance : sinon deux moteurs se disputeraient les ports et le GPU
} else {
  app.on('second-instance', () => {
    if (mainWindow) { if (mainWindow.isMinimized()) mainWindow.restore(); mainWindow.focus(); }
  });

  app.whenReady().then(() => {
    process.env.PORT = String(WEB_PORT);
    process.env.HOST = '127.0.0.1'; // l'app est locale : pas d'exposition sur le réseau
    process.env.NODE_ENV = 'production';
    process.env.INSECURE_COOKIES = '1'; // HTTP local (voir backend/server.js)
    process.env.REPERE_DATA_DIR = dataDir;
    fs.mkdirSync(dataDir, { recursive: true });
    startEngine();
    // Dans ce .then(), une exception (module absent du paquet, fichier illisible…) serait une promesse rejetée SILENCIEUSE :
    // l'app resterait lancée, sans fenêtre ni message. On la montre, puis on quitte (le moteur est arrêté avec l'app).
    try {
      require('./backend/server.js');
    } catch (e) {
      console.error('[web] serveur local impossible à démarrer :', e);
      dialog.showErrorBox('Repère', `Le serveur local n'a pas pu démarrer :\n\n${e.message}`);
      app.quit();
      return;
    }
    createWindow();
  });

  app.on('before-quit', (ev) => {
    if (quitting || !engine) return;
    ev.preventDefault();
    quitting = true;
    stopEngine().finally(() => app.quit());
  });

  app.on('window-all-closed', () => {
    if (process.platform !== 'darwin') app.quit();
  });
}
