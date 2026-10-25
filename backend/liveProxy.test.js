// Tests du proxy du moteur live : node --test backend/liveProxy.test.js
const test = require('node:test');
const assert = require('node:assert');
const { upgrade, isWsUpgrade, isLive } = require('./liveProxy');

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
