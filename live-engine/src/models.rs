//! Catalogue de modèles, résolution des fichiers sur Hugging Face, téléchargement reprenable avec
//! vérification SHA-256, et détection des modèles installés.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tokio::io::AsyncWriteExt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Hash)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Llm,
    Stt,
    Tts,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CatalogEntry {
    pub id: String,
    pub kind: Kind,
    pub name: String,
    #[serde(default)]
    pub description: String,
    /// Dépôt Hugging Face « auteur/nom ».
    pub repo: String,
    /// Sous-dossier du dépôt (vide = racine).
    #[serde(default)]
    pub subdir: String,
    /// Noms de fichiers exacts à télécharger (prioritaire sur `pattern`).
    #[serde(default)]
    pub files: Vec<String>,
    /// Sous-chaîne (insensible à la casse) pour retrouver le .gguf voulu (ex. « Q4_K_M »).
    #[serde(default)]
    pub pattern: Option<String>,
    /// Dépôts de secours essayés dans l'ordre si `repo` est introuvable ou ne contient pas le fichier voulu.
    #[serde(default)]
    pub alt_repos: Vec<String>,
    /// Extensions à télécharger (sans point) quand on veut tous les fichiers d'un dépôt de ce type
    /// (ex. `["safetensors", "pt", "json"]` pour un modèle PyTorch). Utilisé si ni `files` ni `pattern`.
    #[serde(default)]
    pub include_ext: Vec<String>,
    /// Taille indicative en Go (avant résolution).
    #[serde(default)]
    pub size_gb: f64,
    /// Arguments supplémentaires pour le serveur de ce modèle (ex. options du chat template).
    #[serde(default)]
    pub args: Vec<String>,
    /// Exécutable à lancer pour ce modèle (TTS) ; arguments dans `args`, avec les jokers
    /// `{model}` (premier fichier), `{dir}`, `{host}` et `{port}`.
    #[serde(default)]
    pub exec: Option<String>,
    /// Remarque d'usage.
    #[serde(default)]
    pub note: String,
    /// Projecteur multimodal (fichier « mmproj ») qui permet au modèle de VOIR les images : morceau du nom du fichier
    /// (ex. « mmproj-BF16 »), cherché dans le dépôt du modèle ou dans `mmproj_repo`.
    #[serde(default)]
    pub mmproj: Option<String>,
    /// Dépôt du projecteur quand il n'est pas avec le modèle.
    #[serde(default)]
    pub mmproj_repo: Option<String>,
}

#[allow(clippy::too_many_arguments)]
fn e(id: &str, kind: Kind, name: &str, desc: &str, repo: &str, files: &[&str], pattern: Option<&str>, size: f64, args: &[&str]) -> CatalogEntry {
    CatalogEntry {
        id: id.into(), kind, name: name.into(), description: desc.into(), repo: repo.into(),
        subdir: String::new(), files: files.iter().map(|s| s.to_string()).collect(),
        pattern: pattern.map(String::from), alt_repos: vec![], include_ext: vec![], size_gb: size,
        args: args.iter().map(|s| s.to_string()).collect(), exec: None, note: String::new(),
        mmproj: None, mmproj_repo: None,
    }
}

/// Le catalogue livré : micro (Whisper) et voix (Chatterbox). Il ne contient aucun modèle de texte : un modèle s'ajoute par
/// `catalog.json` (voir `load_catalog`), une fois son dépôt et son fichier vérifiés sur Hugging Face.
pub fn builtin_catalog() -> Vec<CatalogEntry> {
    let mut tts = e("chatterbox-multilingual", Kind::Tts, "Chatterbox multilingue — voix clonée, expressive (recommandé)",
        "Synthèse vocale en français à partir d'un court échantillon de voix (≈ 10-20 s) et d'un réglage d'expressivité. \
         Licence MIT. ≈ 3,5 Go de téléchargement, ≈ 3,5 à 5 Go de VRAM. Nécessite d'installer le moteur de voix (Python/PyTorch). \
         Le premier démarrage a besoin d'internet (tokenizer chinois, téléchargé une fois).",
        "ResembleAI/chatterbox",
        &["ve.pt", "t3_mtl23ls_v2.safetensors", "s3gen.pt", "grapheme_mtl_merged_expanded_v1.json", "conds.pt", "Cangjie5_TC.json"],
        None, 3.5,
        &["--host", "{host}", "--port", "{port}", "--model-dir", "{dir}", "--voices-dir", "{voices}"]);
    tts.exec = Some("tts-server".into());
    vec![
        e("whisper-large-v3-turbo-q5", Kind::Stt, "Whisper large-v3-turbo (Q5_0) — précis, lent sur processeur",
          "Reconnaissance vocale multilingue, très bonne en français (≈ 547 Mo). Conçu pour le GPU : lent si le micro tourne sur le processeur (réglage par défaut).",
          "ggerganov/whisper.cpp", &["ggml-large-v3-turbo-q5_0.bin"], None, 0.57, &[]),
        e("whisper-small-q5", Kind::Stt, "Whisper small (Q5_1) — léger",
          "Plus petit et plus rapide, un peu moins précis (≈ 181 Mo).",
          "ggerganov/whisper.cpp", &["ggml-small-q5_1.bin"], None, 0.19, &[]),
        e("whisper-medium-q5", Kind::Stt, "Whisper medium (Q5_0) — recommandé (micro sur processeur)",
          "Compromis précision/vitesse (≈ 0,5 Go) : bon choix avec le micro sur processeur, qui est le réglage par défaut.",
          "ggerganov/whisper.cpp", &["ggml-medium-q5_0.bin"], None, 0.54, &[]),
        tts,
    ]
}

/// Catalogue intégré + entrées personnelles de `catalog.json` (même id = remplacement).
pub fn load_catalog(data_dir: &Path) -> Vec<CatalogEntry> {
    let mut cat = builtin_catalog();
    if let Ok(raw) = std::fs::read_to_string(data_dir.join("catalog.json")) {
        match serde_json::from_str::<Vec<CatalogEntry>>(&raw) {
            Ok(extra) => {
                for x in extra {
                    if let Some(slot) = cat.iter_mut().find(|c| c.id == x.id) { *slot = x } else { cat.push(x) }
                }
            }
            Err(e) => tracing::warn!("catalog.json ignoré : {e}"),
        }
    }
    cat
}

#[derive(Debug, Clone, PartialEq)]
pub struct RemoteFile {
    pub path: String,
    pub size: u64,
    pub sha256: Option<String>,
}

impl RemoteFile {
    fn basename(&self) -> &str {
        self.path.rsplit('/').next().unwrap_or(&self.path)
    }
}

/// Analyse la réponse de `GET /api/models/{repo}/tree/main/{subdir}`.
pub fn parse_tree(json: &serde_json::Value) -> Vec<RemoteFile> {
    json.as_array()
        .map(|a| {
            a.iter()
                .filter(|f| f["type"] == "file")
                .filter_map(|f| {
                    Some(RemoteFile {
                        path: f["path"].as_str()?.to_string(),
                        size: f["lfs"]["size"].as_u64().or_else(|| f["size"].as_u64())?,
                        sha256: f["lfs"]["oid"].as_str().map(String::from),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

fn is_shard(name: &str) -> bool {
    // ...-00001-of-00003.gguf
    let n = name.to_lowercase();
    n.ends_with(".gguf") && n.contains("-of-")
}

/// Choisit les fichiers à télécharger. Pure, donc testable sans réseau.
pub fn select_files(entry: &CatalogEntry, listing: &[RemoteFile]) -> Result<Vec<RemoteFile>, String> {
    if !entry.files.is_empty() {
        return entry
            .files
            .iter()
            .map(|wanted| {
                listing
                    .iter()
                    .find(|f| f.basename() == wanted)
                    .cloned()
                    .ok_or_else(|| format!("fichier « {wanted} » introuvable dans {}", entry.repo))
            })
            .collect();
    }
    if entry.pattern.is_none() && !entry.include_ext.is_empty() {
        let exts: Vec<String> = entry.include_ext.iter().map(|e| e.to_lowercase()).collect();
        let mut all: Vec<RemoteFile> = listing
            .iter()
            .filter(|f| f.basename().rsplit_once('.').is_some_and(|(_, e)| exts.contains(&e.to_lowercase())))
            .cloned()
            .collect();
        all.sort_by(|a, b| a.path.cmp(&b.path));
        return if all.is_empty() {
            Err(format!("aucun fichier {} dans {}", exts.join("/"), entry.repo))
        } else {
            Ok(all)
        };
    }
    let pat = entry
        .pattern
        .as_ref()
        .ok_or("entrée de catalogue sans `files`, `pattern` ni `include_ext`")?
        .to_lowercase();
    let mut hits: Vec<RemoteFile> = listing
        .iter()
        .filter(|f| {
            let n = f.basename().to_lowercase();
            n.ends_with(".gguf") && n.contains(&pat) && !n.contains("mmproj")
        })
        .cloned()
        .collect();
    hits.sort_by(|a, b| a.path.cmp(&b.path));
    match hits.len() {
        0 => Err(format!("aucun .gguf contenant « {pat} » dans {}", entry.repo)),
        1 => Ok(hits),
        _ if hits.iter().all(|h| is_shard(h.basename())) => Ok(hits), // modèle en plusieurs morceaux
        _ => Err(format!(
            "plusieurs fichiers correspondent à « {pat} » dans {} : {} — précise `files` dans catalog.json",
            entry.repo,
            hits.iter().map(|h| h.basename().to_string()).collect::<Vec<_>>().join(", ")
        )),
    }
}

/// Choisit le fichier projecteur (« mmproj ») : un .gguf dont le nom contient « mmproj » ET le morceau demandé. S'il y en a
/// plusieurs, un nom exactement égal l'emporte, sinon on refuse (pas de choix au hasard).
pub fn select_mmproj(wanted: &str, repo: &str, listing: &[RemoteFile]) -> Result<RemoteFile, String> {
    let w = wanted.to_lowercase();
    let hits: Vec<&RemoteFile> = listing
        .iter()
        .filter(|f| {
            let n = f.basename().to_lowercase();
            n.ends_with(".gguf") && n.contains("mmproj") && n.contains(&w)
        })
        .collect();
    match hits.len() {
        0 => Err(format!("aucun projecteur « mmproj » contenant « {wanted} » dans {repo}")),
        1 => Ok(hits[0].clone()),
        _ => hits
            .iter()
            .find(|f| f.basename().to_lowercase() == w || f.basename().to_lowercase() == format!("{w}.gguf"))
            .map(|f| (*f).clone())
            .ok_or_else(|| format!("plusieurs projecteurs correspondent à « {wanted} » dans {repo} : {}", hits.iter().map(|h| h.basename()).collect::<Vec<_>>().join(", "))),
    }
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct DownloadState {
    /// listing | downloading | verifying | done | error | cancelled
    pub status: String,
    pub file: String,
    pub downloaded: u64,
    pub total: u64,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Manifest {
    id: String,
    repo: String,
    files: Vec<ManifestFile>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
struct ManifestFile {
    name: String,
    size: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct Installed {
    pub dir: PathBuf,
    /// Fichiers du modèle (sans le projecteur multimodal).
    pub files: Vec<PathBuf>,
    /// Taille des fichiers du modèle (sans le projecteur).
    pub size_bytes: u64,
    /// Projecteur multimodal téléchargé : le modèle peut voir les images.
    pub mmproj: Option<PathBuf>,
    pub mmproj_bytes: u64,
}

pub struct Models {
    pub catalog: Vec<CatalogEntry>,
    models_dir: PathBuf,
    http: reqwest::Client,
    hf_base: String,
    /// Jeton Hugging Face : enregistré depuis l'interface (fichier `.hf-token`, lisible par l'utilisateur seul), sinon `HF_TOKEN`.
    token: Mutex<Option<String>>,
    token_file: PathBuf,
    states: Mutex<HashMap<String, DownloadState>>,
    handles: Mutex<HashMap<String, tokio::task::AbortHandle>>,
}

impl Models {
    pub fn new(catalog: Vec<CatalogEntry>, models_dir: PathBuf) -> Arc<Self> {
        let base = std::env::var("HF_ENDPOINT").unwrap_or_else(|_| "https://huggingface.co".into());
        Self::with_hf_base(catalog, models_dir, base)
    }

    /// Comme `new`, avec l'adresse du serveur Hugging Face (miroir, tests).
    pub fn with_hf_base(catalog: Vec<CatalogEntry>, models_dir: PathBuf, hf_base: String) -> Arc<Self> {
        let _ = std::fs::create_dir_all(&models_dir);
        let token_file = models_dir.join(".hf-token");
        let saved = std::fs::read_to_string(&token_file).ok().map(|t| t.trim().to_string()).filter(|t| !t.is_empty());
        let token = saved.or_else(|| std::env::var("HF_TOKEN").ok().filter(|t| !t.is_empty()));
        Arc::new(Self {
            catalog,
            models_dir,
            http: reqwest::Client::builder()
                .user_agent("repere/0.1")
                .connect_timeout(std::time::Duration::from_secs(20))
                .build()
                .expect("client HTTP"),
            hf_base,
            token: Mutex::new(token),
            token_file,
            states: Mutex::new(HashMap::new()),
            handles: Mutex::new(HashMap::new()),
        })
    }

    pub fn hf_token(&self) -> Option<String> {
        self.token.lock().unwrap().clone()
    }

    pub fn has_hf_token(&self) -> bool {
        self.token.lock().unwrap().is_some()
    }

    /// Enregistre (ou, avec `None`, efface) le jeton Hugging Face. Le jeton n'est jamais renvoyé par l'API.
    pub fn set_hf_token(&self, token: Option<&str>) -> Result<(), String> {
        let Some(raw) = token else {
            let _ = std::fs::remove_file(&self.token_file);
            *self.token.lock().unwrap() = std::env::var("HF_TOKEN").ok().filter(|t| !t.is_empty());
            return Ok(());
        };
        let t = raw.trim();
        if !t.starts_with("hf_") || t.len() < 10 || t.len() > 200 || t.chars().any(|c| !c.is_ascii_alphanumeric() && c != '_' && c != '-') {
            return Err("ce n'est pas un jeton Hugging Face (il commence par « hf_ » ; créez-en un « Read » sur huggingface.co/settings/tokens)".into());
        }
        let tmp = self.token_file.with_extension("tmp");
        let write = || -> std::io::Result<()> {
            #[cfg(unix)]
            {
                use std::io::Write;
                use std::os::unix::fs::OpenOptionsExt;
                let mut f = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&tmp)?;
                f.write_all(t.as_bytes())?;
            }
            #[cfg(not(unix))]
            std::fs::write(&tmp, t)?;
            std::fs::rename(&tmp, &self.token_file)
        };
        write().map_err(|e| format!("enregistrement impossible : {e}"))?;
        *self.token.lock().unwrap() = Some(t.to_string());
        Ok(())
    }

    pub fn entry(&self, id: &str) -> Option<&CatalogEntry> {
        self.catalog.iter().find(|c| c.id == id)
    }

    pub fn dir_of(&self, id: &str) -> PathBuf {
        self.models_dir.join(id)
    }

    pub fn state(&self, id: &str) -> Option<DownloadState> {
        self.states.lock().unwrap().get(id).cloned()
    }

    fn set_state(&self, id: &str, f: impl FnOnce(&mut DownloadState)) {
        let mut g = self.states.lock().unwrap();
        f(g.entry(id.to_string()).or_default());
    }

    /// Un modèle est installé si son manifeste existe et que tous ses fichiers ont la bonne taille.
    pub fn installed(&self, id: &str) -> Option<Installed> {
        let dir = self.dir_of(id);
        let m: Manifest = serde_json::from_str(&std::fs::read_to_string(dir.join("manifest.json")).ok()?).ok()?;
        let mut files = vec![];
        let (mut total, mut mmproj, mut mmproj_bytes) = (0, None, 0);
        for f in &m.files {
            let p = dir.join(&f.name);
            if std::fs::metadata(&p).ok()?.len() != f.size {
                return None;
            }
            if f.name.to_lowercase().contains("mmproj") {
                mmproj = Some(p);
                mmproj_bytes = f.size;
            } else {
                total += f.size;
                files.push(p);
            }
        }
        files.sort();
        Some(Installed { dir, files, size_bytes: total, mmproj, mmproj_bytes })
    }

    pub fn delete(&self, id: &str) -> bool {
        self.entry(id).is_some() && std::fs::remove_dir_all(self.dir_of(id)).is_ok()
    }

    async fn list_remote(&self, entry: &CatalogEntry) -> anyhow::Result<Vec<RemoteFile>> {
        let sub = entry.subdir.trim_matches('/');
        let url = if sub.is_empty() {
            format!("{}/api/models/{}/tree/main", self.hf_base, entry.repo)
        } else {
            format!("{}/api/models/{}/tree/main/{}", self.hf_base, entry.repo, sub)
        };
        let mut req = self.http.get(&url);
        if let Some(t) = self.hf_token() { req = req.bearer_auth(t); }
        let resp = req.send().await?;
        if !resp.status().is_success() {
            anyhow::bail!("Hugging Face : HTTP {} pour {}", resp.status(), url);
        }
        Ok(parse_tree(&resp.json().await?))
    }

    /// Lance le téléchargement en tâche de fond. `false` si déjà en cours.
    pub fn start_download(self: &Arc<Self>, id: &str) -> Result<bool, String> {
        self.start_download_opts(id, false)
    }

    /// `vision_required` : le projecteur multimodal est indispensable (« Ajouter la vision ») ; sinon son absence du dépôt n'empêche
    /// pas de télécharger le modèle seul.
    pub fn start_download_opts(self: &Arc<Self>, id: &str, vision_required: bool) -> Result<bool, String> {
        let entry = self.entry(id).ok_or("modèle inconnu")?.clone();
        if vision_required && entry.mmproj.is_none() {
            return Err("ce modèle n'a pas de projecteur de vision dans le catalogue".into());
        }
        if matches!(self.state(id).as_ref().map(|s| s.status.as_str()), Some("listing" | "downloading" | "verifying")) {
            return Ok(false);
        }
        self.set_state(id, |s| *s = DownloadState { status: "listing".into(), ..Default::default() });
        let me = self.clone();
        let id2 = id.to_string();
        let h = tokio::spawn(async move {
            let res = me.run_download(&entry, vision_required).await;
            me.handles.lock().unwrap().remove(&id2);
            match res {
                Ok(()) => me.set_state(&id2, |s| { s.status = "done".into(); s.error = None }),
                Err(e) => {
                    tracing::warn!("téléchargement {id2} : {e:#}");
                    me.set_state(&id2, |s| { s.status = "error".into(); s.error = Some(format!("{e:#}")) })
                }
            }
        });
        self.handles.lock().unwrap().insert(id.to_string(), h.abort_handle());
        Ok(true)
    }

    pub fn cancel(&self, id: &str) {
        if let Some(h) = self.handles.lock().unwrap().remove(id) {
            h.abort();
            self.set_state(id, |s| s.status = "cancelled".into());
        }
    }

    async fn run_download(&self, entry: &CatalogEntry, vision_required: bool) -> anyhow::Result<()> {
        // Premier dépôt (principal puis secours) qui existe ET contient le fichier voulu.
        let mut errors = Vec::new();
        let mut chosen = None;
        for repo in std::iter::once(&entry.repo).chain(entry.alt_repos.iter()) {
            let mut e = entry.clone();
            e.repo = repo.clone();
            match self.list_remote(&e).await.map_err(|x| x.to_string()).and_then(|l| select_files(&e, &l)) {
                Ok(files) => { chosen = Some((e, files)); break }
                Err(er) => errors.push(format!("{repo} : {er}")),
            }
        }
        let (entry, files) = chosen.ok_or_else(|| anyhow::anyhow!("aucun dépôt utilisable — {}", errors.join(" ; ")))?;
        let entry = &entry;
        // Projecteur multimodal (la « vue » du modèle) : dans le dépôt du modèle ou dans un autre. Facultatif, sauf si on l'exige
        // (« Ajouter la vision »).
        let mut jobs: Vec<(CatalogEntry, RemoteFile)> = files.iter().map(|f| (entry.clone(), f.clone())).collect();
        if let Some(wanted) = entry.mmproj.clone() {
            let repo = entry.mmproj_repo.clone().unwrap_or_else(|| entry.repo.clone());
            let mut e2 = entry.clone();
            e2.repo = repo.clone();
            match self.list_remote(&e2).await.map_err(|x| x.to_string()).and_then(|l| select_mmproj(&wanted, &repo, &l)) {
                Ok(f) => jobs.push((e2, f)),
                Err(er) if vision_required => anyhow::bail!("vision : {er}"),
                Err(er) => tracing::warn!("{} : modèle téléchargé sans vision ({er})", entry.id),
            }
        }
        let dir = self.dir_of(&entry.id);
        tokio::fs::create_dir_all(&dir).await?;
        let total: u64 = jobs.iter().map(|(_, f)| f.size).sum();
        self.set_state(&entry.id, |s| { s.status = "downloading".into(); s.total = total; s.downloaded = 0 });

        let mut done_before = 0u64;
        for (e, f) in &jobs {
            let name = f.basename().to_string();
            if name.contains("..") || name.is_empty() {
                anyhow::bail!("nom de fichier suspect : {}", f.path);
            }
            self.set_state(&entry.id, |s| s.file = name.clone());
            self.fetch_file(e, f, &dir.join(&name), done_before).await?;
            done_before += f.size;
        }
        let manifest = Manifest {
            id: entry.id.clone(),
            repo: entry.repo.clone(),
            files: jobs.iter().map(|(_, f)| ManifestFile { name: f.basename().to_string(), size: f.size }).collect(),
        };
        tokio::fs::write(dir.join("manifest.json"), serde_json::to_vec_pretty(&manifest)?).await?;
        Ok(())
    }

    async fn fetch_file(&self, entry: &CatalogEntry, f: &RemoteFile, dest: &Path, done_before: u64) -> anyhow::Result<()> {
        if tokio::fs::metadata(dest).await.map(|m| m.len() == f.size).unwrap_or(false) {
            self.set_state(&entry.id, |s| s.downloaded = done_before + f.size);
            return Ok(());
        }
        let part = PathBuf::from(format!("{}.part", dest.display()));
        let mut have = tokio::fs::metadata(&part).await.map(|m| m.len()).unwrap_or(0);
        if have > f.size {
            tokio::fs::remove_file(&part).await.ok();
            have = 0;
        }
        let url = format!("{}/{}/resolve/main/{}", self.hf_base, entry.repo, f.path);
        loop {
            let mut req = self.http.get(&url);
            if let Some(t) = self.hf_token() { req = req.bearer_auth(t); }
            if have > 0 { req = req.header("Range", format!("bytes={have}-")); }
            let resp = req.send().await?;
            let status = resp.status().as_u16();
            let mut out = match status {
                206 => tokio::fs::OpenOptions::new().append(true).open(&part).await?,
                200 => { have = 0; tokio::fs::File::create(&part).await? } // le serveur ignore Range : on repart de zéro
                416 => { tokio::fs::remove_file(&part).await.ok(); have = 0; continue } // .part incohérent
                s => anyhow::bail!("téléchargement de {} : HTTP {s}", f.path),
            };
            let mut got = have;
            let mut stream = resp.bytes_stream();
            use futures_util::StreamExt;
            while let Some(chunk) = stream.next().await {
                let chunk = chunk?;
                out.write_all(&chunk).await?;
                got += chunk.len() as u64;
                self.set_state(&entry.id, |s| s.downloaded = done_before + got);
            }
            out.flush().await?;
            drop(out);
            if got != f.size {
                anyhow::bail!("téléchargement incomplet de {} ({got}/{} octets) — relance pour reprendre", f.path, f.size);
            }
            break;
        }
        if let Some(expected) = &f.sha256 {
            self.set_state(&entry.id, |s| s.status = "verifying".into());
            let p = part.clone();
            let actual = tokio::task::spawn_blocking(move || sha256_file(&p)).await??;
            if !actual.eq_ignore_ascii_case(expected) {
                tokio::fs::remove_file(&part).await.ok();
                anyhow::bail!("somme SHA-256 incorrecte pour {} : fichier supprimé, relance le téléchargement", f.path);
            }
            self.set_state(&entry.id, |s| s.status = "downloading".into());
        }
        tokio::fs::rename(&part, dest).await?;
        Ok(())
    }
}

pub fn sha256_file(path: &Path) -> std::io::Result<String> {
    use std::io::Read;
    let mut f = std::fs::File::open(path)?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 { break; }
        h.update(&buf[..n]);
    }
    Ok(h.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn rf(path: &str) -> RemoteFile {
        RemoteFile { path: path.into(), size: 10, sha256: None }
    }
    fn entry(files: &[&str], pattern: Option<&str>) -> CatalogEntry {
        CatalogEntry {
            id: "x".into(), kind: Kind::Llm, name: "x".into(), description: String::new(), repo: "a/b".into(),
            subdir: String::new(), files: files.iter().map(|s| s.to_string()).collect(),
            pattern: pattern.map(String::from), alt_repos: vec![], include_ext: vec![], size_gb: 0.0, args: vec![], exec: None, note: String::new(),
            mmproj: None, mmproj_repo: None,
        }
    }

    #[test]
    fn analyse_la_reponse_de_l_api_hf() {
        let j = json!([
            {"type":"directory","path":"sub","oid":"1"},
            {"type":"file","path":"m-Q4_K_M.gguf","size":134,"oid":"git","lfs":{"oid":"abcd","size":7400000000u64,"pointerSize":134}},
            {"type":"file","path":"README.md","size":900,"oid":"g2"}
        ]);
        let t = parse_tree(&j);
        assert_eq!(t.len(), 2);
        assert_eq!(t[0].size, 7_400_000_000); // la taille LFS, pas celle du pointeur
        assert_eq!(t[0].sha256.as_deref(), Some("abcd"));
        assert_eq!(t[1].sha256, None);
    }

    #[test]
    fn selection_par_motif() {
        let l = vec![rf("m-Q4_K_M.gguf"), rf("m-Q5_K_M.gguf"), rf("mmproj-BF16.gguf"), rf("README.md")];
        let r = select_files(&entry(&[], Some("q4_k_m")), &l).unwrap();
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].path, "m-Q4_K_M.gguf");
    }

    #[test]
    fn selection_ambigue_est_refusee() {
        let l = vec![rf("a-Q4_K_M.gguf"), rf("b-Q4_K_M.gguf")];
        assert!(select_files(&entry(&[], Some("Q4_K_M")), &l).unwrap_err().contains("plusieurs fichiers"));
    }

    #[test]
    fn modele_en_plusieurs_morceaux() {
        let l = vec![rf("m-Q4_K_M-00001-of-00002.gguf"), rf("m-Q4_K_M-00002-of-00002.gguf")];
        assert_eq!(select_files(&entry(&[], Some("Q4_K_M")), &l).unwrap().len(), 2);
    }

    #[test]
    fn fichiers_exacts_et_absents() {
        let l = vec![rf("ggml-small.bin"), rf("ggml-small.en.bin")];
        assert_eq!(select_files(&entry(&["ggml-small.bin"], None), &l).unwrap()[0].path, "ggml-small.bin");
        assert!(select_files(&entry(&["ggml-tiny.bin"], None), &l).is_err());
    }

    #[test]
    fn selection_par_extensions() {
        let mut e = entry(&[], None);
        e.include_ext = vec!["safetensors".into(), "PT".into(), "json".into()];
        let l = vec![rf("ve.pt"), rf("t3.safetensors"), rf("tokenizer.json"), rf("README.md"), rf("sample.wav")];
        let r = select_files(&e, &l).unwrap();
        assert_eq!(r.iter().map(|f| f.path.as_str()).collect::<Vec<_>>(), vec!["t3.safetensors", "tokenizer.json", "ve.pt"]);
        e.include_ext = vec!["gguf".into()];
        assert!(select_files(&e, &l).is_err());
    }

    #[test]
    fn projecteur_de_vision_choisi_sans_ambiguite() {
        let l = vec![rf("m.Q4_K_M.gguf"), rf("m.mmproj-Q8_0.gguf"), rf("m.mmproj-f16.gguf"), rf("README.md")];
        assert_eq!(select_mmproj("mmproj-Q8_0", "a/b", &l).unwrap().path, "m.mmproj-Q8_0.gguf");
        assert_eq!(select_mmproj("MMPROJ-F16", "a/b", &l).unwrap().path, "m.mmproj-f16.gguf", "insensible à la casse");
        assert!(select_mmproj("mmproj", "a/b", &l).is_err(), "deux candidats : on ne choisit pas au hasard");
        assert!(select_mmproj("bf16", "a/b", &l).unwrap_err().contains("aucun projecteur"));
        // le modèle lui-même n'est jamais pris pour un projecteur, ni l'inverse
        assert!(select_files(&entry(&[], Some("q4_k_m")), &l).unwrap().iter().all(|f| !f.path.contains("mmproj")));
        assert!(select_mmproj("Q4_K_M", "a/b", &l).is_err());
    }

    #[test]
    fn un_depot_de_projecteur_exige_un_motif() {
        for e in &builtin_catalog() {
            assert!(e.mmproj_repo.is_none() || e.mmproj.is_some(), "{} : dépôt de projecteur sans motif", e.id);
        }
    }

    #[tokio::test]
    async fn telechargement_du_modele_et_de_son_projecteur_depuis_deux_depots() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;
        let model: Vec<u8> = (0..30_000u32).map(|i| (i % 241) as u8).collect();
        let proj: Vec<u8> = (0..9_000u32).map(|i| (i % 199) as u8).collect();
        let sha = |b: &[u8]| { let mut h = Sha256::new(); h.update(b); h.finalize().iter().map(|x| format!("{x:02x}")).collect::<String>() };
        let (sha_m, sha_p) = (sha(&model), sha(&proj));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let (m2, p2) = (model.clone(), proj.clone());
        tokio::spawn(async move {
            loop {
                let (mut s, _) = listener.accept().await.unwrap();
                let (m, p) = (m2.clone(), p2.clone());
                let (sha_m, sha_p) = (sha_m.clone(), sha_p.clone());
                tokio::spawn(async move {
                    let mut buf = vec![0u8; 4096];
                    let n = s.read(&mut buf).await.unwrap();
                    let req = String::from_utf8_lossy(&buf[..n]).to_string();
                    let first = req.lines().next().unwrap_or("").to_string();
                    let (code, ctype, body): (&str, &str, Vec<u8>) = if first.starts_with("GET /api/models/org/model/tree/main") {
                        let j = format!(r#"[{{"type":"file","path":"vl.Q4_K_M.gguf","size":10,"lfs":{{"oid":"{sha_m}","size":{}}}}}]"#, m.len());
                        ("200 OK", "application/json", j.into_bytes())
                    } else if first.starts_with("GET /api/models/org/proj/tree/main") {
                        let j = format!(r#"[{{"type":"file","path":"vl.mmproj-f16.gguf","size":10,"lfs":{{"oid":"{sha_p}","size":{}}}}},{{"type":"file","path":"README.md","size":5}}]"#, p.len());
                        ("200 OK", "application/json", j.into_bytes())
                    } else if first.contains("/org/model/resolve/main/vl.Q4_K_M.gguf") {
                        ("200 OK", "application/octet-stream", m)
                    } else if first.contains("/org/proj/resolve/main/vl.mmproj-f16.gguf") {
                        ("200 OK", "application/octet-stream", p)
                    } else {
                        ("404 Not Found", "text/plain", b"nope".to_vec())
                    };
                    let h = format!("HTTP/1.1 {code}\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
                    s.write_all(h.as_bytes()).await.unwrap();
                    s.write_all(&body).await.unwrap();
                    let _ = s.shutdown().await;
                });
            }
        });
        let dir = std::env::temp_dir().join(format!("repere-dl-vision-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut e = entry(&[], Some("Q4_K_M"));
        e.id = "vl".into();
        e.repo = "org/model".into();
        e.mmproj = Some("mmproj-f16".into());
        e.mmproj_repo = Some("org/proj".into());
        let m = Models::with_hf_base(vec![e.clone()], dir.clone(), format!("http://{addr}"));
        let wait = |m: Arc<Models>| async move {
            for _ in 0..200 {
                tokio::time::sleep(std::time::Duration::from_millis(25)).await;
                if matches!(m.state("vl").map(|s| s.status).as_deref(), Some("done" | "error")) { break; }
            }
            m.state("vl").unwrap()
        };
        assert!(m.start_download("vl").unwrap());
        let st = wait(m.clone()).await;
        assert_eq!(st.status, "done", "{st:?}");
        let inst = m.installed("vl").unwrap();
        assert_eq!(inst.files.len(), 1, "le projecteur n'est pas un fichier du modèle");
        assert_eq!(std::fs::read(&inst.files[0]).unwrap(), model);
        assert_eq!(std::fs::read(inst.mmproj.as_ref().unwrap()).unwrap(), proj);
        assert_eq!((inst.size_bytes, inst.mmproj_bytes), (model.len() as u64, proj.len() as u64));

        // Modèle déjà installé SANS projecteur (ancien téléchargement) : « Ajouter la vision » ne télécharge que ce qui manque.
        std::fs::remove_file(m.dir_of("vl").join("vl.mmproj-f16.gguf")).unwrap();
        std::fs::write(
            m.dir_of("vl").join("manifest.json"),
            format!(r#"{{"id":"vl","repo":"org/model","files":[{{"name":"vl.Q4_K_M.gguf","size":{}}}]}}"#, model.len()),
        ).unwrap();
        assert!(m.installed("vl").unwrap().mmproj.is_none());
        let mtime = std::fs::metadata(&inst.files[0]).unwrap().modified().unwrap();
        assert!(m.start_download_opts("vl", true).unwrap());
        let st = wait(m.clone()).await;
        assert_eq!(st.status, "done", "{st:?}");
        assert!(m.installed("vl").unwrap().mmproj.is_some());
        assert_eq!(std::fs::metadata(&inst.files[0]).unwrap().modified().unwrap(), mtime, "le modèle n'est pas retéléchargé");

        // Projecteur introuvable : facultatif au téléchargement normal, fatal quand on le demande.
        let mut e2 = e.clone();
        e2.id = "vl2".into();
        e2.mmproj = Some("mmproj-bf16".into());
        let m2 = Models::with_hf_base(vec![e2], dir.clone(), format!("http://{addr}"));
        let wait2 = |m: Arc<Models>| async move {
            for _ in 0..200 {
                tokio::time::sleep(std::time::Duration::from_millis(25)).await;
                if matches!(m.state("vl2").map(|s| s.status).as_deref(), Some("done" | "error")) { break; }
            }
            m.state("vl2").unwrap()
        };
        assert!(m2.start_download("vl2").unwrap());
        let st = wait2(m2.clone()).await;
        assert_eq!(st.status, "done", "sans projecteur, le modèle seul est téléchargé : {st:?}");
        assert!(m2.installed("vl2").unwrap().mmproj.is_none());
        assert!(m2.start_download_opts("vl2", true).unwrap());
        let st = wait2(m2.clone()).await;
        assert_eq!(st.status, "error");
        assert!(st.error.unwrap().contains("vision"));
        assert!(m2.start_download_opts("vl", false).is_err(), "modèle inconnu de ce catalogue");
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn catalogue_ids_uniques() {
        let c = builtin_catalog();
        let mut ids: Vec<_> = c.iter().map(|e| e.id.clone()).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), c.len());
        assert!(c.iter().all(|e| !e.files.is_empty() || e.pattern.is_some() || !e.include_ext.is_empty()));
        assert!(c.iter().any(|e| e.kind == Kind::Tts && e.exec.is_some()), "un TTS installable est proposé");
        // Tout modèle de texte annonce une taille (sinon l'interface le classerait comme « mini » à tort).
        assert!(c.iter().filter(|e| e.kind == Kind::Llm).all(|e| e.size_gb > 0.0));
        assert!(c.iter().all(|e| !e.name.is_empty() && !e.repo.is_empty() && e.repo.contains('/')));
        assert!(c.iter().filter(|e| e.kind == Kind::Stt).count() >= 3, "trois modèles de micro");
        // Chatterbox : liste exacte des fichiers de from_local (pas le modèle anglais).
        let t = c.iter().find(|e| e.id == "chatterbox-multilingual").unwrap();
        assert!(t.files.iter().any(|f| f == "t3_mtl23ls_v2.safetensors") && !t.files.iter().any(|f| f.starts_with("t3_cfg")));
    }

    #[test]
    fn le_catalogue_personnel_ajoute_ou_remplace_des_entrees() {
        let dir = std::env::temp_dir().join(format!("repere-catalog-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let builtin = builtin_catalog().len();
        assert_eq!(load_catalog(&dir).len(), builtin, "sans fichier : le catalogue intégré seul");
        // Un modèle de texte ajouté, et une entrée intégrée remplacée (même identifiant).
        std::fs::write(
            dir.join("catalog.json"),
            r#"[
                {"id":"mon-modele","kind":"llm","name":"Mon modèle","repo":"org/depot","pattern":"Q4_K_M","size_gb":4.5},
                {"id":"whisper-small-q5","kind":"stt","name":"Whisper small (perso)","repo":"org/autre","files":["f.bin"]}
            ]"#,
        )
        .unwrap();
        let cat = load_catalog(&dir);
        assert_eq!(cat.len(), builtin + 1);
        assert_eq!(cat.iter().find(|e| e.id == "mon-modele").map(|e| e.kind), Some(Kind::Llm));
        assert_eq!(cat.iter().find(|e| e.id == "whisper-small-q5").unwrap().repo, "org/autre");
        // Un fichier illisible est ignoré, le catalogue intégré reste entier.
        std::fs::write(dir.join("catalog.json"), "{pas du json").unwrap();
        assert_eq!(load_catalog(&dir).len(), builtin);
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn sha256_connu() {
        let dir = std::env::temp_dir().join(format!("repere-sha-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("f");
        std::fs::write(&p, b"abc").unwrap();
        assert_eq!(sha256_file(&p).unwrap(), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn jeton_hugging_face_enregistre_valide_et_efface() {
        let dir = std::env::temp_dir().join(format!("repere-hf-{}", std::process::id()));
        let m = Models::new(vec![], dir.clone());
        for bad in ["", "abc", "hf_", "hf_ab cd1234567", "hf_é1234567890", "sk-1234567890123"] {
            assert!(m.set_hf_token(Some(bad)).is_err(), "{bad:?} accepté");
        }
        m.set_hf_token(Some("  hf_AbCdEf0123456789  ")).unwrap();
        assert_eq!(m.hf_token().as_deref(), Some("hf_AbCdEf0123456789"));
        // il survit à un redémarrage, avec des droits réservés à l'utilisateur
        assert_eq!(Models::new(vec![], dir.clone()).hf_token().as_deref(), Some("hf_AbCdEf0123456789"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(dir.join(".hf-token")).unwrap().permissions().mode() & 0o777, 0o600);
        }
        m.set_hf_token(None).unwrap();
        assert!(std::env::var("HF_TOKEN").is_ok() || !m.has_hf_token());
        assert!(!dir.join(".hf-token").exists());
        std::fs::remove_dir_all(dir).ok();
    }

    #[tokio::test]
    async fn telechargement_reprise_et_verification_sur_serveur_local() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;
        // Faux « Hugging Face » : API de liste + fichier avec support de Range.
        let body: Vec<u8> = (0..200_000u32).map(|i| (i % 251) as u8).collect();
        let sha = {
            let mut h = Sha256::new();
            h.update(&body);
            h.finalize().iter().map(|b| format!("{b:02x}")).collect::<String>()
        };
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let (b2, sha2) = (body.clone(), sha.clone());
        tokio::spawn(async move {
            loop {
                let (mut s, _) = listener.accept().await.unwrap();
                let (b, sha) = (b2.clone(), sha2.clone());
                tokio::spawn(async move {
                    let mut buf = vec![0u8; 4096];
                    let n = s.read(&mut buf).await.unwrap();
                    let req = String::from_utf8_lossy(&buf[..n]).to_string();
                    if req.starts_with("GET /api/models/a/b/tree/main") {
                        let j = format!(r#"[{{"type":"file","path":"m-Q4_K_M.gguf","size":134,"lfs":{{"oid":"{sha}","size":{}}}}}]"#, b.len());
                        let r = format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{j}", j.len());
                        s.write_all(r.as_bytes()).await.unwrap();
                    } else {
                        let start = req.lines().find_map(|l| l.to_lowercase().strip_prefix("range: bytes=").map(|v| v.trim_end_matches('-').to_string()))
                            .and_then(|v| v.parse::<usize>().ok());
                        let (code, data) = match start { Some(st) => ("206 Partial Content", &b[st..]), None => ("200 OK", &b[..]) };
                        let h = format!("HTTP/1.1 {code}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", data.len());
                        s.write_all(h.as_bytes()).await.unwrap();
                        s.write_all(data).await.unwrap();
                    }
                    let _ = s.shutdown().await;
                });
            }
        });

        let dir = std::env::temp_dir().join(format!("repere-dl-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::env::set_var("HF_ENDPOINT", format!("http://{addr}"));
        let m = Models::new(vec![entry(&[], Some("Q4_K_M"))], dir.clone());
        // Un .part partiel existe déjà : le téléchargement doit reprendre à la suite.
        std::fs::create_dir_all(m.dir_of("x")).unwrap();
        std::fs::write(m.dir_of("x").join("m-Q4_K_M.gguf.part"), &body[..50_000]).unwrap();

        assert!(m.start_download("x").unwrap());
        for _ in 0..100 {
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            if matches!(m.state("x").map(|s| s.status).as_deref(), Some("done" | "error")) { break; }
        }
        let st = m.state("x").unwrap();
        assert_eq!(st.status, "done", "{st:?}");
        let inst = m.installed("x").expect("installé");
        assert_eq!(std::fs::read(&inst.files[0]).unwrap(), body);
        std::fs::remove_dir_all(dir).ok();
    }
}
