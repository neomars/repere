const http = require('http');
const net = require('net');

// Relaie /api/live/* (REST + WebSocket) vers le moteur Rust, pour que l'app n'expose qu'un seul port.
const TARGET = { host: '127.0.0.1', port: Number(process.env.LIVE_PORT || 3001) };

const isLive = (url = '') => /^\/api\/live(?:[/?]|$)/.test(url);

function proxyHttp(req, res) {
  const headers = { ...req.headers, host: `${TARGET.host}:${TARGET.port}` };
  const upstream = http.request(
    { ...TARGET, method: req.method, path: req.originalUrl || req.url, headers },
    (up) => {
      res.writeHead(up.statusCode, up.headers);
      up.pipe(res);
    }
  );
  upstream.on('error', () => {
    if (!res.headersSent) res.status(502).send({ message: 'Moteur live injoignable (live-engine démarré ?)' });
    else res.end();
  });
  req.pipe(upstream);
}

const READ_ONLY = new Set(['GET', 'HEAD', 'OPTIONS']);

// Middleware Express : à placer APRÈS la session et AVANT express.json() (le corps doit rester un flux).
// Lecture (profils d'assistant, état…) ouverte ; toute écriture (télécharger/supprimer un modèle,
// charger l'IA, éditer médias et profils) exige la même connexion que l'administration.
function middleware(req, res, next) {
  if (!isLive(req.originalUrl || req.url)) return next();
  if (!READ_ONLY.has(req.method) && !(req.session && req.session.userId)) {
    return res.status(401).send({ message: 'Non autorisé. Veuillez vous connecter.' });
  }
  proxyHttp(req, res);
}

// Seule vraie poignée de main WebSocket vers /api/live/ws est relayée. Node émet « upgrade » pour TOUTE requête portant
// `Connection: Upgrade` — y compris un DELETE ou un PUT /api/live/personas/… : ce chemin ne passe pas par le middleware
// (donc ni session ni contrôle d'écriture), il ne doit jamais devenir un moyen de contourner la connexion admin.
const isWsUpgrade = (req) =>
  req.method === 'GET' &&
  /^\/api\/live\/ws(?:\?|$)/.test(req.url || '') &&
  String(req.headers.upgrade || '').toLowerCase() === 'websocket';

// À brancher sur server.on('upgrade', ...) ; renvoie true si la requête a été prise en charge.
function upgrade(req, socket, head) {
  if (!isLive(req.url)) return false;
  if (!isWsUpgrade(req)) {
    socket.end('HTTP/1.1 400 Bad Request\r\nConnection: close\r\n\r\n');
    return true;
  }
  const up = net.connect(TARGET.port, TARGET.host, () => {
    const lines = [`${req.method} ${req.url} HTTP/${req.httpVersion}`];
    for (let i = 0; i < req.rawHeaders.length; i += 2) {
      const name = req.rawHeaders[i];
      lines.push(`${name}: ${name.toLowerCase() === 'host' ? `${TARGET.host}:${TARGET.port}` : req.rawHeaders[i + 1]}`);
    }
    up.write(lines.join('\r\n') + '\r\n\r\n');
    if (head && head.length) up.write(head);
    socket.pipe(up);
    up.pipe(socket);
  });
  up.on('error', () => socket.destroy());
  socket.on('error', () => up.destroy());
  socket.on('close', () => up.destroy());
  return true;
}

module.exports = { middleware, upgrade, isLive, isWsUpgrade };
