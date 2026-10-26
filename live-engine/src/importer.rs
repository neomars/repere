//! Import massif de vidéos et de photos : parcourt un dossier (avec ses sous-dossiers), place chaque fichier dans les
//! dossiers servis par l'app (`videos/`, `photos/`) par lien physique (instantané, sans doubler l'espace disque) ou par
//! copie, crée une miniature, ignore les doublons (même contenu) et référence tout dans la médiathèque, avec des tags
//! tirés des noms de dossiers. Les vidéos que le navigateur ne lit pas (AVI) sont converties en MP4 par ffmpeg. Le travail
//! tourne en tâche de fond : l'interface suit l'avancement et peut l'annuler.

use crate::kb::Source;
use crate::state::AppState;
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Combien de fichiers sont traités en même temps (copie + miniature).
const PARALLEL: usize = 6;
const MAX_ERRORS: usize = 60;
const VIDEO_EXT: [&str; 5] = ["mp4", "webm", "mov", "m4v", "mkv"];
/// Vidéos que le navigateur ne lit pas, mais qu'ffmpeg convertit en MP4 (H.264 + AAC) pendant l'import.
const CONVERT_EXT: [&str; 1] = ["avi"];
const PHOTO_EXT: [&str; 6] = ["jpg", "jpeg", "png", "webp", "gif", "avif"];
/// Formats courants que le navigateur ne sait pas lire : comptés et signalés, pas importés.
const UNPLAYABLE_EXT: [&str; 11] = ["wmv", "flv", "mpg", "mpeg", "3gp", "ts", "heic", "heif", "tif", "tiff", "bmp"];
/// Conversions menées de front : chacune occupe tous les cœurs, plus ne ferait qu'attendre en se gênant.
const CONVERSIONS: usize = 2;
/// Noms de dossiers sans valeur de tag.
const GENERIC_DIRS: [&str; 24] = [
    "videos", "video", "vidéos", "vidéo", "photos", "photo", "images", "image", "pictures", "picture", "dcim", "camera",
    "download", "downloads", "téléchargements", "new folder", "nouveau dossier", "desktop", "bureau", "home", "tmp",
    "media", "medias", "mnt",
];

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct ImportOptions {
    pub path: String,
    /// Copier les fichiers au lieu de les lier (lien physique : même disque, aucun espace en plus).
    pub copy: bool,
    pub recursive: bool,
    /// Tags tirés des noms de dossiers (le dossier choisi compris).
    pub folder_tags: bool,
    /// Annotation par défaut de tout le lot.
    pub tags: Vec<String>,
}

impl Default for ImportOptions {
    fn default() -> Self {
        Self { path: String::new(), copy: false, recursive: true, folder_tags: true, tags: vec![] }
    }
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct ImportStatus {
    pub running: bool,
    /// Fin normale ou annulée : le résultat reste affiché jusqu'au prochain import.
    pub finished: bool,
    pub cancelled: bool,
    pub source: String,
    pub phase: String, // scan | import
    pub total: usize,
    pub done: usize,
    pub added: usize,
    pub videos: usize,
    pub photos: usize,
    pub duplicates: usize,
    pub failed: usize,
    pub no_thumb: usize,
    /// Vidéos converties en MP4 (AVI) parmi celles qui ont été ajoutées.
    pub converted: usize,
    pub current: String,
    /// Extensions ignorées (le navigateur ne les lit pas) et leur nombre.
    pub unsupported: BTreeMap<String, usize>,
    pub errors: Vec<String>,
    pub warning: Option<String>,
    pub seconds: u64,
}

pub struct Importer {
    status: Arc<Mutex<ImportStatus>>,
    cancel: Arc<AtomicBool>,
}

impl Importer {
    pub fn new() -> Self {
        Self { status: Arc::new(Mutex::new(ImportStatus::default())), cancel: Arc::new(AtomicBool::new(false)) }
    }

    pub fn status(&self) -> ImportStatus {
        self.status.lock().unwrap().clone()
    }

    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    /// Lance l'import en tâche de fond. Erreur si un import tourne déjà ou si le dossier est inutilisable.
    pub fn start(&self, app: Arc<AppState>, opts: ImportOptions) -> Result<(), String> {
        let root = PathBuf::from(opts.path.trim());
        if !root.is_absolute() {
            return Err("indiquez le chemin complet du dossier (ex. /home/vous/Documents)".into());
        }
        if !root.is_dir() {
            return Err(format!("« {} » n'est pas un dossier", root.display()));
        }
        {
            let mut st = self.status.lock().unwrap();
            if st.running {
                return Err("un import est déjà en cours".into());
            }
            *st = ImportStatus { running: true, source: root.display().to_string(), phase: "scan".into(), ..Default::default() };
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

#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) enum Kind {
    Video,
    Photo,
}

struct Found {
    path: PathBuf,
    kind: Kind,
    /// Noms de dossiers entre la racine (comprise) et le fichier.
    dirs: Vec<String>,
    /// Vidéo que le navigateur ne lit pas : elle est convertie en MP4 au lieu d'être liée ou copiée.
    convert: bool,
}

/// Parcourt le dossier (sans suivre les liens vers des dossiers : pas de boucle) et classe les fichiers.
fn walk(root: &Path, recursive: bool, unsupported: &mut BTreeMap<String, usize>) -> Vec<Found> {
    let root_name = root.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    let mut out = vec![];
    let mut stack = vec![(root.to_path_buf(), vec![root_name])];
    while let Some((dir, names)) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        let mut entries: Vec<_> = rd.flatten().collect();
        entries.sort_by_key(|e| e.file_name());
        for e in entries {
            let p = e.path();
            let name = e.file_name().to_string_lossy().to_string();
            if name.starts_with('.') {
                continue;
            }
            let Ok(ft) = e.file_type() else { continue };
            if ft.is_dir() {
                if recursive {
                    let mut n = names.clone();
                    n.push(name);
                    stack.push((p, n));
                }
                continue;
            }
            // Un lien symbolique vers un fichier est accepté ; vers un dossier, ignoré.
            if ft.is_symlink() && !p.is_file() {
                continue;
            }
            let ext = p.extension().and_then(|x| x.to_str()).unwrap_or("").to_lowercase();
            let convert = CONVERT_EXT.contains(&ext.as_str());
            let kind = if VIDEO_EXT.contains(&ext.as_str()) || convert {
                Kind::Video
            } else if PHOTO_EXT.contains(&ext.as_str()) {
                Kind::Photo
            } else {
                if UNPLAYABLE_EXT.contains(&ext.as_str()) {
                    *unsupported.entry(ext).or_default() += 1;
                }
                continue;
            };
            out.push(Found { path: p, kind, dirs: names.clone(), convert });
        }
    }
    out
}

/// Sans ffmpeg, rien ne peut être converti : les vidéos concernées sont comptées parmi les formats non importés.
fn without_conversions(found: Vec<Found>, unsupported: &mut BTreeMap<String, usize>) -> Vec<Found> {
    found
        .into_iter()
        .filter(|f| {
            if f.convert {
                let ext = f.path.extension().and_then(|x| x.to_str()).unwrap_or("").to_lowercase();
                *unsupported.entry(ext).or_default() += 1;
            }
            !f.convert
        })
        .collect()
}

/// Tags d'après les dossiers : du plus proche du fichier au plus lointain, 4 au maximum, sans noms génériques.
pub fn folder_tags(dirs: &[String]) -> Vec<String> {
    let mut tags: Vec<String> = vec![];
    for d in dirs.iter().rev() {
        let t = d.trim().to_lowercase();
        let t = t.trim_matches(|c: char| c == '_' || c == '-' || c == '.' || c.is_whitespace()).to_string();
        let digits_only = t.chars().all(|c| c.is_ascii_digit() || c == '-' || c == '_' || c == ' ' || c == '.');
        if t.chars().count() < 2 || t.chars().count() > 40 || digits_only || GENERIC_DIRS.contains(&t.as_str()) || tags.contains(&t) {
            continue;
        }
        tags.push(t);
        if tags.len() == 4 {
            break;
        }
    }
    tags
}

/// Nom de fichier sûr pour l'URL et le disque : ASCII, sans espaces.
pub fn safe_name(stem: &str, ext: &str) -> String {
    let mut s: String = stem
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' })
        .collect();
    while s.contains("__") {
        s = s.replace("__", "_");
    }
    let s = s.trim_matches('_');
    let s: String = s.chars().take(80).collect();
    format!("{}.{}", if s.is_empty() { "media" } else { &s }, ext.to_lowercase())
}

/// Empreinte rapide du contenu : taille + 64 Ko du début, du milieu et de la fin (deux fichiers différents de même taille
/// et de mêmes extrémités sont extrêmement rares ; lire tout un disque de vidéos serait trop long).
pub fn fingerprint(path: &Path) -> std::io::Result<String> {
    let mut f = std::fs::File::open(path)?;
    let len = f.metadata()?.len();
    let mut h = Sha256::new();
    h.update(len.to_le_bytes());
    let mut buf = vec![0u8; 64 * 1024];
    for pos in [0, len / 2, len.saturating_sub(buf.len() as u64)] {
        f.seek(SeekFrom::Start(pos))?;
        let n = f.read(&mut buf)?;
        h.update(&buf[..n]);
    }
    Ok(h.finalize().iter().take(16).map(|b| format!("{b:02x}")).collect())
}

/// Crée `dir/name` sans jamais écraser : lien physique (ou copie si le lien est impossible ou demandé). En cas de nom pris,
/// ajoute un suffixe. Renvoie le chemin créé.
fn place(src: &Path, dir: &Path, name: &str, copy: bool, key: &str) -> std::io::Result<PathBuf> {
    let (stem, ext) = name.rsplit_once('.').unwrap_or((name, ""));
    for attempt in 0..50 {
        let candidate = match attempt {
            0 => name.to_string(),
            1 => format!("{stem}-{}.{ext}", &key[..8]),
            n => format!("{stem}-{}-{n}.{ext}", &key[..8]),
        };
        let dst = dir.join(&candidate);
        let res = if copy {
            copy_new(src, &dst)
        } else {
            match std::fs::hard_link(src, &dst) {
                Ok(()) => Ok(()),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Err(e),
                Err(_) => copy_new(src, &dst), // autre disque, système de fichiers sans liens…
            }
        };
        match res {
            Ok(()) => return Ok(dst),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    Err(std::io::Error::new(std::io::ErrorKind::AlreadyExists, "trop de fichiers du même nom"))
}

fn copy_new(src: &Path, dst: &Path) -> std::io::Result<()> {
    let mut out = std::fs::OpenOptions::new().write(true).create_new(true).open(dst)?;
    let res = std::fs::File::open(src).and_then(|mut i| std::io::copy(&mut i, &mut out)).map(|_| ());
    if res.is_err() {
        drop(out);
        let _ = std::fs::remove_file(dst); // jamais de fichier à moitié copié
    }
    res
}

/// Miniature PNG de 320 px de large (image d'une vidéo à 1 s, ou la photo réduite). `false` si ffmpeg échoue.
pub(crate) async fn thumbnail(ffmpeg: &Path, src: &Path, dst: &Path, kind: Kind) -> bool {
    let attempts: &[&str] = if kind == Kind::Video { &["1", "0"] } else { &[""] };
    for ss in attempts {
        let mut cmd = tokio::process::Command::new(ffmpeg);
        cmd.args(["-hide_banner", "-loglevel", "error"]);
        if !ss.is_empty() {
            cmd.args(["-ss", ss]);
        }
        cmd.arg("-i").arg(src).args(["-frames:v", "1", "-vf", "scale=320:-2", "-y"]).arg(dst);
        cmd.env_remove("LD_LIBRARY_PATH").stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null()).kill_on_drop(true);
        let ok = matches!(tokio::time::timeout(Duration::from_secs(90), cmd.status()).await, Ok(Ok(s)) if s.success());
        if ok && dst.metadata().is_ok_and(|m| m.len() > 0) {
            return true;
        }
        let _ = std::fs::remove_file(dst);
    }
    false
}

/// Miniature d'un WebP animé, que ffmpeg ne lit pas : celle de sa première image.
async fn animated_webp_thumbnail(ffmpeg: &Path, src: &Path, dst: &Path) -> bool {
    let src = src.to_path_buf();
    let Ok(Some(still)) = tokio::task::spawn_blocking(move || crate::webp::still_copy(&src)).await else { return false };
    let ok = thumbnail(ffmpeg, &still, dst, Kind::Photo).await;
    let _ = std::fs::remove_file(&still);
    ok
}

/// Lance ffmpeg et attend sa fin ; l'annulation de l'import l'arrête sans attendre. Erreur : la dernière ligne qu'il a écrite.
async fn run_ffmpeg(ffmpeg: &Path, args: &[OsString], cancel: &AtomicBool) -> Result<(), String> {
    use tokio::io::AsyncReadExt;
    let mut cmd = tokio::process::Command::new(ffmpeg);
    cmd.args(["-hide_banner", "-loglevel", "error", "-nostdin"]).args(args).env_remove("LD_LIBRARY_PATH");
    cmd.stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::piped()).kill_on_drop(true);
    let mut child = cmd.spawn().map_err(|e| format!("ffmpeg introuvable ({e})"))?;
    let mut stderr = child.stderr.take();
    // Lu en entier et à part (un ffmpeg bavard remplirait le tuyau et se bloquerait), mais seule la fin est gardée.
    let log = tokio::spawn(async move {
        let mut tail: Vec<u8> = vec![];
        if let Some(e) = stderr.as_mut() {
            let mut buf = [0u8; 8192];
            while let Ok(n) = e.read(&mut buf).await {
                if n == 0 {
                    break;
                }
                tail.extend_from_slice(&buf[..n]);
                if tail.len() > 16 * 1024 {
                    tail.drain(..tail.len() - 8 * 1024);
                }
            }
        }
        String::from_utf8_lossy(&tail).to_string()
    });
    loop {
        tokio::select! {
            status = child.wait() => {
                let status = status.map_err(|e| format!("ffmpeg : {e}"))?;
                if status.success() {
                    return Ok(());
                }
                let text = log.await.unwrap_or_default();
                let why = text.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("échec sans message");
                return Err(format!("ffmpeg : {}", why.chars().take(200).collect::<String>()));
            }
            _ = tokio::time::sleep(Duration::from_millis(400)) => {
                if cancel.load(Ordering::Relaxed) {
                    let _ = child.kill().await;
                    return Err("annulé".into());
                }
            }
        }
    }
}

/// Convertit une vidéo que le navigateur ne lit pas (AVI…) en MP4 (H.264 + AAC, images carrées, dimensions paires, index en tête
/// de fichier) dans `dir`, sous un nom libre. On réencode toujours : le contenu d'un AVI (MPEG-4/DivX, MJPEG…) n'est presque jamais
/// lisible tel quel, et son horodatage en « re-emballage » est trop capricieux. Le fichier d'origine n'est pas touché ; rien ne reste
/// en cas d'échec ou d'annulation.
async fn convert_to_mp4(ffmpeg: &Path, src: &Path, dir: &Path, name: &str, key: &str, cancel: &AtomicBool) -> Result<PathBuf, String> {
    let part = dir.join(format!(".conversion-{}-{}.part", &key[..8], std::process::id()));
    let after: &[&str] = &[
        "-map", "0:v:0", "-map", "0:a:0?",
        "-vf", "scale=trunc(iw*sar/2)*2:trunc(ih/2)*2,setsar=1,format=yuv420p",
        "-c:v", "libx264", "-preset", "veryfast", "-crf", "22", "-c:a", "aac", "-b:a", "160k", "-ac", "2",
        "-movflags", "+faststart", "-f", "mp4", "-y",
    ];
    // les chemins restent des `OsString` : un nom de fichier qui n'est pas de l'UTF-8 valide ne doit pas être déformé
    let mut args: Vec<OsString> = vec![OsString::from("-i"), src.as_os_str().to_owned()];
    args.extend(after.iter().map(OsString::from));
    args.push(part.as_os_str().to_owned());
    let res = run_ffmpeg(ffmpeg, &args, cancel).await;
    if res.is_ok() && part.metadata().map_or(true, |m| m.len() == 0) {
        let _ = std::fs::remove_file(&part);
        return Err("conversion impossible : ffmpeg n'a rien produit".into());
    }
    if let Err(e) = res {
        let _ = std::fs::remove_file(&part);
        return Err(e);
    }
    let (p, d, n, k) = (part.clone(), dir.to_path_buf(), name.to_string(), key.to_string());
    let placed = tokio::task::spawn_blocking(move || place(&p, &d, &n, false, &k)).await.map_err(|e| e.to_string())?;
    let _ = std::fs::remove_file(&part);
    placed.map_err(|e| format!("copie impossible ({e})"))
}

pub(crate) fn ffmpeg_path() -> Option<PathBuf> {
    // ffmpeg embarqué par l'app (REPERE_FFMPEG), à défaut celui du système.
    let mut candidates = vec![];
    if let Some(p) = std::env::var_os("REPERE_FFMPEG") {
        candidates.push(PathBuf::from(p));
    }
    candidates.push(PathBuf::from("ffmpeg"));
    candidates.into_iter().find(|c| {
        std::process::Command::new(c)
            .arg("-version")
            .env_remove("LD_LIBRARY_PATH")
            .stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
    })
}

enum Outcome {
    Added { kind: Kind, thumb: bool, converted: bool },
    Duplicate,
}

fn note_error(status: &Mutex<ImportStatus>, msg: String) {
    let mut st = status.lock().unwrap();
    st.failed += 1;
    if st.errors.len() < MAX_ERRORS {
        st.errors.push(msg);
    }
}

async fn run(app: Arc<AppState>, root: PathBuf, opts: ImportOptions, status: Arc<Mutex<ImportStatus>>, cancel: Arc<AtomicBool>) {
    let uploads = app.cfg.server.uploads_dir.clone();
    let (videos_dir, photos_dir, thumbs_dir) = (uploads.join("videos"), uploads.join("photos"), uploads.join("thumbnails"));
    for d in [&videos_dir, &photos_dir, &thumbs_dir] {
        if let Err(e) = std::fs::create_dir_all(d) {
            note_error(&status, format!("dossier {} inutilisable : {e}", d.display()));
            return;
        }
    }
    let (found, mut unsupported) = {
        let (r, rec) = (root.clone(), opts.recursive);
        tokio::task::spawn_blocking(move || {
            let mut u = BTreeMap::new();
            let f = walk(&r, rec, &mut u);
            (f, u)
        })
        .await
        .unwrap_or_default()
    };
    let ffmpeg = ffmpeg_path();
    let found = if ffmpeg.is_some() { found } else { without_conversions(found, &mut unsupported) };
    {
        let mut st = status.lock().unwrap();
        st.total = found.len();
        st.unsupported = unsupported;
        st.phase = "import".into();
    }
    if ffmpeg.is_none() {
        status.lock().unwrap().warning = Some("ffmpeg introuvable : les médias sont importés sans miniature, et les vidéos à convertir (AVI) ne le sont pas.".into());
    }
    let conversions = Arc::new(tokio::sync::Semaphore::new(CONVERSIONS));
    let base_tags: Vec<String> = opts.tags.iter().map(|t| t.trim().to_lowercase()).filter(|t| !t.is_empty()).collect();

    futures_util::stream::iter(found)
        .for_each_concurrent(PARALLEL, |f| {
            let (app, status, cancel, ffmpeg, opts, base_tags, conversions) =
                (app.clone(), status.clone(), cancel.clone(), ffmpeg.clone(), opts.clone(), base_tags.clone(), conversions.clone());
            let (videos_dir, photos_dir, thumbs_dir) = (videos_dir.clone(), photos_dir.clone(), thumbs_dir.clone());
            async move {
                if cancel.load(Ordering::Relaxed) {
                    return;
                }
                let label = f.path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                status.lock().unwrap().current = if f.convert { format!("{label} (conversion en MP4…)") } else { label.clone() };
                let res = import_one(&app.kb, &f, &opts, &base_tags, ffmpeg.as_deref(), &videos_dir, &photos_dir, &thumbs_dir, &conversions, &cancel).await;
                let mut st = status.lock().unwrap();
                st.done += 1;
                match res {
                    Ok(Outcome::Added { kind, thumb, converted }) => {
                        st.added += 1;
                        if kind == Kind::Video { st.videos += 1 } else { st.photos += 1 }
                        if converted {
                            st.converted += 1;
                        }
                        if !thumb && ffmpeg.is_some() {
                            st.no_thumb += 1;
                        }
                    }
                    Ok(Outcome::Duplicate) => st.duplicates += 1,
                    Err(_) if cancel.load(Ordering::Relaxed) => {} // interrompu par l'annulation : ni ajouté, ni en échec
                    Err(e) => {
                        st.failed += 1;
                        if st.errors.len() < MAX_ERRORS {
                            st.errors.push(format!("{label} : {e}"));
                        }
                    }
                }
            }
        })
        .await;
}

#[allow(clippy::too_many_arguments)]
async fn import_one(
    media: &crate::kb::KnowledgeBase,
    f: &Found,
    opts: &ImportOptions,
    base_tags: &[String],
    ffmpeg: Option<&Path>,
    videos_dir: &Path,
    photos_dir: &Path,
    thumbs_dir: &Path,
    conversions: &tokio::sync::Semaphore,
    cancel: &AtomicBool,
) -> Result<Outcome, String> {
    let src = f.path.clone();
    let key = tokio::task::spawn_blocking(move || fingerprint(&src)).await.map_err(|e| e.to_string())?.map_err(|e| format!("lecture impossible ({e})"))?;
    if media.has_src_key(&key) {
        return Ok(Outcome::Duplicate);
    }
    let ext = f.path.extension().and_then(|x| x.to_str()).unwrap_or("").to_lowercase();
    let stem = f.path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    let name = safe_name(&stem, if f.convert { "mp4" } else { &ext });
    let (dir, url_dir) = if f.kind == Kind::Video { (videos_dir, "videos") } else { (photos_dir, "photos") };
    let dst = if f.convert {
        // Vidéo que le navigateur ne lit pas : convertie en MP4 (au plus deux conversions à la fois).
        let ff = ffmpeg.ok_or_else(|| "ffmpeg introuvable : conversion impossible".to_string())?;
        let _slot = conversions.acquire().await.map_err(|e| e.to_string())?;
        if cancel.load(Ordering::Relaxed) {
            return Err("annulé".into());
        }
        convert_to_mp4(ff, &f.path, dir, &name, &key, cancel).await?
    } else {
        let (src, dir2, name2, copy, key2) = (f.path.clone(), dir.to_path_buf(), name.clone(), opts.copy, key.clone());
        tokio::task::spawn_blocking(move || place(&src, &dir2, &name2, copy, &key2))
            .await
            .map_err(|e| e.to_string())?
            .map_err(|e| format!("copie impossible ({e})"))?
    };
    let final_name = dst.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or(name);
    let base = final_name.rsplit_once('.').map(|(b, _)| b.to_string()).unwrap_or_else(|| final_name.clone());
    let thumb_file = thumbs_dir.join(format!("thumb-{base}.png"));
    let has_thumb = match ffmpeg {
        // un WebP animé, que ffmpeg ne lit pas, a quand même la miniature de sa première image
        Some(ff) => thumbnail(ff, &dst, &thumb_file, f.kind).await || (ext == "webp" && animated_webp_thumbnail(ff, &dst, &thumb_file).await),
        None => false,
    };

    let mut tags = base_tags.to_vec();
    if opts.folder_tags {
        for t in folder_tags(&f.dirs) {
            if !tags.contains(&t) {
                tags.push(t);
            }
        }
    }
    let item = Source {
        id: 0,
        kind: if f.kind == Kind::Video { "video" } else { "photo" }.into(),
        url: format!("/{url_dir}/{final_name}"),
        thumb: has_thumb.then(|| format!("/thumbnails/thumb-{base}.png")),
        title: stem.replace(['_', '-'], " ").trim().to_string(),
        description: String::new(),
        tags,
        ..Default::default()
    };
    match media.insert_new(&item, Some(&key)) {
        Ok(Some(_)) => Ok(Outcome::Added { kind: f.kind, thumb: has_thumb, converted: f.convert }),
        Ok(None) => {
            // Même contenu importé en même temps par un autre fichier : on défait ce qu'on vient de créer.
            let _ = std::fs::remove_file(&dst);
            let _ = std::fs::remove_file(&thumb_file);
            Ok(Outcome::Duplicate)
        }
        Err(e) => {
            let _ = std::fs::remove_file(&dst);
            let _ = std::fs::remove_file(&thumb_file);
            Err(format!("base de données : {e}"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("repere-import-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn tags_des_dossiers_sans_noms_generiques_ni_nombres() {
        let dirs: Vec<String> = ["Videos", "Plage 2023", "2023-07", "Été à Nice", "DCIM", "été à nice"].iter().map(|s| s.to_string()).collect();
        assert_eq!(folder_tags(&dirs), vec!["été à nice", "plage 2023"]);
        assert!(folder_tags(&["x".to_string(), "".to_string()]).is_empty());
    }

    #[test]
    fn noms_de_fichiers_sans_espaces_ni_caracteres_speciaux() {
        assert_eq!(safe_name("Été à Nice (1)", "MP4"), "t_Nice_1.mp4");
        assert_eq!(safe_name("???", "jpg"), "media.jpg");
        assert!(safe_name(&"a".repeat(300), "png").len() <= 84);
    }

    #[test]
    fn parcours_classe_les_fichiers_et_compte_les_formats_illisibles() {
        let root = tmp("walk");
        let sub = root.join("plage").join("2023");
        std::fs::create_dir_all(&sub).unwrap();
        for f in ["a.mp4", "plage/b.MOV", "plage/2023/c.jpg", "plage/2023/d.heic", "plage/2023/e.avi", "plage/2023/f.txt", ".cache/g.mp4"] {
            let p = root.join(f);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, b"x").unwrap();
        }
        let mut u = BTreeMap::new();
        let found = walk(&root, true, &mut u);
        let mut names: Vec<_> = found.iter().map(|f| (f.path.file_name().unwrap().to_string_lossy().to_string(), f.kind)).collect();
        names.sort_by(|a, b| a.0.cmp(&b.0));
        assert_eq!(names, vec![("a.mp4".into(), Kind::Video), ("b.MOV".into(), Kind::Video), ("c.jpg".into(), Kind::Photo), ("e.avi".into(), Kind::Video)]);
        assert_eq!(u.get("heic"), Some(&1));
        assert_eq!(u.get("avi"), None, "les AVI sont importés (convertis en MP4), plus signalés comme illisibles");
        assert!(found.iter().find(|f| f.path.ends_with("e.avi")).unwrap().convert);
        assert!(found.iter().filter(|f| !f.path.ends_with("e.avi")).all(|f| !f.convert));
        let c = found.iter().find(|f| f.path.ends_with("c.jpg")).unwrap();
        assert_eq!(c.dirs.len(), 3); // racine, plage, 2023
        let mut u2 = BTreeMap::new();
        assert_eq!(walk(&root, false, &mut u2).len(), 1, "sans sous-dossiers : seulement a.mp4");
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn empreinte_stable_et_sensible_au_contenu() {
        let d = tmp("fp");
        let big: Vec<u8> = (0..300_000u32).map(|i| (i % 253) as u8).collect();
        std::fs::write(d.join("a"), &big).unwrap();
        std::fs::write(d.join("b"), &big).unwrap();
        let mut other = big.clone();
        *other.last_mut().unwrap() ^= 1;
        std::fs::write(d.join("c"), &other).unwrap();
        assert_eq!(fingerprint(&d.join("a")).unwrap(), fingerprint(&d.join("b")).unwrap());
        assert_ne!(fingerprint(&d.join("a")).unwrap(), fingerprint(&d.join("c")).unwrap());
        std::fs::write(d.join("small"), b"x").unwrap();
        assert!(fingerprint(&d.join("small")).is_ok());
        std::fs::remove_dir_all(d).ok();
    }

    fn found(path: &Path, kind: Kind, dirs: &[&str]) -> Found {
        let convert = path.extension().is_some_and(|e| e == "avi");
        Found { path: path.to_path_buf(), kind, dirs: dirs.iter().map(|s| s.to_string()).collect(), convert }
    }

    /// Les outils d'un import dans les tests : une conversion à la fois, pas d'annulation.
    fn outils() -> (tokio::sync::Semaphore, AtomicBool) {
        (tokio::sync::Semaphore::new(1), AtomicBool::new(false))
    }

    #[tokio::test]
    async fn import_d_un_lot_doublons_tags_et_noms_en_double() {
        let d = tmp("one");
        let (src, up) = (d.join("src"), d.join("uploads"));
        for sub in ["videos", "photos", "thumbnails"] {
            std::fs::create_dir_all(up.join(sub)).unwrap();
        }
        std::fs::create_dir_all(src.join("a")).unwrap();
        std::fs::create_dir_all(src.join("b")).unwrap();
        std::fs::write(src.join("a/Mon Film.mp4"), vec![1u8; 5000]).unwrap();
        std::fs::write(src.join("b/Mon Film.mp4"), vec![2u8; 6000]).unwrap(); // même nom, autre contenu
        std::fs::write(src.join("b/copie.mp4"), vec![1u8; 5000]).unwrap(); // même contenu que le premier
        std::fs::write(src.join("b/photo.JPG"), vec![3u8; 100]).unwrap();
        let lib = crate::kb::KnowledgeBase::open_in_memory().unwrap();
        let opts = ImportOptions { tags: vec!["Été".into()], ..Default::default() };
        let base = opts.tags.clone();
        let run_one = |p: PathBuf, kind: Kind, dirs: &[&str]| {
            let (lib, opts, base, up) = (&lib, opts.clone(), base.clone(), up.clone());
            let f = found(&p, kind, dirs);
            async move {
                let (sem, cancel) = outils();
                import_one(lib, &f, &opts, &base, None, &up.join("videos"), &up.join("photos"), &up.join("thumbnails"), &sem, &cancel).await
            }
        };
        assert!(matches!(run_one(src.join("a/Mon Film.mp4"), Kind::Video, &["src", "alpha"]).await, Ok(Outcome::Added { .. })));
        assert!(matches!(run_one(src.join("b/Mon Film.mp4"), Kind::Video, &["src", "beta"]).await, Ok(Outcome::Added { .. })));
        assert!(matches!(run_one(src.join("b/copie.mp4"), Kind::Video, &["src", "beta"]).await, Ok(Outcome::Duplicate)));
        assert!(matches!(run_one(src.join("b/photo.JPG"), Kind::Photo, &["src", "beta"]).await, Ok(Outcome::Added { .. })));
        // réimport : tout est reconnu
        assert!(matches!(run_one(src.join("a/Mon Film.mp4"), Kind::Video, &["src", "alpha"]).await, Ok(Outcome::Duplicate)));

        let items = lib.list().unwrap();
        assert_eq!(items.len(), 3);
        let urls: Vec<_> = items.iter().map(|m| m.url.as_str()).collect();
        assert!(urls.contains(&"/videos/Mon_Film.mp4") && urls.iter().any(|u| u.starts_with("/videos/Mon_Film-")) && urls.contains(&"/photos/photo.jpg"));
        let first = &items[0];
        assert_eq!(first.title, "Mon Film");
        assert_eq!(first.tags, vec!["été", "alpha", "src"]); // lot + dossiers
        assert_eq!(std::fs::read_dir(up.join("videos")).unwrap().count(), 2, "le doublon n'a laissé aucun fichier");
        std::fs::remove_dir_all(d).ok();
    }

    #[tokio::test]
    async fn miniature_creee_par_ffmpeg_si_present() {
        let Some(ff) = ffmpeg_path() else { return }; // pas de ffmpeg sur cette machine : rien à vérifier
        let d = tmp("thumb");
        let vid = d.join("clip.mp4");
        let ok = std::process::Command::new(&ff)
            .args(["-hide_banner", "-loglevel", "error", "-f", "lavfi", "-i", "testsrc=size=640x360:rate=10:duration=2", "-pix_fmt", "yuv420p", "-y"])
            .arg(&vid).status().is_ok_and(|s| s.success());
        if !ok { return } // ffmpeg sans libx264/mpeg4 : on n'invente pas
        let out = d.join("t.png");
        assert!(thumbnail(&ff, &vid, &out, Kind::Video).await);
        assert!(out.metadata().unwrap().len() > 100);
        let img = d.join("p.png");
        assert!(thumbnail(&ff, &out, &img, Kind::Photo).await);
        assert!(!thumbnail(&ff, &d.join("absent.mp4"), &d.join("x.png"), Kind::Video).await);
        std::fs::remove_dir_all(d).ok();
    }

    /// Un lot d'import prêt : dossier source, dossiers servis, médiathèque.
    fn atelier(name: &str) -> (PathBuf, PathBuf, PathBuf, crate::kb::KnowledgeBase) {
        let d = tmp(name);
        let (src, up) = (d.join("src"), d.join("uploads"));
        for sub in ["videos", "photos", "thumbnails"] {
            std::fs::create_dir_all(up.join(sub)).unwrap();
        }
        std::fs::create_dir_all(&src).unwrap();
        (d, src, up, crate::kb::KnowledgeBase::open_in_memory().unwrap())
    }

    async fn importer_un(lib: &crate::kb::KnowledgeBase, up: &Path, f: &Found, ff: Option<&Path>, cancel: &AtomicBool) -> Result<Outcome, String> {
        let sem = tokio::sync::Semaphore::new(1);
        import_one(lib, f, &ImportOptions::default(), &[], ff, &up.join("videos"), &up.join("photos"), &up.join("thumbnails"), &sem, cancel).await
    }

    fn lavfi(ff: &Path, out: &Path, extra: &[&str]) -> bool {
        std::process::Command::new(ff)
            .args(["-hide_banner", "-loglevel", "error", "-f", "lavfi", "-i", "testsrc2=size=321x181:rate=25:duration=2"])
            .args(extra)
            .arg("-y")
            .arg(out)
            .status()
            .is_ok_and(|s| s.success())
    }

    #[tokio::test]
    async fn un_avi_est_converti_en_mp4_lisible_par_le_navigateur() {
        let Some(ff) = ffmpeg_path() else { return };
        let (d, src, up, lib) = atelier("avi");
        let avi = src.join("Mon Film (1).avi");
        // le contenu typique d'un AVI : MPEG-4 (DivX/Xvid) et dimensions impaires, que ni H.264 ni le navigateur n'acceptent tels quels
        if !lavfi(&ff, &avi, &["-c:v", "mpeg4", "-q:v", "5", "-an"]) { return }
        let f = found(&avi, Kind::Video, &["src", "vacances"]);
        let cancel = AtomicBool::new(false);
        assert!(matches!(importer_un(&lib, &up, &f, Some(&ff), &cancel).await, Ok(Outcome::Added { kind: Kind::Video, converted: true, thumb: true })));
        let items = lib.list().unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!((items[0].kind.as_str(), items[0].url.as_str(), items[0].title.as_str()), ("video", "/videos/Mon_Film_1.mp4", "Mon Film (1)"));
        assert_eq!(items[0].tags, vec!["vacances", "src"]);
        assert!(items[0].thumb.as_deref().is_some_and(|t| up.join(t.trim_start_matches('/')).exists()), "miniature créée");
        // le MP4 est du H.264 en yuv420p aux dimensions paires, et se décode sans erreur
        let mp4 = up.join("videos/Mon_Film_1.mp4");
        let info = std::process::Command::new(&ff).args(["-hide_banner", "-i"]).arg(&mp4).output().unwrap();
        let info = String::from_utf8_lossy(&info.stderr).to_string();
        assert!(info.contains("Video: h264") && info.contains("yuv420p") && info.contains("320x180"), "{info}");
        assert!(std::process::Command::new(&ff).args(["-v", "error", "-i"]).arg(&mp4).args(["-f", "null", "-"]).status().unwrap().success());
        // l'AVI d'origine n'est pas touché, rien ne traîne, et un second import le reconnaît
        assert!(avi.exists());
        assert_eq!(std::fs::read_dir(up.join("videos")).unwrap().count(), 1, "pas de fichier temporaire");
        assert!(matches!(importer_un(&lib, &up, &f, Some(&ff), &cancel).await, Ok(Outcome::Duplicate)));
        assert_eq!(std::fs::read_dir(up.join("videos")).unwrap().count(), 1);
        std::fs::remove_dir_all(d).ok();
    }

    #[tokio::test]
    async fn un_avi_avec_son_est_converti_avec_son() {
        let Some(ff) = ffmpeg_path() else { return };
        let (d, src, up, lib) = atelier("avi-son");
        let avi = src.join("avec son.avi");
        if !lavfi(&ff, &avi, &["-f", "lavfi", "-i", "sine=frequency=440:duration=2", "-c:v", "mpeg4", "-q:v", "5", "-c:a", "libmp3lame", "-shortest"]) { return }
        let f = found(&avi, Kind::Video, &["src"]);
        assert!(matches!(importer_un(&lib, &up, &f, Some(&ff), &AtomicBool::new(false)).await, Ok(Outcome::Added { converted: true, .. })));
        let info = std::process::Command::new(&ff).args(["-hide_banner", "-i"]).arg(up.join("videos/avec_son.mp4")).output().unwrap();
        assert!(String::from_utf8_lossy(&info.stderr).contains("Audio: aac"), "le son est gardé, en AAC");
        std::fs::remove_dir_all(d).ok();
    }

    #[tokio::test]
    async fn un_avi_illisible_ou_une_annulation_ne_laisse_rien() {
        let Some(ff) = ffmpeg_path() else { return };
        let (d, src, up, lib) = atelier("avi-ko");
        let casse = src.join("casse.avi");
        std::fs::write(&casse, b"RIFF\x10\x00\x00\x00AVI LIST pas du tout une video").unwrap();
        let f = found(&casse, Kind::Video, &["src"]);
        let err = match importer_un(&lib, &up, &f, Some(&ff), &AtomicBool::new(false)).await {
            Err(e) => e,
            Ok(_) => panic!("un AVI illisible ne doit pas être ajouté"),
        };
        assert!(err.starts_with("ffmpeg"), "{err}");
        assert_eq!(std::fs::read_dir(up.join("videos")).unwrap().count(), 0, "ni fichier ni fichier temporaire");
        assert!(lib.list().unwrap().is_empty());
        // import annulé avant la conversion
        let bon = src.join("bon.avi");
        if lavfi(&ff, &bon, &["-c:v", "mpeg4", "-q:v", "5", "-an"]) {
            let f = found(&bon, Kind::Video, &["src"]);
            assert_eq!(importer_un(&lib, &up, &f, Some(&ff), &AtomicBool::new(true)).await.err().as_deref(), Some("annulé"));
            assert_eq!(std::fs::read_dir(up.join("videos")).unwrap().count(), 0);
            // sans ffmpeg : pas de conversion possible
            assert!(importer_un(&lib, &up, &f, None, &AtomicBool::new(false)).await.err().is_some_and(|e| e.contains("ffmpeg introuvable")));
        }
        std::fs::remove_dir_all(d).ok();
    }

    #[test]
    fn sans_ffmpeg_les_avi_sont_signales_comme_non_importes() {
        let f = |name: &str, convert| Found { path: PathBuf::from(name), kind: Kind::Video, dirs: vec![], convert };
        let mut u = BTreeMap::new();
        let kept = without_conversions(vec![f("/a/x.mp4", false), f("/a/y.AVI", true), f("/a/z.avi", true)], &mut u);
        assert_eq!(kept.len(), 1);
        assert_eq!(u.get("avi"), Some(&2));
    }

    #[tokio::test]
    async fn les_webp_fixes_et_animes_sont_importes_avec_leur_miniature() {
        let Some(ff) = ffmpeg_path() else { return };
        let (d, src, up, lib) = atelier("webp");
        let (fixe, anime) = (src.join("fixe.WEBP"), src.join("anime.webp"));
        let ok = lavfi(&ff, &fixe, &["-frames:v", "1"]) && lavfi(&ff, &anime, &["-c:v", "libwebp_anim", "-loop", "0"]);
        if !ok { return } // ffmpeg sans encodeur WebP
        let cancel = AtomicBool::new(false);
        for p in [&fixe, &anime] {
            let f = found(p, Kind::Photo, &["src"]);
            assert!(matches!(importer_un(&lib, &up, &f, Some(&ff), &cancel).await, Ok(Outcome::Added { kind: Kind::Photo, thumb: true, converted: false })), "{}", p.display());
        }
        let urls: Vec<String> = lib.list().unwrap().into_iter().map(|m| m.url).collect();
        assert_eq!(urls, vec!["/photos/fixe.webp", "/photos/anime.webp"]);
        assert_eq!(std::fs::read_dir(up.join("thumbnails")).unwrap().count(), 2, "une miniature chacun, y compris pour le WebP animé");
        let mut unsupported = BTreeMap::new();
        let found_files = walk(&src, true, &mut unsupported);
        assert_eq!(found_files.len(), 2, "les extensions en majuscules comptent aussi");
        assert!(found_files.iter().all(|f| f.kind == Kind::Photo && !f.convert) && unsupported.is_empty());
        std::fs::remove_dir_all(d).ok();
    }

    #[test]
    fn placement_par_lien_sans_jamais_ecraser() {
        let d = tmp("place");
        let (src, dst) = (d.join("src"), d.join("dst"));
        std::fs::create_dir_all(&src).unwrap();
        std::fs::create_dir_all(&dst).unwrap();
        std::fs::write(src.join("a.mp4"), b"un").unwrap();
        std::fs::write(src.join("b.mp4"), b"deux").unwrap();
        let k = "0123456789abcdef0123456789abcdef";
        let p1 = place(&src.join("a.mp4"), &dst, "film.mp4", false, k).unwrap();
        let p2 = place(&src.join("b.mp4"), &dst, "film.mp4", false, k).unwrap();
        assert_eq!(p1.file_name().unwrap(), "film.mp4");
        assert_eq!(p2.file_name().unwrap(), "film-01234567.mp4", "nom pris : suffixe, l'existant n'est pas touché");
        assert_eq!(std::fs::read(&p1).unwrap(), b"un");
        assert_eq!(std::fs::read(&p2).unwrap(), b"deux");
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            assert_eq!(std::fs::metadata(&p1).unwrap().nlink(), 2, "lien physique : le fichier n'est pas dupliqué");
        }
        let p3 = place(&src.join("a.mp4"), &dst, "copie.mp4", true, k).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            assert_eq!(std::fs::metadata(&p3).unwrap().nlink(), 1, "copie demandée");
        }
        assert_eq!(std::fs::read(p3).unwrap(), b"un");
        std::fs::remove_dir_all(d).ok();
    }
}
