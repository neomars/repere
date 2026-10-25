const express = require('express');
const router = express.Router();
const bcrypt = require('bcrypt');
const { pool: dbPool } = require('../db');
const { loginLimiter } = require('../middleware/rateLimiter');

router.post('/login', loginLimiter, async (req, res) => {
  const { username, password } = req.body;
  try {
    const [rows] = await dbPool.query('SELECT * FROM users WHERE username = ?', [username]);
    if (rows.length === 0) {
      return res.status(401).send({ message: 'Identifiants invalides.' });
    }
    const user = rows[0];
    const match = await bcrypt.compare(password, user.password_hash);
    if (!match) {
      return res.status(401).send({ message: 'Identifiants invalides.' });
    }
    req.session.userId = user.id;
    req.session.username = user.username;
    res.send({ id: user.id, username: user.username });
  } catch (err) {
    console.error('Login error:', err);
    res.status(500).send({ message: 'Erreur lors de la connexion.' });
  }
});

router.post('/logout', (req, res) => {
  req.session.destroy(err => {
    if (err) {
      return res.status(500).send({ message: 'Erreur lors de la déconnexion.' });
    }
    res.clearCookie('repere.sid');
    res.send({ message: 'Déconnecté avec succès.' });
  });
});

router.get('/me', (req, res) => {
  if (req.session && req.session.userId) {
    res.send({ id: req.session.userId, username: req.session.username });
  } else {
    res.status(401).send({ message: 'Non authentifié.' });
  }
});

module.exports = router;
