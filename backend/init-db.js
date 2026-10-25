
const bcrypt = require('bcrypt');
const { pool } = require('./db');

async function initializeDatabase() {
  try {
    console.log('Initializing JSON database...');

    // Insérer l'utilisateur admin par défaut si absent
    const [userRows] = await pool.query("SELECT * FROM users WHERE username = 'admin'");
    if (userRows.length === 0) {
      console.log('Création de l\'utilisateur admin par défaut...');
      const saltRounds = 10;
      const hash = await bcrypt.hash('admin', saltRounds);
      await pool.query("INSERT INTO users (username, password_hash) VALUES (?, ?)", ['admin', hash]);
    }

    console.log('JSON database initialized successfully.');

  } catch (error) {
    console.error("Erreur lors de l'initialisation de la base de données:", error.message);
    process.exit(1);
  } finally {
    // pool.end() is not strictly needed here if we want to keep it simple, but let's call it to be sure
    await pool.end();
  }
}

initializeDatabase();
