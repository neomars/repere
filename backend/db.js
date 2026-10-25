const alasql = require('alasql');
const fs = require('fs');
const path = require('path');

const DB_FILE = require('./utils/paths').dbFile;

// Helper to save to JSON
function save() {
    const data = {};
    Object.keys(alasql.tables).forEach(table => {
        data[table] = alasql.tables[table].data;
    });
    fs.writeFileSync(DB_FILE, JSON.stringify(data, null, 2));
}

// Initialize tables
const initSql = `
CREATE TABLE IF NOT EXISTS users (id INT AUTO_INCREMENT PRIMARY KEY, username STRING, password_hash STRING);
`;

initSql.split(';').filter(s => s.trim()).forEach(sql => {
    try {
        alasql(sql);
    } catch (e) {
        console.error("Error creating table:", e.message, "SQL:", sql);
    }
});

// Load data if exists
if (fs.existsSync(DB_FILE)) {
    try {
        const data = JSON.parse(fs.readFileSync(DB_FILE, 'utf8'));
        Object.keys(data).forEach(table => {
            if (alasql.tables[table]) {
                alasql.tables[table].data = data[table];
                // Update AUTO_INCREMENT counter to avoid ID collisions
                if (data[table].length > 0 && alasql.tables[table].identities && alasql.tables[table].identities.id) {
                    const maxId = Math.max(...data[table].map(r => parseInt(r.id) || 0));
                    alasql.tables[table].identities.id.value = maxId + 1;
                }
            }
        });
    } catch (e) {
        console.error("Error loading db.json:", e);
    }
}

const pool = {
    async query(sql, params = []) {
        let trimmedSql = sql.trim();
        let upperSql = trimmedSql.toUpperCase();
        let processedParams = [...params];

        // Preprocess SQL and params for Alasql compatibility
        // 1. Handle "IN (?)" with array parameters (common in mysql2)
        if (processedParams.some(p => Array.isArray(p))) {
            let newParams = [];
            let paramIdx = 0;
            trimmedSql = trimmedSql.replace(/\?/g, (match) => {
                const currentParam = processedParams[paramIdx++];
                if (Array.isArray(currentParam)) {
                    newParams.push(...currentParam);
                    return currentParam.map(() => '?').join(', ');
                } else {
                    newParams.push(currentParam);
                    return '?';
                }
            });
            processedParams = newParams;
            upperSql = trimmedSql.toUpperCase();
        }

        // 2. Cast string numbers to INT if they look like IDs and the param is a string
        // Alasql is strict about types in WHERE clauses
        processedParams = processedParams.map(p => {
            if (typeof p === 'string' && /^\d+$/.test(p)) {
                return parseInt(p, 10);
            }
            return p;
        });

        try {
            const result = alasql(trimmedSql, processedParams);

            // Save on writes
            if (trimmedSql.match(/^(INSERT|UPDATE|DELETE|CREATE|DROP|ALTER)/i)) {
                save();
            }

            // Mock mysql2 return format for INSERT
            if (upperSql.startsWith('INSERT')) {
                const tableNameMatch = trimmedSql.match(/INSERT INTO\s+([^\s(]+)/i);
                const tableName = tableNameMatch ? tableNameMatch[1].replace(/`/g, '') : null;
                let insertId = 0;
                if (tableName && alasql.tables[tableName] && alasql.tables[tableName].identities && alasql.tables[tableName].identities.id) {
                    insertId = alasql.tables[tableName].identities.id.value - (alasql.tables[tableName].identities.id.step || 1);
                }
                return [{ insertId }, []];
            }

            // SELECT and other queries return result as rows
            return [result, []];
        } catch (err) {
            console.error('Alasql error:', err.message, 'SQL:', sql);
            throw err;
        }
    },
    async getConnection() {
        return {
            query: this.query.bind(this),
            release: () => {}
        };
    },
    async end() {
        save();
    }
};

module.exports = {
    pool,
    dbConfig: {
        database: 'json_db'
    }
};
