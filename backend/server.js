const express = require('express');
const session = require('express-session');
const helmet = require('helmet');
const cors = require('cors');
const compression = require('compression');
const crypto = require('crypto');
const path = require('path');
const fs = require('fs').promises;

const { apiLimiter } = require('./middleware/rateLimiter');

const liveProxy = require('./liveProxy');
const { uploadsDir } = require('./utils/paths');
const { ensureDefaults } = require('./bootstrap');

const app = express();
const PORT = process.env.PORT || 3000;
const HOST = process.env.HOST || '0.0.0.0';
const isProd = process.env.NODE_ENV === 'production';

// Basic security headers
app.use(helmet({
  contentSecurityPolicy: false,
  crossOriginResourcePolicy: { policy: "cross-origin" }
}));

app.use(cors({
  origin: true,
  credentials: true
}));

// Session configuration. Sans SESSION_SECRET, une clé aléatoire est tirée à chaque démarrage : les sessions sont gardées en mémoire,
// elles ne survivent de toute façon pas à un redémarrage, et aucune clé connue de tous n'est codée dans le dépôt.
app.use(session({
  secret: process.env.SESSION_SECRET || crypto.randomBytes(32).toString('hex'),
  resave: false,
  saveUninitialized: false,
  name: 'repere.sid',
  cookie: {
    // L'app Electron sert du HTTP local : un cookie « secure » n'y serait jamais posé (connexion impossible).
    secure: isProd && process.env.INSECURE_COOKIES !== '1',
    httpOnly: true,
    sameSite: 'lax',
    maxAge: 24 * 60 * 60 * 1000
  }
}));

// Moteur IA live (Rust) : proxy REST + WebSocket. Placé après la session (pour exiger une connexion sur les
// routes qui modifient quelque chose) et avant express.json() (le corps doit rester un flux).
app.use(liveProxy.middleware);

// Compression gzip/brotli du JS, du CSS, du HTML et des réponses JSON de l'API. Placée après le proxy live (qui relaie les flux
// du moteur tels quels) ; les vidéos et images, déjà compressées, ne passent pas par le filtre par défaut.
app.use(compression());

app.use(express.json());

// Global API Rate Limiter
app.use('/api/', apiLimiter);

// Base JSON : crée l'admin par défaut au premier lancement (app empaquetée : personne ne lance init-db.js).
ensureDefaults().catch((e) => console.error('Initialisation de la base impossible :', e));

// Dossiers des médias de la base de connaissances (importés par le moteur)
const videosDir = path.join(uploadsDir, 'videos');
const thumbnailsDir = path.join(uploadsDir, 'thumbnails');
const photosDir = path.join(uploadsDir, 'photos');

(async () => {
    try {
        await fs.mkdir(videosDir, { recursive: true });
        await fs.mkdir(thumbnailsDir, { recursive: true });
        await fs.mkdir(photosDir, { recursive: true });
    } catch (error) {
        console.error("Impossible de créer les dossiers de médias :", error);
    }
})();

// Static assets
app.use('/videos', express.static(videosDir));
app.use('/thumbnails', express.static(thumbnailsDir));
app.use('/photos', express.static(photosDir));

// Routes
const authRoutes = require('./routes/auth');
const adminRoutes = require('./routes/admin');

app.use('/api/auth', authRoutes);
app.use('/api/admin', adminRoutes);

// Serve frontend static files
const frontendDistPath = path.join(__dirname, '../frontend/dist');
// Les fichiers de /assets portent un hash dans leur nom : un an de cache, sans revalidation. index.html, lui, se revalide toujours
// (il désigne les fichiers de la version courante, donc une mise à jour de l'app est vue au prochain chargement).
app.use(express.static(frontendDistPath, {
  setHeaders(res, filePath) {
    if (path.relative(frontendDistPath, filePath).split(path.sep)[0] === 'assets') res.setHeader('Cache-Control', 'public, max-age=31536000, immutable');
    else if (filePath.endsWith('.html')) res.setHeader('Cache-Control', 'no-cache');
  }
}));

app.get(/.*/, (req, res) => {
  res.setHeader('Cache-Control', 'no-cache');
  res.sendFile(path.join(frontendDistPath, 'index.html'));
});

// Global error handler
app.use((err, req, res, next) => {
  console.error('Erreur non gérée :', err);
  const message = isProd ? 'Une erreur inattendue est survenue sur le serveur.' : err.message;
  res.status(err.status || 500).send({ message });
});

const server = app.listen(PORT, HOST, () => {
  console.log(`Serveur démarré sur http://localhost:${PORT} (écoute sur ${HOST}).`);
});
server.on('upgrade', (req, socket, head) => {
  if (!liveProxy.upgrade(req, socket, head)) socket.destroy();
});
