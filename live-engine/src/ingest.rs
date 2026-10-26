//! Import de documents dans la base de connaissances : lecture du fichier (`extract`), découpage en passages, signalement des mots
//! de passe oubliés dans le texte, copie du fichier d'origine (pour le télécharger plus tard) et enregistrement. Un dossier entier
//! s'importe en tâche de fond : l'interface suit l'avancement et peut l'annuler.
//!
//! Réimporter un document de même nom le remplace (ses passages sont refaits) ; le même contenu sous un autre nom est un doublon, et
//! un fichier inchangé n'est pas relu.

use crate::extract::{self, Block, Format};
use crate::kb::{KnowledgeBase, NewDocument, NewPassage};
use crate::secrets;
use crate::state::AppState;
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Dossier (dans les données du moteur) où sont gardées les copies des fichiers d'origine.
pub const FILES_DIR: &str = "kb-files";
/// Une lecture plus longue est abandonnée : un PDF piégé ne doit pas bloquer l'import.
const EXTRACT_TIMEOUT: Duration = Duration::from_secs(180);
/// Fichiers lus de front.
const PARALLEL: usize = 4;
const MAX_FILES: usize = 20_000;
const MAX_PASSAGES: usize = 3000;
const MAX_NOTES: usize = 100;
/// Longueur maximale de la clé d'un document.
const MAX_NAME: usize = 300;

/// Un fichier à importer.
#[derive(Debug, Clone, Default)]
pub struct Upload {
    /// Nom du fichier, ou chemin relatif dans le dossier importé : c'est la clé du document.
    pub name: String,
    pub bytes: Vec<u8>,
    /// Titre imposé ; sinon celui du document, sinon le nom du fichier.
    pub title: Option<String>,
    /// Adresse d'origine imposée ; sinon celle que donne le document.
    pub url: Option<String>,
    pub tags: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum Outcome {
    Added { id: i64, title: String, passages: usize, replaced: bool, warnings: Vec<String> },
    /// Même fichier que celui déjà importé sous ce nom.
    Unchanged { id: i64, title: String },
    /// Même contenu que le document `of`, importé sous un autre nom.
    Duplicate { of: i64, title: String },
    Failed { error: String },
}

/// Le nom d'un fichier comme clé : séparateurs normalisés, pas de dossier parent ni de nom vide.
fn clean_name(name: &str) -> Option<String> {
    let n: Vec<&str> = name.split(['/', '\\']).map(str::trim).filter(|p| !p.is_empty() && *p != "." && *p != "..").collect();
    let n = n.join("/");
    (!n.is_empty() && n.chars().count() <= MAX_NAME && !n.chars().any(char::is_control)).then_some(n)
}

/// « 05-imprimantes.md » → « Imprimantes » : titre de repli quand le document n'en donne pas.
pub fn title_from_name(name: &str) -> String {
    let file = name.rsplit('/').next().unwrap_or(name);
    let stem = file.rsplit_once('.').map_or(file, |(s, _)| s);
    let words: Vec<&str> = stem.split(['_', '-', ' ']).filter(|w| !w.is_empty()).collect();
    let skip = words.iter().take_while(|w| w.chars().all(|c| c.is_ascii_digit())).count().min(words.len().saturating_sub(1));
    let text = words[skip..].join(" ");
    let mut cs = text.chars();
    cs.next().map(|c| c.to_uppercase().collect::<String>() + cs.as_str()).unwrap_or_default()
}

fn hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

/// Copie le fichier d'origine sous `<empreinte>.<format>` (écriture atomique) ; `None` si la copie échoue (le document est importé quand
/// même, sans téléchargement possible).
fn store_copy(files_dir: &Path, hash: &str, format: Format, bytes: &[u8]) -> Option<String> {
    let name = format!("{hash}.{}", format.key());
    let path = files_dir.join(&name);
    if path.exists() {
        return Some(name);
    }
    let write = || -> std::io::Result<()> {
        std::fs::create_dir_all(files_dir)?;
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, bytes)?;
        std::fs::rename(&tmp, &path)
    };
    match write() {
        Ok(()) => Some(name),
        Err(e) => {
            tracing::warn!("copie du document impossible : {e}");
            None
        }
    }
}

/// Importe un fichier.
pub async fn ingest(kb: &KnowledgeBase, files_dir: &Path, up: Upload) -> Outcome {
    let fail = |error: String| Outcome::Failed { error };
    let Some(name) = clean_name(&up.name) else { return fail("nom de fichier invalide".into()) };
    let Some(format) = Format::detect(&name, &up.bytes) else {
        let ext = name.rsplit_once('.').map(|(_, e)| format!(".{e}")).unwrap_or_else(|| "sans extension".into());
        return fail(format!("format non pris en charge ({ext}) : Markdown, HTML, Word (.docx), PDF et texte seulement"));
    };
    if up.bytes.len() > extract::MAX_DOCUMENT_BYTES {
        return fail(format!("fichier trop volumineux ({} Mo, maximum {} Mo)", up.bytes.len() / (1024 * 1024), extract::MAX_DOCUMENT_BYTES / (1024 * 1024)));
    }
    let hash = hex(&up.bytes);
    let given_url = up.url.as_deref().map(str::trim).filter(|u| !u.is_empty()).map(str::to_string);
    if let Some(same) = kb.document_by_name(&name) {
        if same.hash == hash && given_url.as_ref().is_none_or(|u| same.url.as_ref() == Some(u)) {
            return Outcome::Unchanged { id: same.id, title: same.title };
        }
    } else if let Some(dup) = kb.document_by_hash(&hash) {
        return Outcome::Duplicate { of: dup.id, title: dup.title };
    }

    // lecture dans un fil à part, avec une limite de temps
    let bytes = Arc::new(up.bytes);
    let reading = tokio::task::spawn_blocking({
        let bytes = bytes.clone();
        move || extract::extract(format, &bytes)
    });
    let mut extracted = match tokio::time::timeout(EXTRACT_TIMEOUT, reading).await {
        Ok(Ok(Ok(e))) => e,
        Ok(Ok(Err(e))) => return fail(e.to_string()),
        Ok(Err(_)) => return fail("la lecture du fichier a échoué (fichier endommagé ?)".into()),
        Err(_) => return fail(format!("lecture trop longue (plus de {} minutes) : fichier abandonné", EXTRACT_TIMEOUT.as_secs() / 60)),
    };
    if !extracted.has_text() {
        return fail(extracted.warnings.pop().unwrap_or_else(|| "aucun texte à lire dans ce fichier".into()));
    }

    let first_h1 = extracted.blocks.iter().find_map(|b| if let Block::Heading { level: 1, text } = b { Some(text.clone()) } else { None });
    let title = up.title.as_deref().map(str::trim).filter(|t| !t.is_empty()).map(str::to_string).or(extracted.title.take()).or(first_h1).unwrap_or_else(|| title_from_name(&name));
    let tags = {
        let mut all = extracted.tags.clone();
        all.extend(up.tags.iter().cloned());
        extract::normalize_tags(&all)
    };
    let url = given_url.or(extracted.url.take()).filter(|u| extract::is_web_address(u));

    let chunks = extract::chunk(&title, &extracted.blocks);
    if chunks.is_empty() {
        return fail("le document ne contient aucun passage exploitable".into());
    }
    let mut warnings = std::mem::take(&mut extracted.warnings);
    let mut chunks = chunks;
    if chunks.len() > MAX_PASSAGES {
        warnings.push(format!("seuls les {MAX_PASSAGES} premiers passages sur {} sont gardés", chunks.len()));
        chunks.truncate(MAX_PASSAGES);
    }
    // mots de passe ou clés oubliés dans le texte : signalés à la personne qui importe, jamais recopiés dans le message
    let mut kinds: Vec<&str> = vec![];
    let mut at: Vec<usize> = vec![];
    for (i, c) in chunks.iter().enumerate() {
        let k = secrets::kinds(&c.text);
        if !k.is_empty() {
            at.push(i + 1);
            for kind in k {
                if !kinds.contains(&kind) {
                    kinds.push(kind);
                }
            }
        }
    }
    if !at.is_empty() {
        let places: Vec<String> = at.iter().take(5).map(|n| n.to_string()).collect();
        warnings.push(format!(
            "{} {} (passage{} {}{}) : l'assistant ne les répétera jamais et le fichier d'origine n'est plus proposé au téléchargement ; mieux vaut les retirer du document",
            secrets::WARNING,
            kinds.join(", "),
            if at.len() > 1 { "s" } else { "" },
            places.join(", "),
            if at.len() > 5 { "…" } else { "" }
        ));
    }

    let passages: Vec<NewPassage> = chunks.iter().map(|c| NewPassage { section: c.label(), text: c.text.clone(), page: c.page }).collect();
    let stored = store_copy(files_dir, &hash, format, &bytes);
    let doc = NewDocument { name, title: title.clone(), format: format.key().into(), hash, bytes: bytes.len() as u64, url, tags, stored: stored.clone(), warnings: warnings.clone() };
    match kb.add_document(&doc, &passages) {
        Ok((id, previous)) => {
            // la copie de l'ancienne version n'est gardée que si un autre document s'en sert
            if let Some(old) = previous.as_ref().and_then(|p| p.stored.clone()).filter(|old| Some(old) != stored.as_ref()) {
                if !kb.stored_in_use(&old) {
                    let _ = std::fs::remove_file(files_dir.join(old));
                }
            }
            Outcome::Added { id, title, passages: passages.len(), replaced: previous.is_some(), warnings }
        }
        Err(e) => fail(format!("enregistrement impossible : {e}")),
    }
}

// ───────────────────────────── import d'un dossier

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct IngestOptions {
    /// Dossier (ou fichier) à importer : chemin complet.
    pub path: String,
    pub recursive: bool,
    /// Étiquettes ajoutées à tous les documents.
    pub tags: Vec<String>,
    /// Étiquettes tirées des noms de dossiers.
    pub folder_tags: bool,
    /// Adresse du dossier sur l'intranet : l'adresse d'un document en est déduite (adresse de base + chemin relatif).
    pub base_url: Option<String>,
}

impl Default for IngestOptions {
    fn default() -> Self {
        Self { path: String::new(), recursive: true, tags: vec![], folder_tags: true, base_url: None }
    }
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct IngestStatus {
    pub running: bool,
    /// Fin normale ou annulée : le résultat reste affiché jusqu'au prochain import.
    pub finished: bool,
    pub cancelled: bool,
    pub source: String,
    pub total: usize,
    pub done: usize,
    pub added: usize,
    pub replaced: usize,
    pub unchanged: usize,
    pub duplicates: usize,
    pub failed: usize,
    pub passages: usize,
    pub current: String,
    /// Extensions ignorées (formats non pris en charge) et leur nombre.
    pub skipped: BTreeMap<String, usize>,
    /// Fichiers en échec : « nom : cause ».
    pub errors: Vec<String>,
    /// Remarques sur des documents importés : « nom : remarque ».
    pub warnings: Vec<String>,
    pub seconds: u64,
}

pub struct Ingester {
    status: Arc<Mutex<IngestStatus>>,
    cancel: Arc<AtomicBool>,
}

impl Ingester {
    pub fn new() -> Self {
        Self { status: Arc::new(Mutex::new(IngestStatus::default())), cancel: Arc::new(AtomicBool::new(false)) }
    }

    pub fn status(&self) -> IngestStatus {
        self.status.lock().unwrap().clone()
    }

    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    /// Lance l'import en tâche de fond. Erreur si un import tourne déjà ou si le chemin est inutilisable.
    pub fn start(&self, app: Arc<AppState>, opts: IngestOptions) -> Result<(), String> {
        let root = PathBuf::from(opts.path.trim());
        if !root.is_absolute() {
            return Err("indiquez le chemin complet du dossier (ex. /home/vous/Documents)".into());
        }
        if !root.exists() {
            return Err(format!("« {} » n'existe pas", root.display()));
        }
        if let Some(u) = opts.base_url.as_deref().map(str::trim).filter(|u| !u.is_empty()) {
            if !extract::is_web_address(u) {
                return Err("l'adresse de base doit commencer par http:// ou https://".into());
            }
        }
        {
            let mut st = self.status.lock().unwrap();
            if st.running {
                return Err("un import est déjà en cours".into());
            }
            *st = IngestStatus { running: true, source: root.display().to_string(), ..Default::default() };
        }
        self.cancel.store(false, Ordering::Relaxed);
        let (status, cancel) = (self.status.clone(), self.cancel.clone());
        tokio::spawn(async move {
            let started = Instant::now();
            run(app, root, opts, status.clone(), cancel.clone()).await;
            let mut st = status.lock().unwrap();
            st.running = false;
            st.finished = true;
            st.cancelled = cancel.load(Ordering::Relaxed);
            st.current.clear();
            st.seconds = started.elapsed().as_secs();
        });
        Ok(())
    }
}

struct Found {
    path: PathBuf,
    /// Chemin relatif au dossier importé, avec des « / ».
    rel: String,
    /// Noms des dossiers entre la racine (comprise) et le fichier.
    dirs: Vec<String>,
}

const KNOWN_EXT: [&str; 11] = ["md", "markdown", "mdown", "html", "htm", "xhtml", "txt", "text", "docx", "docm", "pdf"];

/// Parcourt le dossier (sans suivre les liens vers des dossiers : pas de boucle) et range les fichiers lisibles ; compte les autres.
fn walk(root: &Path, recursive: bool, skipped: &mut BTreeMap<String, usize>) -> Vec<Found> {
    if root.is_file() {
        let name = root.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        return vec![Found { path: root.to_path_buf(), rel: name, dirs: vec![] }];
    }
    let root_name = root.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    let mut out = vec![];
    let mut stack = vec![(root.to_path_buf(), String::new(), vec![root_name])];
    while let Some((dir, prefix, names)) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        let mut entries: Vec<_> = rd.flatten().collect();
        entries.sort_by_key(|e| e.file_name());
        for e in entries {
            let p = e.path();
            let name = e.file_name().to_string_lossy().to_string();
            // fichiers cachés, verrous de Word (« ~$guide.docx ») et fichiers temporaires
            if name.starts_with('.') || name.starts_with("~$") || name.ends_with('~') {
                continue;
            }
            let Ok(ft) = e.file_type() else { continue };
            if ft.is_dir() {
                if recursive {
                    let mut n = names.clone();
                    n.push(name.clone());
                    stack.push((p, format!("{prefix}{name}/"), n));
                }
                continue;
            }
            if ft.is_symlink() && !p.is_file() {
                continue;
            }
            let ext = p.extension().and_then(|x| x.to_str()).unwrap_or("").to_lowercase();
            if !KNOWN_EXT.contains(&ext.as_str()) {
                if !ext.is_empty() {
                    *skipped.entry(ext).or_default() += 1;
                }
                continue;
            }
            out.push(Found { path: p, rel: format!("{prefix}{name}"), dirs: names.clone() });
            if out.len() >= MAX_FILES {
                return out;
            }
        }
    }
    out
}

/// Adresse d'un document d'après l'adresse de base du dossier : chaque segment du chemin relatif est encodé.
fn url_for(base: &str, rel: &str) -> String {
    let enc = |s: &str| -> String {
        s.bytes().map(|b| if b.is_ascii_alphanumeric() || b"-._~".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") }).collect()
    };
    format!("{}/{}", base.trim_end_matches('/'), rel.split('/').map(enc).collect::<Vec<_>>().join("/"))
}

async fn run(app: Arc<AppState>, root: PathBuf, opts: IngestOptions, status: Arc<Mutex<IngestStatus>>, cancel: Arc<AtomicBool>) {
    let (files, skipped) = tokio::task::spawn_blocking({
        let (root, recursive) = (root.clone(), opts.recursive);
        move || {
            let mut skipped = BTreeMap::new();
            let files = walk(&root, recursive, &mut skipped);
            (files, skipped)
        }
    })
    .await
    .unwrap_or_default();
    {
        let mut st = status.lock().unwrap();
        st.total = files.len();
        st.skipped = skipped;
    }
    let files_dir = app.cfg.server.data_dir.join(FILES_DIR);
    let base_url = opts.base_url.as_deref().map(str::trim).filter(|u| !u.is_empty()).map(str::to_string);
    let base_tags = extract::normalize_tags(&opts.tags);
    let folder_tags = opts.folder_tags;

    let work = futures_util::stream::iter(files).map(|f| {
        let (app, files_dir, base_url, base_tags, status, cancel) = (app.clone(), files_dir.clone(), base_url.clone(), base_tags.clone(), status.clone(), cancel.clone());
        async move {
            if cancel.load(Ordering::Relaxed) {
                return None;
            }
            status.lock().unwrap().current = f.rel.clone();
            let size = std::fs::metadata(&f.path).map(|m| m.len()).unwrap_or(0);
            if size as usize > extract::MAX_DOCUMENT_BYTES {
                return Some((f.rel.clone(), Outcome::Failed { error: format!("fichier trop volumineux ({} Mo)", size / (1024 * 1024)) }));
            }
            let bytes = match tokio::fs::read(&f.path).await {
                Ok(b) => b,
                Err(e) => return Some((f.rel.clone(), Outcome::Failed { error: format!("lecture impossible : {e}") })),
            };
            let mut tags = base_tags;
            if folder_tags {
                tags.extend(crate::importer::folder_tags(&f.dirs));
            }
            let up = Upload { name: f.rel.clone(), bytes, title: None, url: base_url.as_deref().map(|b| url_for(b, &f.rel)), tags };
            Some((f.rel.clone(), ingest(&app.kb, &files_dir, up).await))
        }
    });
    let mut results = work.buffer_unordered(PARALLEL);
    while let Some(r) = results.next().await {
        let Some((name, outcome)) = r else { continue };
        let mut st = status.lock().unwrap();
        st.done += 1;
        match outcome {
            Outcome::Added { passages, replaced, warnings, .. } => {
                st.added += 1;
                st.passages += passages;
                st.replaced += usize::from(replaced);
                for w in warnings {
                    if st.warnings.len() < MAX_NOTES {
                        st.warnings.push(format!("{name} : {w}"));
                    }
                }
            }
            Outcome::Unchanged { .. } => st.unchanged += 1,
            Outcome::Duplicate { title, .. } => {
                st.duplicates += 1;
                if st.warnings.len() < MAX_NOTES {
                    st.warnings.push(format!("{name} : même contenu que « {title} », déjà importé : ignoré"));
                }
            }
            Outcome::Failed { error } => {
                st.failed += 1;
                if st.errors.len() < MAX_NOTES {
                    st.errors.push(format!("{name} : {error}"));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kb() -> KnowledgeBase {
        KnowledgeBase::open_in_memory().unwrap()
    }

    fn dossier(nom: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("repere-ingest-{nom}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn up(name: &str, texte: &str) -> Upload {
        Upload { name: name.into(), bytes: texte.as_bytes().to_vec(), ..Default::default() }
    }

    #[test]
    fn noms_et_titres() {
        assert_eq!(clean_name("vpn/../guide.md").as_deref(), Some("vpn/guide.md"));
        assert_eq!(clean_name("C:\\docs\\a.md").as_deref(), Some("C:/docs/a.md"));
        assert_eq!(clean_name("  ").as_deref(), None);
        assert_eq!(clean_name(&"a".repeat(400)), None);
        assert_eq!(title_from_name("05-imprimantes.md"), "Imprimantes");
        assert_eq!(title_from_name("dossier/poste_lent.txt"), "Poste lent");
        assert_eq!(title_from_name("2024.pdf"), "2024", "un nom fait de chiffres reste un nom");
        assert_eq!(title_from_name("wifi"), "Wifi");
        assert_eq!(url_for("https://intranet.exemple.fr/aide/", "vpn/guide réseau.md"), "https://intranet.exemple.fr/aide/vpn/guide%20r%C3%A9seau.md");
    }

    #[tokio::test]
    async fn un_document_est_lu_decoupe_et_enregistre() {
        let (kb, dir) = (kb(), dossier("ajout"));
        let md = "---\ntitle: Guide VPN\ntags: [vpn]\n---\n# Guide VPN\n\n## Erreur 809\n\nAutorisez les ports UDP 500 et 4500 dans le pare-feu.\n";
        let o = ingest(&kb, &dir, Upload { tags: vec!["Accès distant".into()], ..up("vpn/guide.md", md) }).await;
        let Outcome::Added { id, title, passages, replaced, warnings } = o else { panic!("{o:?}") };
        assert_eq!((title.as_str(), passages, replaced, warnings.is_empty()), ("Guide VPN", 1, false, true));
        let doc = kb.document(id).unwrap();
        assert_eq!((doc.name.as_str(), doc.format.as_str(), doc.tags.clone(), doc.passages), ("vpn/guide.md", "md", vec!["vpn".to_string(), "accès distant".into()], 1));
        assert!(doc.has_file && dir.join(doc.stored.as_deref().unwrap()).is_file(), "copie du fichier d'origine");
        let p = &kb.passages_of(id).unwrap()[0];
        assert_eq!((p.kind.as_str(), p.section.as_str(), p.title.as_str()), ("passage", "Erreur 809", "Guide VPN"));
        assert!(p.description.contains("UDP 500 et 4500"));
        // la recherche le retrouve
        let parts = vec![("erreur 809 sur le VPN".to_string(), 1.0)];
        assert_eq!(kb.relevant(&parts, &Default::default(), 4).iter().map(|s| s.id).collect::<Vec<_>>(), vec![p.id]);
        std::fs::remove_dir_all(dir).ok();
    }

    #[tokio::test]
    async fn meme_fichier_doublon_et_remplacement() {
        let (kb, dir) = (kb(), dossier("remplace"));
        let v1 = "# Wi-Fi\n\nLe réseau ENTREPRISE utilise vos identifiants habituels pour les agents.\n";
        let Outcome::Added { id, .. } = ingest(&kb, &dir, up("wifi.md", v1)).await else { panic!() };
        let ancien = kb.document(id).unwrap().stored.unwrap();
        // même fichier, même nom : rien à refaire
        assert!(matches!(ingest(&kb, &dir, up("wifi.md", v1)).await, Outcome::Unchanged { id: i, .. } if i == id));
        // même contenu sous un autre nom : doublon
        assert!(matches!(ingest(&kb, &dir, up("copie.md", v1)).await, Outcome::Duplicate { of, .. } if of == id));
        assert_eq!(kb.documents().unwrap().len(), 1);
        // nouveau contenu sous le même nom : le document est remplacé, ses anciens passages et sa copie disparaissent
        let v2 = "# Wi-Fi\n\nLe réseau INVITES donne accès à Internet avec un code valable 24 heures.\n";
        let Outcome::Added { id: nouveau, replaced, .. } = ingest(&kb, &dir, up("wifi.md", v2)).await else { panic!() };
        assert!(replaced && nouveau != id);
        assert!(kb.document(id).is_none() && kb.passages_of(id).unwrap().is_empty());
        assert!(!dir.join(&ancien).exists(), "ancienne copie supprimée");
        assert_eq!(kb.stats().passages, 1);
        // une adresse d'origine donnée après coup relance l'import
        let o = ingest(&kb, &dir, Upload { url: Some("https://intranet.exemple.fr/wifi".into()), ..up("wifi.md", v2) }).await;
        assert!(matches!(o, Outcome::Added { replaced: true, .. }), "{o:?}");
        assert_eq!(kb.document_by_name("wifi.md").unwrap().url.as_deref(), Some("https://intranet.exemple.fr/wifi"));
        std::fs::remove_dir_all(dir).ok();
    }

    #[tokio::test]
    async fn les_fichiers_inutilisables_sont_refuses_avec_la_cause() {
        let (kb, dir) = (kb(), dossier("refus"));
        let err = |o: Outcome| match o {
            Outcome::Failed { error } => error,
            o => panic!("{o:?}"),
        };
        assert!(err(ingest(&kb, &dir, up("tableur.xlsx", "PK")).await).contains("format non pris en charge (.xlsx)"));
        assert!(err(ingest(&kb, &dir, up("vide.md", "   \n\n")).await).contains("aucun texte"));
        assert!(err(ingest(&kb, &dir, up("faux.pdf", "ce n'est pas un pdf")).await).contains("format non pris en charge"));
        assert!(err(ingest(&kb, &dir, up("../..", "x")).await).contains("nom de fichier invalide"));
        let gros = Upload { name: "gros.txt".into(), bytes: vec![b'a'; extract::MAX_DOCUMENT_BYTES + 1], ..Default::default() };
        assert!(err(ingest(&kb, &dir, gros).await).contains("trop volumineux"));
        assert_eq!(kb.stats(), Default::default(), "rien n'a été enregistré");
        std::fs::remove_dir_all(dir).ok();
    }

    #[tokio::test]
    async fn un_mot_de_passe_dans_le_texte_est_signale_sans_etre_recopie() {
        let (kb, dir) = (kb(), dossier("secret"));
        let md = "# Accès au serveur\n\nConnectez-vous avec le compte admin. Mot de passe : Printemps2024! (à changer).\n";
        let Outcome::Added { warnings, .. } = ingest(&kb, &dir, up("serveur.md", md)).await else { panic!() };
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("mot de passe") && warnings[0].contains("passage 1"), "{warnings:?}");
        assert!(!warnings[0].contains("Printemps"), "la valeur n'est jamais recopiée");
        let doc = kb.document_by_name("serveur.md").unwrap();
        assert_eq!(doc.warnings, warnings);
        assert!(doc.secrets, "le document est marqué : son fichier d'origine ne se télécharge pas");
        let Outcome::Added { id, .. } = ingest(&kb, &dir, up("propre.md", "# Guide\n\nRien de secret ici, juste des consignes.\n")).await else { panic!() };
        assert!(!kb.document(id).unwrap().secrets);
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn le_parcours_ignore_les_fichiers_caches_les_verrous_et_les_formats_inconnus() {
        let d = dossier("parcours");
        for (f, c) in [("guide.md", "x"), ("sous/page.html", "x"), ("sous/profond/notes.txt", "x"), (".cache.md", "x"), ("~$verrou.docx", "x"), ("tableur.xlsx", "x"), ("sous/diapo.pptx", "x"), ("sauvegarde.md~", "x"), ("image.png", "x")] {
            let p = d.join(f);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, c).unwrap();
        }
        let mut skipped = BTreeMap::new();
        let found = walk(&d, true, &mut skipped);
        let mut rels: Vec<&str> = found.iter().map(|f| f.rel.as_str()).collect();
        rels.sort();
        assert_eq!(rels, vec!["guide.md", "sous/page.html", "sous/profond/notes.txt"]);
        assert_eq!(skipped, BTreeMap::from([("pptx".to_string(), 1), ("png".to_string(), 1), ("xlsx".to_string(), 1)]));
        // sans sous-dossiers
        assert_eq!(walk(&d, false, &mut BTreeMap::new()).len(), 1);
        // un fichier seul
        let seul = walk(&d.join("guide.md"), true, &mut BTreeMap::new());
        assert_eq!((seul.len(), seul[0].rel.as_str()), (1, "guide.md"));
        let profond = found.iter().find(|f| f.rel.ends_with("notes.txt")).unwrap();
        assert_eq!(profond.dirs.len(), 3, "dossier racine, sous, profond");
        std::fs::remove_dir_all(d).ok();
    }
}
