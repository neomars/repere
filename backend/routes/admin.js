const express = require('express');
const router = express.Router();
const bcrypt = require('bcrypt');
const { pool: dbPool } = require('../db');
const { isAuthenticated } = require('../middleware/auth');

router.get('/users', isAuthenticated, async (req, res) => {
  try {
    const [rows] = await dbPool.query('SELECT id, username FROM users');
    res.send(rows);
  } catch (err) {
    console.error('Fetch users error:', err);
    res.status(500).send({ message: 'Erreur lors de la récupération des utilisateurs.' });
  }
});

router.post('/users', isAuthenticated, async (req, res) => {
  const { username, password } = req.body;
  try {
    const hash = await bcrypt.hash(password, 10);
    const [result] = await dbPool.query('INSERT INTO users (username, password_hash) VALUES (?, ?)', [username, hash]);
    res.status(201).send({ id: result.insertId, username });
  } catch (err) {
    console.error('Create user error:', err);
    res.status(500).send({ message: 'Erreur lors de la création de l\'utilisateur.' });
  }
});

router.put('/users/:id', isAuthenticated, async (req, res) => {
  const { username, password } = req.body;
  try {
    if (password) {
      const hash = await bcrypt.hash(password, 10);
      await dbPool.query('UPDATE users SET username = ?, password_hash = ? WHERE id = ?', [username, hash, req.params.id]);
    } else {
      await dbPool.query('UPDATE users SET username = ? WHERE id = ?', [username, req.params.id]);
    }
    res.send({ message: 'Utilisateur mis à jour.' });
  } catch (err) {
    console.error('Update user error:', err);
    res.status(500).send({ message: 'Erreur lors de la mise à jour de l\'utilisateur.' });
  }
});

router.delete('/users/:id', isAuthenticated, async (req, res) => {
  try {
    if (parseInt(req.params.id) === req.session.userId) {
      return res.status(400).send({ message: 'Vous ne pouvez pas supprimer votre propre compte.' });
    }
    await dbPool.query('DELETE FROM users WHERE id = ?', [req.params.id]);
    res.send({ message: 'Utilisateur supprimé.' });
  } catch (err) {
    console.error('Delete user error:', err);
    res.status(500).send({ message: 'Erreur lors de la suppression de l\'utilisateur.' });
  }
});

router.post('/change-password', isAuthenticated, async (req, res) => {
  const { oldPassword, newPassword } = req.body;
  try {
    const [rows] = await dbPool.query('SELECT * FROM users WHERE id = ?', [req.session.userId]);
    const user = rows[0];
    const match = await bcrypt.compare(oldPassword, user.password_hash);
    if (!match) return res.status(401).send({ message: 'Ancien mot de passe incorrect.' });
    const hash = await bcrypt.hash(newPassword, 10);
    await dbPool.query('UPDATE users SET password_hash = ? WHERE id = ?', [hash, req.session.userId]);
    res.send({ message: 'Mot de passe mis à jour avec succès.' });
  } catch (err) {
    console.error('Change password error:', err);
    res.status(500).send({ message: 'Erreur lors du changement de mot de passe.' });
  }
});

module.exports = router;
