// Tests du proxy du moteur live : node --test backend/liveProxy.test.js
const test = require('node:test');
const assert = require('node:assert');
const { upgrade, isWsUpgrade, isLive, needsLogin, middleware } = require('./liveProxy');

const req = (method, url, headers = {}) => ({ method, url, headers, rawHeaders: [], httpVersion: '1.1' });
const fakeSocket = () => {
  const s = { ended: null, destroyed: false };
  s.end = (data) => { s.ended = String(data); };
  s.destroy = () => { s.destroyed = true; };
  s.pipe = () => s;
  s.on = () => s;
  return s;
};

test('seule une poignée de main WebSocket GET /api/live/ws est relayée', () => {
  const ws = { upgrade: 'websocket', connection: 'Upgrade' };
  assert.ok(isWsUpgrade(req('GET', '/api/live/ws', ws)));
  assert.ok(isWsUpgrade(req('GET', '/api/live/ws?x=1', { upgrade: 'WebSocket' })));
  assert.ok(!isWsUpgrade(req('DELETE', '/api/live/personas/abc', ws)));
  assert.ok(!isWsUpgrade(req('PUT', '/api/live/personas/x', ws)));
  assert.ok(!isWsUpgrade(req('GET', '/api/live/personas/abc', ws)));
  assert.ok(!isWsUpgrade(req('GET', '/api/live/wsx', ws)));
  assert.ok(!isWsUpgrade(req('GET', '/api/live/ws', { connection: 'Upgrade' })), 'sans en-tête Upgrade: websocket');
});

test("une écriture déguisée en « upgrade » est refusée sans toucher au moteur", () => {
  const socket = fakeSocket();
  const handled = upgrade(req('DELETE', '/api/live/personas/abc', { upgrade: 'websocket', connection: 'Upgrade' }), socket, Buffer.alloc(0));
  assert.strictEqual(handled, true);
  assert.match(socket.ended, /^HTTP\/1\.1 400/);
});

test('les autres chemins ne sont pas pris en charge ici', () => {
  assert.strictEqual(isLive('/api/auth/me'), false);
  assert.strictEqual(upgrade(req('GET', '/api/auth/me', { upgrade: 'websocket' }), fakeSocket(), Buffer.alloc(0)), false);
});

test('la connexion est exigée pour toute écriture, pas pour la lecture', () => {
  for (const m of ['PUT', 'POST', 'PATCH', 'DELETE']) assert.strictEqual(needsLogin(m, '/api/live/kb/documents'), true, m);
  assert.strictEqual(needsLogin('POST', '/api/live/kb/import'), true);
  assert.strictEqual(needsLogin('PUT', '/api/live/kb/documents?name=guide.md'), true);
  for (const m of ['GET', 'HEAD', 'OPTIONS']) assert.strictEqual(needsLogin(m, '/api/live/kb/passages/12'), false, m);
  assert.strictEqual(needsLogin('GET', '/api/live/personas'), false);
});

test("n'importe qui peut demander un ticket, mais seule l'administration les lit et les suit", () => {
  assert.strictEqual(needsLogin('POST', '/api/live/tickets'), false, 'demande depuis la conversation');
  assert.strictEqual(needsLogin('POST', '/api/live/tickets?x=1'), false);
  assert.strictEqual(needsLogin('GET', '/api/live/tickets'), true, 'la liste contient des résumés de conversations');
  assert.strictEqual(needsLogin('GET', '/api/live/tickets/3'), true);
  assert.strictEqual(needsLogin('PATCH', '/api/live/tickets/3'), true);
  assert.strictEqual(needsLogin('DELETE', '/api/live/tickets'), true);
  assert.strictEqual(needsLogin('POST', '/api/live/tickets/3'), true, 'seul POST sur la collection est ouvert');
});

test("les variantes d'écriture du chemin des tickets ne contournent pas la connexion", () => {
  for (const url of ['/api/live/TICKETS', '/api/live/tickets/', '/api/live//tickets', '/api/live/%74ickets', '/api/live/tickets#x', '/api/live/tickets?a=b']) {
    assert.strictEqual(needsLogin('GET', url), true, url);
  }
  assert.strictEqual(needsLogin('GET', '/api/live/%E0%A4%A'), true, 'URL mal formée : refusée');
});

test('le middleware répond 401 sans session et ignore les chemins hors du moteur', () => {
  const res = () => {
    const r = { code: null, body: null };
    r.status = (c) => { r.code = c; return r; };
    r.send = (b) => { r.body = b; return r; };
    return r;
  };
  const anonymous = { method: 'GET', url: '/api/live/tickets', originalUrl: '/api/live/tickets', session: {} };
  const r1 = res();
  let passed = false;
  middleware(anonymous, r1, () => { passed = true; });
  assert.strictEqual(r1.code, 401);
  assert.strictEqual(passed, false);
  // hors /api/live : le middleware passe la main sans rien faire
  const r2 = res();
  middleware({ method: 'GET', url: '/api/auth/me', originalUrl: '/api/auth/me' }, r2, () => { passed = true; });
  assert.strictEqual(passed, true);
  assert.strictEqual(r2.code, null);
});
