//! Médiathèque : fichiers annotés (titre, description, tags) dans SQLite, et moteur de recherche que le moteur interroge à chaque
//! tour pour proposer à l'IA les sources les plus proches de la conversation.

use crate::terms::terms;
use crate::thesaurus::Thesaurus;
use rusqlite::{params, Connection, Row};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MediaItem {
    pub id: i64,
    pub kind: String, // "video" | "photo"
    pub url: String,
    pub thumb: Option<String>,
    pub title: String,
    pub description: String,
    pub tags: Vec<String>,
    /// Décrit par l'analyse d'image (description et tags remplis par un modèle de vision).
    #[serde(default)]
    pub analyzed: bool,
}

impl Default for MediaItem {
    fn default() -> Self {
        Self { id: 0, kind: "video".into(), url: String::new(), thumb: None, title: String::new(), description: String::new(), tags: vec![], analyzed: false }
    }
}

/// Résultat de l'analyse d'une image par un modèle de vision.
#[derive(Debug, Clone, Default)]
pub struct Analysis {
    pub description: String,
    pub tags: Vec<String>,
}

#[derive(Debug, Default, Clone, Deserialize)]
pub struct MediaPatch {
    pub title: Option<String>,
    pub description: Option<String>,
    pub tags: Option<Vec<String>>,
    /// Tags à ajouter à ceux qui existent déjà (annotation en lot).
    pub add_tags: Option<Vec<String>>,
}

/// Recherche de l'écran de la médiathèque (pagination côté base : des milliers de fichiers ne se chargent pas d'un coup).
#[derive(Debug, Default, Clone)]
pub struct MediaFilter {
    pub text: String,
    pub kind: Option<String>,
    /// Fichiers sans aucun tag (à annoter).
    pub untagged: bool,
    /// `todo` (pas encore analysés) ou `done` (analysés).
    pub analysis: Option<String>,
    pub offset: usize,
    pub limit: usize,
}

pub struct MediaLibrary {
    conn: Mutex<Connection>,
    /// Change à chaque modification de la base : l'index de recherche se reconstruit alors à la prochaine recherche.
    generation: AtomicU64,
    index: Mutex<Option<Arc<Index>>>,
    /// Fichier de synonymes de l'utilisateur (`synonyms.txt`, à côté de la base) : relu dès qu'il change.
    synonyms_file: Option<PathBuf>,
    thesaurus: Mutex<Option<(Option<SystemTime>, Arc<Thesaurus>)>>,
}

/// Un fichier prêt à être comparé à un texte : ses mots utiles (tags, titre, description).
struct Doc {
    item: MediaItem,
    words: HashSet<String>,
}

/// Les fichiers de la médiathèque et la rareté de chaque mot : un mot présent dans toutes les descriptions ne départage rien.
struct Index {
    generation: u64,
    /// Version du fichier de synonymes de l'utilisateur avec lequel l'index a été construit.
    stamp: Option<SystemTime>,
    /// Le dictionnaire de synonymes en vigueur. Pas encore consulté par `relevant()` : il le sera pour élargir la requête (phase 2),
    /// l'index se reconstruit déjà quand le fichier de l'utilisateur change.
    #[allow(dead_code)]
    thesaurus: Arc<Thesaurus>,
    docs: Vec<Doc>,
    df: HashMap<String, u32>,
}

impl Index {
    fn build(generation: u64, stamp: Option<SystemTime>, thesaurus: Arc<Thesaurus>, items: Vec<MediaItem>) -> Self {
        let mut docs = Vec::with_capacity(items.len());
        let mut df: HashMap<String, u32> = HashMap::new();
        for item in items {
            let mut words: HashSet<String> = HashSet::new();
            words.extend(item.tags.iter().flat_map(|t| terms(t)));
            words.extend(terms(&item.title));
            words.extend(terms(&item.description));
            for w in &words {
                *df.entry(w.clone()).or_default() += 1;
            }
            docs.push(Doc { words, item });
        }
        Self { generation, stamp, thesaurus, docs, df }
    }

    /// Poids d'un mot : fort s'il est rare dans la médiathèque, faible (mais jamais nul) s'il est partout.
    fn idf(&self, word: &str) -> f64 {
        let n = self.docs.len() as f64;
        let df = self.df.get(word).copied().unwrap_or(0) as f64;
        ((n + 1.0) / (df + 0.5)).ln().max(0.1)
    }
}

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS media (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  kind TEXT NOT NULL,
  url TEXT NOT NULL UNIQUE,
  thumb TEXT,
  title TEXT NOT NULL DEFAULT '',
  description TEXT NOT NULL DEFAULT '',
  tags TEXT NOT NULL DEFAULT '',
  src_key TEXT,
  analyzed_at INTEGER,
  analysis_model TEXT
);
";

/// Colonnes ajoutées après coup : une base plus ancienne les reçoit au démarrage (l'erreur « colonne déjà là » est ignorée).
/// Les colonnes d'une base plus ancienne qui ne servent plus y restent, sans effet : elles ont toutes une valeur par défaut.
const ADDED_COLUMNS: [&str; 3] = ["src_key TEXT", "analyzed_at INTEGER", "analysis_model TEXT"];

/// Empreinte du fichier d'origine (anti-doublons) : index unique.
const MIGRATE: &str = "CREATE UNIQUE INDEX IF NOT EXISTS media_src_key ON media(src_key) WHERE src_key IS NOT NULL;";

const COLS: &str = "id,kind,url,thumb,title,description,tags,analyzed_at";

fn split_tags(s: &str) -> Vec<String> {
    s.split(',').map(|t| t.trim().to_lowercase()).filter(|t| !t.is_empty()).collect()
}

fn from_row(r: &Row) -> rusqlite::Result<MediaItem> {
    Ok(MediaItem {
        id: r.get(0)?,
        kind: r.get(1)?,
        url: r.get(2)?,
        thumb: r.get(3)?,
        title: r.get(4)?,
        description: r.get(5)?,
        tags: split_tags(&r.get::<_, String>(6)?),
        analyzed: r.get::<_, Option<i64>>(7)?.is_some(),
    })
}

impl MediaLibrary {
    pub fn open(path: &Path) -> anyhow::Result<Self> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let conn = Connection::open(path)?;
        conn.execute_batch("PRAGMA journal_mode=WAL;")?;
        Self::init(&conn)?;
        let mut lib = Self::with_conn(conn);
        lib.synonyms_file = path.parent().map(|d| d.join("synonyms.txt"));
        Ok(lib)
    }

    #[cfg(test)]
    pub fn open_in_memory() -> anyhow::Result<Self> {
        let conn = Connection::open_in_memory()?;
        Self::init(&conn)?;
        Ok(Self::with_conn(conn))
    }

    fn with_conn(conn: Connection) -> Self {
        Self { conn: Mutex::new(conn), generation: AtomicU64::new(1), index: Mutex::new(None), synonyms_file: None, thesaurus: Mutex::new(None) }
    }

    /// Le dictionnaire de synonymes : celui de base, complété par le fichier de l'utilisateur, relu s'il a changé.
    fn thesaurus(&self) -> (Option<SystemTime>, Arc<Thesaurus>) {
        let stamp = self.synonyms_file.as_ref().and_then(|p| std::fs::metadata(p).ok()).and_then(|m| m.modified().ok());
        let mut slot = self.thesaurus.lock().unwrap();
        if let Some((s, th)) = slot.as_ref().filter(|(s, _)| *s == stamp) {
            return (*s, th.clone());
        }
        let extra = self.synonyms_file.as_ref().and_then(|p| std::fs::read_to_string(p).ok()).unwrap_or_default();
        let th = Arc::new(Thesaurus::with_extra(&extra));
        *slot = Some((stamp, th.clone()));
        (stamp, th)
    }

    /// À appeler après toute modification de la base.
    fn touch(&self) {
        self.generation.fetch_add(1, Ordering::AcqRel);
    }

    /// L'index de recherche à jour (reconstruit seulement si la base a changé depuis la dernière fois).
    fn index(&self) -> Arc<Index> {
        let generation = self.generation.load(Ordering::Acquire);
        let (stamp, thesaurus) = self.thesaurus();
        let mut slot = self.index.lock().unwrap();
        if let Some(ix) = slot.as_ref().filter(|ix| ix.generation == generation && ix.stamp == stamp) {
            return ix.clone();
        }
        let ix = Arc::new(Index::build(generation, stamp, thesaurus, self.list().unwrap_or_default()));
        *slot = Some(ix.clone());
        ix
    }

    fn init(conn: &Connection) -> anyhow::Result<()> {
        conn.execute_batch(SCHEMA)?;
        for col in ADDED_COLUMNS {
            let _ = conn.execute(&format!("ALTER TABLE media ADD COLUMN {col}"), []); // erreur ignorée si la colonne existe déjà
        }
        conn.execute_batch(MIGRATE)?;
        Ok(())
    }

    /// Ajoute un fichier ; `None` s'il existe déjà (même URL, ou même empreinte de fichier d'origine).
    pub fn insert_new(&self, m: &MediaItem, src_key: Option<&str>) -> anyhow::Result<Option<i64>> {
        let c = self.conn.lock().unwrap();
        let n = c.execute(
            "INSERT OR IGNORE INTO media (kind,url,thumb,title,description,tags,src_key)
             VALUES (?1,?2,?3,?4,?5,?6,?7)",
            params![m.kind, m.url, m.thumb, m.title, m.description, m.tags.join(","), src_key],
        )?;
        self.touch();
        Ok((n > 0).then(|| c.last_insert_rowid()))
    }

    pub fn has_src_key(&self, key: &str) -> bool {
        let c = self.conn.lock().unwrap();
        c.query_row("SELECT 1 FROM media WHERE src_key=?1", [key], |_| Ok(())).is_ok()
    }

    /// Une page de fichiers (les plus récents d'abord) et le nombre total correspondant au filtre.
    pub fn query(&self, f: &MediaFilter) -> anyhow::Result<(usize, Vec<MediaItem>)> {
        let mut conds: Vec<String> = vec![];
        let mut args: Vec<String> = vec![];
        for w in f.text.split_whitespace().take(6) {
            // chaque mot doit se trouver dans le titre, les tags ou la description
            let like = format!("%{}%", w.to_lowercase().replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_"));
            args.push(like);
            let i = args.len();
            conds.push(format!("(lower(title) LIKE ?{i} ESCAPE '\\' OR lower(tags) LIKE ?{i} ESCAPE '\\' OR lower(description) LIKE ?{i} ESCAPE '\\')"));
        }
        if let Some(k) = &f.kind {
            args.push(k.clone());
            conds.push(format!("kind=?{}", args.len()));
        }
        if f.untagged {
            conds.push("tags=''".into());
        }
        match f.analysis.as_deref() {
            Some("todo") => conds.push("analyzed_at IS NULL".into()),
            Some("done") => conds.push("analyzed_at IS NOT NULL".into()),
            _ => {}
        }
        let wh = if conds.is_empty() { String::new() } else { format!("WHERE {}", conds.join(" AND ")) };
        let c = self.conn.lock().unwrap();
        let total: i64 = c.query_row(&format!("SELECT COUNT(*) FROM media {wh}"), rusqlite::params_from_iter(args.iter()), |r| r.get(0))?;
        let mut st = c.prepare(&format!("SELECT {COLS} FROM media {wh} ORDER BY id DESC LIMIT {} OFFSET {}", f.limit.clamp(1, 500), f.offset))?;
        let rows = st.query_map(rusqlite::params_from_iter(args.iter()), from_row)?.collect::<Result<Vec<_>, _>>()?;
        Ok((total as usize, rows))
    }

    /// Fichiers à analyser, du plus ancien au plus récent : `new` = jamais analysés, `all` = tous, `ids` = la sélection.
    /// `kind` limite aux vidéos ou aux photos.
    pub fn to_analyze(&self, scope: &str, ids: &[i64], kind: Option<&str>, limit: Option<usize>) -> anyhow::Result<Vec<MediaItem>> {
        let mut conds: Vec<String> = vec![];
        match scope {
            "ids" => {
                if ids.is_empty() {
                    return Ok(vec![]);
                }
                conds.push(format!("id IN ({})", ids.iter().map(|i| i.to_string()).collect::<Vec<_>>().join(",")));
            }
            "all" => {}
            _ => conds.push("analyzed_at IS NULL".into()),
        }
        let mut args: Vec<String> = vec![];
        if let Some(k) = kind {
            args.push(k.to_string());
            conds.push(format!("kind=?{}", args.len()));
        }
        let wh = if conds.is_empty() { String::new() } else { format!("WHERE {}", conds.join(" AND ")) };
        let lim = limit.map(|l| format!(" LIMIT {}", l.min(1_000_000))).unwrap_or_default();
        let c = self.conn.lock().unwrap();
        let mut st = c.prepare(&format!("SELECT {COLS} FROM media {wh} ORDER BY id{lim}"))?;
        let rows = st.query_map(rusqlite::params_from_iter(args.iter()), from_row)?.collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// (images, analysées) : de quoi afficher « 120 / 4 500 images analysées ». Les vidéos ne sont pas analysées : pas comptées.
    pub fn analysis_counts(&self) -> anyhow::Result<(usize, usize)> {
        let c = self.conn.lock().unwrap();
        Ok(c.query_row(
            "SELECT COUNT(*), COALESCE(SUM(analyzed_at IS NOT NULL),0) FROM media WHERE kind='photo'",
            [],
            |r| Ok((r.get::<_, i64>(0)? as usize, r.get::<_, i64>(1)? as usize)),
        )?)
    }

    /// Enregistre le résultat d'une analyse. Sans `overwrite`, ce que vous avez réglé est respecté : les tags s'ajoutent à ceux qui
    /// existent déjà ; avec, ils sont remplacés. La description est toujours remplacée.
    pub fn apply_analysis(&self, id: i64, a: &Analysis, model: &str, overwrite: bool) -> anyhow::Result<Option<MediaItem>> {
        let Some(mut m) = self.get(id) else { return Ok(None) };
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
        m.analyzed = true;
        m.description = a.description.clone();
        if overwrite {
            m.tags = a.tags.clone();
        } else {
            for t in &a.tags {
                if !m.tags.contains(t) {
                    m.tags.push(t.clone());
                }
            }
        }
        m.tags.truncate(30);
        let c = self.conn.lock().unwrap();
        c.execute(
            "UPDATE media SET description=?2,tags=?3,analyzed_at=?4,analysis_model=?5 WHERE id=?1",
            params![id, m.description, m.tags.join(","), now, model],
        )?;
        self.touch();
        Ok(Some(m))
    }

    /// Un fichier de la médiathèque utilise-t-il encore ce fichier (comme original ou comme miniature) ?
    pub fn url_used(&self, url: &str) -> bool {
        let c = self.conn.lock().unwrap();
        c.query_row("SELECT 1 FROM media WHERE url=?1 OR thumb=?1", [url], |_| Ok(())).is_ok()
    }

    /// Retire des fichiers de la médiathèque (les fichiers ne sont pas effacés : voir `files::remove_media_files`).
    pub fn delete_many(&self, ids: &[i64]) -> anyhow::Result<usize> {
        let c = self.conn.lock().unwrap();
        let mut n = 0;
        for id in ids {
            n += c.execute("DELETE FROM media WHERE id=?1", [id])?;
        }
        self.touch();
        Ok(n)
    }

    #[cfg(test)]
    pub fn insert(&self, m: &MediaItem) -> anyhow::Result<i64> {
        let c = self.conn.lock().unwrap();
        c.execute(
            "INSERT OR IGNORE INTO media (kind,url,thumb,title,description,tags)
             VALUES (?1,?2,?3,?4,?5,?6)",
            params![m.kind, m.url, m.thumb, m.title, m.description, m.tags.join(",")],
        )?;
        self.touch();
        Ok(c.query_row("SELECT id FROM media WHERE url=?1", [&m.url], |r| r.get(0))?)
    }

    pub fn get(&self, id: i64) -> Option<MediaItem> {
        let c = self.conn.lock().unwrap();
        c.query_row(&format!("SELECT {COLS} FROM media WHERE id=?1"), [id], from_row).ok()
    }

    pub fn list(&self) -> anyhow::Result<Vec<MediaItem>> {
        let c = self.conn.lock().unwrap();
        let mut st = c.prepare(&format!("SELECT {COLS} FROM media ORDER BY id"))?;
        let rows = st.query_map([], from_row)?.collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn patch(&self, id: i64, p: &MediaPatch) -> anyhow::Result<Option<MediaItem>> {
        let Some(mut m) = self.get(id) else { return Ok(None) };
        if let Some(v) = &p.title { m.title = v.clone(); }
        if let Some(v) = &p.description { m.description = v.clone(); }
        if let Some(v) = &p.tags { m.tags = v.iter().map(|t| t.trim().to_lowercase()).filter(|t| !t.is_empty()).collect(); }
        if let Some(v) = &p.add_tags {
            for t in v.iter().map(|t| t.trim().to_lowercase()).filter(|t| !t.is_empty()) {
                if !m.tags.contains(&t) { m.tags.push(t); }
            }
        }
        let c = self.conn.lock().unwrap();
        c.execute(
            "UPDATE media SET title=?2,description=?3,tags=?4 WHERE id=?1",
            params![id, m.title, m.description, m.tags.join(",")],
        )?;
        self.touch();
        Ok(Some(m))
    }

    /// Les fichiers dont le texte rappelle la conversation en cours, du plus pertinent au moins pertinent : de quoi proposer à l'IA
    /// des sources pour sa réponse. `parts` = textes de la conversation avec leur poids (le dernier message compte plus que le
    /// résumé). Un fichier doit partager avec elle au moins deux mots utiles (ou un seul, rare et dit à l'instant), pesés par leur
    /// rareté dans la médiathèque ; ceux de `exclude` (déjà montrés) sont écartés.
    pub fn relevant(&self, parts: &[(String, f64)], exclude: &HashSet<i64>, limit: usize) -> Vec<MediaItem> {
        let ix = self.index();
        if ix.docs.is_empty() || limit == 0 {
            return vec![];
        }
        let mut weights: HashMap<String, f64> = HashMap::new();
        for (text, w) in parts {
            for t in terms(text) {
                let e = weights.entry(t).or_insert(0.0);
                *e = e.max(*w);
            }
        }
        if weights.is_empty() {
            return vec![];
        }
        let threshold = (0.9 * ((ix.docs.len() + 1) as f64).ln()).min(3.0);
        // Un mot présent dans peu de fichiers, et dit dans le dernier message, suffit à lui seul.
        let rare = (ix.docs.len() / 20).max(1) as u32;
        let mut scored: Vec<(f64, &Doc)> = vec![];
        for d in &ix.docs {
            if exclude.contains(&d.item.id) {
                continue;
            }
            let (mut score, mut hits, mut strong) = (0.0, 0, false);
            for (word, w) in &weights {
                if d.words.contains(word) {
                    score += w * ix.idf(word);
                    hits += 1;
                    strong |= *w >= 0.9 && ix.df.get(word).is_some_and(|df| *df <= rare);
                }
            }
            // deux mots en commun doivent peser assez (pas seulement des mots courants) ; un seul, il doit être rare
            if (hits >= 2 && score < threshold) || (hits < 2 && !strong) {
                continue;
            }
            scored.push((score, d));
        }
        scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal).then(b.1.item.id.cmp(&a.1.item.id)));
        scored.into_iter().take(limit).map(|(_, d)| d.item.clone()).collect()
    }

    /// Parcourt le dossier d'uploads et référence les fichiers inconnus (vidéos ET photos).
    pub fn scan_uploads(&self, uploads: &Path) -> anyhow::Result<usize> {
        fn walk(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
            let Ok(rd) = std::fs::read_dir(dir) else { return };
            for e in rd.flatten() {
                let p = e.path();
                if p.is_dir() { walk(&p, out); } else { out.push(p); }
            }
        }
        let mut files = vec![];
        walk(uploads, &mut files);
        let mut n = 0;
        for f in files {
            let ext = f.extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase();
            let kind = match ext.as_str() {
                "mp4" | "webm" | "mov" | "m4v" | "mkv" => "video",
                "jpg" | "jpeg" | "png" | "webp" | "gif" | "avif" => "photo",
                _ => continue,
            };
            let rel = f.strip_prefix(uploads)?.to_string_lossy().replace('\\', "/");
            // On ignore les miniatures : ce ne sont pas des fichiers de la médiathèque.
            if rel.starts_with("thumbnails/") { continue; }
            let url = format!("/{rel}");
            let title = f.file_stem().and_then(|s| s.to_str()).unwrap_or("").replace(['_', '-'], " ");
            let added = self.insert_new(&MediaItem { id: 0, kind: kind.into(), url, thumb: None, title, ..Default::default() }, None)?;
            if added.is_some() { n += 1; }
        }
        Ok(n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(url: &str, tags: &[&str]) -> MediaItem {
        MediaItem { url: url.into(), title: url.into(), tags: tags.iter().map(|s| s.to_string()).collect(), ..Default::default() }
    }

    /// Un fichier décrit, sans titre (le titre d'un fichier importé ajouterait du bruit).
    fn decrit(url: &str, description: &str, tags: &[&str]) -> MediaItem {
        MediaItem {
            kind: if url.ends_with(".jpg") { "photo" } else { "video" }.into(),
            url: url.into(),
            description: description.into(),
            tags: tags.iter().map(|t| t.to_string()).collect(),
            ..Default::default()
        }
    }

    fn bibliotheque() -> (MediaLibrary, [i64; 4]) {
        let lib = MediaLibrary::open_in_memory().unwrap();
        let a = lib.insert(&decrit("/a.jpg", "Fenêtre de connexion au VPN : le message « Échec de l'authentification » s'affiche après la saisie de l'identifiant.", &["vpn", "authentification"])).unwrap();
        let b = lib.insert(&decrit("/b.jpg", "Boîte de dialogue d'Outlook qui demande de saisir de nouveau les identifiants de la messagerie.", &["outlook", "messagerie"])).unwrap();
        let c = lib.insert(&decrit("/c.jpg", "File d'impression bloquée : le document reste en attente sur l'imprimante du deuxième étage.", &["imprimante", "impression"])).unwrap();
        let d = lib.insert(&decrit("/d.jpg", "Écran bleu de Windows avec le code d'arrêt CRITICAL_PROCESS_DIED.", &["windows", "écran bleu"])).unwrap();
        (lib, [a, b, c, d])
    }

    fn conversation(texte: &str) -> Vec<(String, f64)> {
        vec![(texte.to_string(), 1.0)]
    }

    fn ids(lib: &MediaLibrary, parts: &[(String, f64)], shown: &[i64], limit: usize) -> Vec<i64> {
        let ex: HashSet<i64> = shown.iter().copied().collect();
        lib.relevant(parts, &ex, limit).into_iter().map(|m| m.id).collect()
    }

    #[test]
    fn insert_est_idempotent() {
        let lib = MediaLibrary::open_in_memory().unwrap();
        let a = lib.insert(&item("/a.mp4", &[])).unwrap();
        let b = lib.insert(&item("/a.mp4", &[])).unwrap();
        assert_eq!(a, b);
        assert_eq!(lib.list().unwrap().len(), 1);
    }

    #[test]
    fn recherche_paginee_filtres_et_empreinte() {
        let lib = MediaLibrary::open_in_memory().unwrap();
        for i in 0..7 {
            let mut m = item(&format!("/v{i}.mp4"), &if i % 2 == 0 { vec!["vpn"] } else { vec![] });
            m.title = format!("Guide {i} accès distant");
            if i == 6 { m.kind = "photo".into(); }
            assert!(lib.insert_new(&m, Some(&format!("k{i}"))).unwrap().is_some());
        }
        assert!(lib.insert_new(&item("/autre.mp4", &[]), Some("k3")).unwrap().is_none(), "même empreinte : refusé");
        assert!(lib.has_src_key("k3") && !lib.has_src_key("zz"));
        let f = |text: &str, kind: Option<&str>, untagged: bool, offset, limit| MediaFilter { text: text.into(), kind: kind.map(String::from), untagged, offset, limit, ..Default::default() };
        let (total, page) = lib.query(&f("", None, false, 0, 3)).unwrap();
        assert_eq!((total, page.len()), (7, 3));
        assert_eq!(page[0].url, "/v6.mp4", "les plus récents d'abord");
        let (_, page2) = lib.query(&f("", None, false, 6, 3)).unwrap();
        assert_eq!(page2.len(), 1);
        assert_eq!(lib.query(&f("VPN guide", None, false, 0, 50)).unwrap().0, 4);
        assert_eq!(lib.query(&f("", None, true, 0, 50)).unwrap().0, 3);
        assert_eq!(lib.query(&f("", Some("photo"), false, 0, 50)).unwrap().0, 1);
        assert_eq!(lib.query(&f("100%", None, false, 0, 50)).unwrap().0, 0, "% et _ sont cherchés tels quels");
    }

    #[test]
    fn ajout_de_tags_en_lot_et_retrait_de_la_mediatheque() {
        let lib = MediaLibrary::open_in_memory().unwrap();
        let a = lib.insert(&item("/a.mp4", &["vpn"])).unwrap();
        let b = lib.insert(&item("/b.mp4", &[])).unwrap();
        let patch = MediaPatch { add_tags: Some(vec![" Wi-Fi ".into(), "vpn".into()]), ..Default::default() };
        assert_eq!(lib.patch(a, &patch).unwrap().unwrap().tags, vec!["vpn", "wi-fi"]);
        assert_eq!(lib.patch(b, &patch).unwrap().unwrap().tags, vec!["wi-fi", "vpn"]);
        assert_eq!(lib.delete_many(&[a, 999]).unwrap(), 1);
        assert!(lib.get(a).is_none() && lib.get(b).is_some());
    }

    #[test]
    fn la_description_se_retrouve_malgre_accents_pluriels_et_casse() {
        let (lib, [a, _, c, d]) = bibliotheque();
        // un mot rare suffit, même au pluriel, en majuscules ou sans accent
        assert_eq!(ids(&lib, &conversation("IMPRIMANTES"), &[], 5), vec![c]);
        assert_eq!(ids(&lib, &conversation("ECRAN BLEU"), &[], 5), vec![d]);
        assert_eq!(ids(&lib, &conversation("Mon VPN affiche un échec d'authentification"), &[], 5), vec![a]);
        // rien à voir : rien n'est proposé
        assert!(ids(&lib, &conversation("Bonjour, merci pour votre aide"), &[], 5).is_empty());
        assert!(ids(&lib, &conversation("volcan"), &[], 5).is_empty());
        assert!(ids(&lib, &conversation("   "), &[], 5).is_empty());
    }

    #[test]
    fn sources_pertinentes_pour_la_conversation_en_cours() {
        let (lib, [a, b, ..]) = bibliotheque();
        let vpn = conversation("Mon VPN affiche un échec d'authentification");
        assert_eq!(ids(&lib, &vpn, &[], 5), vec![a]);
        // déjà montré : écarté
        assert!(ids(&lib, &vpn, &[a], 5).is_empty());
        // les messages précédents comptent, moins que le dernier : deux mots en commun suffisent
        let parts = vec![("D'accord.".to_string(), 1.0), ("La messagerie Outlook me redemande mes identifiants".to_string(), 0.7)];
        assert_eq!(ids(&lib, &parts, &[], 5), vec![b]);
        // un seul mot, dit avant le dernier message : trop faible
        let parts = vec![("Merci.".to_string(), 1.0), ("Outlook".to_string(), 0.7)];
        assert!(ids(&lib, &parts, &[], 5).is_empty());
    }

    #[test]
    fn plusieurs_sources_classees_de_la_plus_pertinente() {
        let lib = MediaLibrary::open_in_memory().unwrap();
        let x = lib.insert(&decrit("/x.mp4", "Ajouter une imprimante réseau sur un poste Windows : ouvrir Paramètres, puis Imprimantes et scanners.", &["imprimante", "windows"])).unwrap();
        let y = lib.insert(&decrit("/y.mp4", "Installer le pilote d'une imprimante locale USB sur un poste Windows.", &["imprimante", "usb"])).unwrap();
        let z = lib.insert(&decrit("/z.mp4", "Configurer la messagerie sur un smartphone.", &["messagerie", "mobile"])).unwrap();
        let texte = conversation("Comment ajouter une imprimante réseau sur mon poste Windows ?");
        assert_eq!(ids(&lib, &texte, &[], 5), vec![x, y], "x partage plus de mots, z aucun");
        assert!(!ids(&lib, &texte, &[], 5).contains(&z));
        assert_eq!(ids(&lib, &texte, &[], 1).len(), 1, "la limite est respectée");
    }

    #[test]
    fn l_index_suit_les_modifications() {
        let (lib, [a, ..]) = bibliotheque();
        let vpn = conversation("authentification VPN");
        assert_eq!(ids(&lib, &vpn, &[], 5), vec![a]);
        // modification des tags et de la description : prise en compte tout de suite
        lib.patch(a, &MediaPatch { tags: Some(vec!["wifi".into()]), description: Some("Réseau sans fil indisponible.".into()), ..Default::default() }).unwrap();
        assert!(ids(&lib, &vpn, &[], 5).is_empty());
        assert_eq!(ids(&lib, &conversation("Le réseau sans fil est indisponible"), &[], 5), vec![a]);
        // retrait de la médiathèque
        lib.delete_many(&[a]).unwrap();
        assert_eq!(lib.list().unwrap().len(), 3);
        assert!(ids(&lib, &conversation("Le réseau sans fil est indisponible"), &[], 5).is_empty());
        // ajout : retrouvé aussitôt
        let e = lib.insert(&decrit("/e.mp4", "Remplacer la batterie d'un ordinateur portable.", &["batterie"])).unwrap();
        assert_eq!(ids(&lib, &conversation("batteries"), &[], 5), vec![e]);
    }

    #[test]
    fn analyse_enregistree_sans_ecraser_les_tags_de_l_utilisateur() {
        let (lib, [a, ..]) = bibliotheque();
        lib.patch(a, &MediaPatch { tags: Some(vec!["vpn".into(), "télétravail".into()]), ..Default::default() }).unwrap();
        let analyse = Analysis { description: "Fenêtre du client VPN.".into(), tags: vec!["client vpn".into(), "vpn".into()] };
        let m = lib.apply_analysis(a, &analyse, "modele-test", false).unwrap().unwrap();
        assert!(m.analyzed);
        assert_eq!((m.description.as_str(), m.tags.clone()), ("Fenêtre du client VPN.", vec!["vpn".to_string(), "télétravail".into(), "client vpn".into()]));
        assert!(lib.get(a).unwrap().analyzed, "l'état est enregistré");
        let m = lib.apply_analysis(a, &analyse, "modele-test", true).unwrap().unwrap();
        assert_eq!(m.tags, vec!["client vpn".to_string(), "vpn".into()], "avec « remplacer », les tags de l'analyse l'emportent");
        assert!(lib.apply_analysis(999, &analyse, "modele-test", false).unwrap().is_none());
    }

    #[test]
    fn fichiers_a_analyser_et_decompte() {
        let (lib, [a, b, ..]) = bibliotheque();
        assert_eq!(lib.analysis_counts().unwrap(), (4, 0));
        lib.insert(&decrit("/v.mp4", "Tutoriel en vidéo.", &[])).unwrap();
        assert_eq!(lib.analysis_counts().unwrap(), (4, 0), "une vidéo n'est jamais analysée : elle n'est pas comptée");
        lib.apply_analysis(a, &Analysis { description: "x".into(), tags: vec![] }, "m", false).unwrap();
        assert_eq!(lib.analysis_counts().unwrap(), (4, 1));
        let photo = Some("photo");
        let todo = lib.to_analyze("new", &[], photo, None).unwrap();
        assert_eq!(todo.len(), 3);
        assert!(todo.iter().all(|m| m.id != a));
        assert_eq!(lib.to_analyze("all", &[], photo, Some(2)).unwrap().len(), 2);
        assert_eq!(lib.to_analyze("ids", &[b], photo, None).unwrap().iter().map(|m| m.id).collect::<Vec<_>>(), vec![b]);
        assert!(lib.to_analyze("ids", &[], photo, None).unwrap().is_empty());
        assert_eq!(lib.to_analyze("all", &[], Some("video"), None).unwrap().len(), 1, "la vidéo ajoutée au test");
        let f = |analysis: &str| MediaFilter { analysis: Some(analysis.into()), kind: Some("photo".into()), limit: 50, ..Default::default() };
        assert_eq!(lib.query(&f("done")).unwrap().0, 1);
        assert_eq!(lib.query(&f("todo")).unwrap().0, 3);
    }

    #[test]
    fn le_dossier_d_uploads_est_parcouru_sans_doublon() {
        let dir = std::env::temp_dir().join(format!("repere-scan-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        for (f, contenu) in [("videos/tuto_vpn.mp4", "x"), ("photos/erreur-0x80070005.png", "x"), ("thumbnails/t.png", "x"), ("notes.txt", "x")] {
            let p = dir.join(f);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, contenu).unwrap();
        }
        let lib = MediaLibrary::open_in_memory().unwrap();
        assert_eq!(lib.scan_uploads(&dir).unwrap(), 2, "miniatures et autres formats ignorés");
        assert_eq!(lib.scan_uploads(&dir).unwrap(), 0, "un second passage n'ajoute rien");
        let titres: Vec<String> = lib.list().unwrap().into_iter().map(|m| m.title).collect();
        assert!(titres.contains(&"tuto vpn".to_string()) && titres.contains(&"erreur 0x80070005".to_string()), "{titres:?}");
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn le_fichier_de_synonymes_de_l_utilisateur_est_pris_en_compte_et_relu() {
        let dir = std::env::temp_dir().join(format!("repere-syn-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let lib = MediaLibrary::open(&dir.join("live.db")).unwrap();
        let concepts = |text: &str| {
            let (_, th) = lib.thesaurus();
            th.triggers(&terms(text)).0.into_iter().map(|c| th.name(c).to_string()).collect::<Vec<_>>()
        };
        assert!(concepts("Mon sésame ne marche plus").is_empty(), "pas de synonyme encore");
        let file = dir.join("synonyms.txt");
        let write = |text: &str, secs: u64| {
            std::fs::write(&file, text).unwrap();
            let f = std::fs::OpenOptions::new().write(true).open(&file).unwrap();
            f.set_modified(SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000 + secs)).unwrap();
        };
        write("mot de passe, sésame\n", 1);
        assert_eq!(concepts("Mon sésame ne marche plus"), vec!["mot de passe"]);
        write("mot de passe, sésame\nimprimante, photocopieur\n", 2);
        assert_eq!(concepts("Le photocopieur est en panne"), vec!["imprimante"], "le fichier est relu dès qu'il change");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn base_ancienne_sans_colonne_d_empreinte_migree() {
        let dir = std::env::temp_dir().join(format!("repere-mig-{}.db", std::process::id()));
        let _ = std::fs::remove_file(&dir);
        {
            let c = Connection::open(&dir).unwrap();
            c.execute_batch("CREATE TABLE media (id INTEGER PRIMARY KEY AUTOINCREMENT, kind TEXT NOT NULL, url TEXT NOT NULL UNIQUE, thumb TEXT, title TEXT NOT NULL DEFAULT '', description TEXT NOT NULL DEFAULT '', tags TEXT NOT NULL DEFAULT '', ancienne_a TEXT NOT NULL DEFAULT '', ancienne_b INTEGER NOT NULL DEFAULT 1, ancienne_c INTEGER NOT NULL DEFAULT 0); INSERT INTO media (kind,url) VALUES ('video','/old.mp4');").unwrap();
        }
        let lib = MediaLibrary::open(&dir).unwrap();
        assert_eq!(lib.list().unwrap().len(), 1, "l'ancien fichier reste lisible malgré les colonnes en trop");
        assert!(lib.insert_new(&item("/new.mp4", &[]), Some("k")).unwrap().is_some(), "et l'on peut en ajouter");
        let _ = std::fs::remove_file(&dir);
        let _ = std::fs::remove_file(dir.with_extension("db-wal"));
        let _ = std::fs::remove_file(dir.with_extension("db-shm"));
    }
}
