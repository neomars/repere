//! Analyse des images : un modèle de vision (llama-server lancé avec son projecteur « mmproj ») regarde chaque image de la base
//! de connaissances — capture d'écran, message d'erreur, écran de configuration, schéma — et en tire une description et des
//! mots-clés qui permettent de la retrouver. Les vidéos ne sont pas analysées.
//!
//! Le travail tourne en tâche de fond, une image à la fois (le GPU ne fait qu'une chose à la fois), et chaque résultat est
//! enregistré tout de suite : on peut annuler, relancer, et reprendre là où l'on s'était arrêté (« nouveaux fichiers »).

use crate::config::LlmConfig;
use crate::engine::Engine;
use crate::kb::{Analysis, Source};
use crate::models::Kind;
use crate::state::AppState;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const MAX_ERRORS: usize = 60;
/// Arrêt si le serveur ne répond plus de suite autant de fois : inutile de continuer à vide.
const GIVE_UP_AFTER: usize = 5;
const PHOTO_SIDE: u32 = 1024;

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct AnalyzeOptions {
    /// `new` (jamais analysées), `all` (tout refaire), `ids` (la sélection).
    pub scope: String,
    pub ids: Vec<i64>,
    /// Remplacer les tags (sinon : les tags s'ajoutent, l'existant est respecté).
    pub overwrite: bool,
    /// Pour essayer sur quelques images.
    pub limit: Option<usize>,
}

impl Default for AnalyzeOptions {
    fn default() -> Self {
        Self { scope: "new".into(), ids: vec![], overwrite: false, limit: None }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct LastResult {
    pub id: i64,
    pub title: String,
    pub kind: String,
    pub description: String,
    pub tags: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct AnalyzeStatus {
    pub running: bool,
    pub finished: bool,
    pub cancelled: bool,
    pub total: usize,
    pub done: usize,
    pub described: usize,
    pub failed: usize,
    pub current: String,
    pub model: String,
    pub errors: Vec<String>,
    /// Erreur qui a arrêté tout le travail.
    pub error: Option<String>,
    pub seconds: u64,
    pub last: Option<LastResult>,
}

pub struct Analyzer {
    status: Arc<Mutex<AnalyzeStatus>>,
    cancel: Arc<AtomicBool>,
}

/// Le modèle de texte chargé voit-il les images ? (`model` : son nom)
#[derive(Debug, Clone, Serialize)]
pub struct VisionInfo {
    pub ready: bool,
    pub model: String,
    /// Pourquoi ce n'est pas prêt, en français.
    pub reason: Option<String>,
}

impl Analyzer {
    pub fn new() -> Self {
        Self { status: Arc::new(Mutex::new(AnalyzeStatus::default())), cancel: Arc::new(AtomicBool::new(false)) }
    }

    pub fn status(&self) -> AnalyzeStatus {
        self.status.lock().unwrap().clone()
    }

    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    /// Lance l'analyse. Refuse tout de suite (avec la raison) si aucun modèle de vision n'est chargé ou si un travail est en cours.
    pub async fn start(&self, app: Arc<AppState>, opts: AnalyzeOptions) -> Result<(), String> {
        if self.status.lock().unwrap().running {
            return Err("une analyse est déjà en cours".into());
        }
        let info = vision_info(&app).await;
        if !info.ready {
            return Err(info.reason.unwrap_or_else(|| "aucun modèle de vision n'est chargé".into()));
        }
        let items = app
            .kb
            .to_analyze(&opts.scope, &opts.ids, Some("photo"), opts.limit)
            .map_err(|e| format!("médiathèque illisible : {e}"))?;
        {
            let mut st = self.status.lock().unwrap();
            if st.running {
                return Err("une analyse est déjà en cours".into());
            }
            *st = AnalyzeStatus { running: true, total: items.len(), model: info.model.clone(), ..Default::default() };
        }
        self.cancel.store(false, Ordering::Relaxed);
        let (status, cancel) = (self.status.clone(), self.cancel.clone());
        tokio::spawn(async move {
            let started = Instant::now();
            run(app, items, opts, info, status.clone(), cancel.clone(), started).await;
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

/// Le serveur de texte voit-il les images ? On le demande à llama-server lui-même (`/props` → `modalities.vision`), ce qui
/// marche aussi pour un serveur lancé à la main.
pub async fn vision_info(app: &AppState) -> VisionInfo {
    let http = reqwest::Client::builder().timeout(Duration::from_secs(3)).build().unwrap_or_default();
    let props: Option<Value> = match http.get(format!("{}/props", app.cfg.llm.url.trim_end_matches('/'))).send().await {
        Ok(r) if r.status().is_success() => r.json().await.ok(),
        _ => None,
    };
    let model = loaded_model(&app.engine, &app.models);
    let no = |why: &str| VisionInfo { ready: false, model: model.clone(), reason: Some(why.to_string()) };
    let Some(props) = props else {
        return no("Le modèle de texte n'est pas chargé : charge-le dans Admin → ⚙ Modèles IA (avec la vision).");
    };
    if props["modalities"]["vision"].as_bool() != Some(true) {
        return no("Le modèle chargé ne voit pas les images : dans Admin → ⚙ Modèles IA, charge-le « avec la vision » (ou télécharge d'abord sa vision).");
    }
    VisionInfo { ready: true, model, reason: None }
}

fn loaded_model(engine: &Engine, models: &crate::models::Models) -> String {
    match engine.status_of(Kind::Llm).model.and_then(|id| models.entry(&id).cloned()) {
        Some(e) => e.name,
        None => "modèle de vision externe".to_string(),
    }
}

// ------------------------------------------------------------------------------------------------ requêtes au modèle

#[derive(Clone)]
struct Vlm {
    http: reqwest::Client,
    base: String,
    model: String,
}

impl Vlm {
    fn new(cfg: &LlmConfig) -> Self {
        Self {
            http: reqwest::Client::builder().connect_timeout(Duration::from_secs(5)).timeout(Duration::from_secs(300)).build().unwrap_or_default(),
            base: cfg.url.trim_end_matches('/').to_string(),
            model: cfg.model.clone(),
        }
    }

    /// Une question au modèle, avec ou sans image (JPEG). Un échec passager est retenté une fois.
    async fn ask(&self, system: &str, prompt: &str, image: Option<&[u8]>, max_tokens: u32) -> Result<String, ItemError> {
        let user = match image {
            Some(img) => json!([
                {"type": "text", "text": prompt},
                {"type": "image_url", "image_url": {"url": format!("data:image/jpeg;base64,{}", base64(img))}}
            ]),
            None => json!(prompt),
        };
        let body = json!({
            "model": self.model,
            "messages": [{"role": "system", "content": system}, {"role": "user", "content": user}],
            "temperature": 0.2,
            "top_p": 0.9,
            "max_tokens": max_tokens,
            "stream": false,
        });
        let mut last = String::new();
        for attempt in 0..2 {
            if attempt > 0 {
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
            match self.http.post(format!("{}/v1/chat/completions", self.base)).json(&body).send().await {
                Ok(r) if r.status().is_success() => {
                    let v: Value = r.json().await.map_err(|e| ItemError::Vlm(format!("réponse illisible : {e}")))?;
                    return Ok(v["choices"][0]["message"]["content"].as_str().unwrap_or("").trim().to_string());
                }
                Ok(r) => {
                    let code = r.status();
                    let text = r.text().await.unwrap_or_default();
                    last = format!("HTTP {code} : {}", text.chars().take(200).collect::<String>());
                    if code.is_client_error() {
                        break; // une requête refusée ne s'arrange pas en la répétant
                    }
                }
                Err(e) => last = format!("serveur de vision injoignable ({e})"),
            }
        }
        Err(ItemError::Vlm(last))
    }
}

#[derive(Debug)]
enum ItemError {
    Cancelled,
    /// Le serveur de vision ne répond pas ou renvoie une erreur.
    Vlm(String),
    /// Fichier introuvable, ffmpeg absent ou en échec.
    Media(String),
}

impl ItemError {
    fn text(&self) -> String {
        match self {
            ItemError::Cancelled => "annulé".into(),
            ItemError::Vlm(s) | ItemError::Media(s) => s.clone(),
        }
    }
}

// ------------------------------------------------------------------------------------------------ consignes

const SYSTEM: &str = "Tu catalogues les images d'une base de connaissances d'assistance informatique : captures d'écran de logiciels, messages d'erreur, \
    écrans de configuration, schémas, photos de matériel. Tu décris exactement ce que tu vois, de façon factuelle et précise. Tu recopies tels quels les \
    messages d'erreur, les codes et les libellés importants. Tu n'inventes rien qui ne soit pas visible. Tu ne recopies jamais un mot de passe, un code \
    secret ni une donnée personnelle visible (nom, adresse e-mail, numéro de téléphone) : tu écris « [masqué] » à la place. Tu réponds toujours en français.";

const JSON_FIELDS: &str = "{\n  \"description\": \"2 à 5 phrases : l'application ou le matériel, ce qui est affiché (message d'erreur, code et libellés recopiés \
    exactement), et à quelle étape d'une procédure cela correspond\",\n  \"tags\": [\"6 à 12 mots-clés en minuscules : application, message d'erreur, \
    code d'erreur, composant, action\"]\n}";

fn photo_prompt() -> String {
    format!(
        "Analyse cette image et réponds UNIQUEMENT par un objet JSON valide, sans texte autour ni balises de code :\n{JSON_FIELDS}\n\
         N'ajoute rien qui ne figure pas sur l'image."
    )
}

// ------------------------------------------------------------------------------------------------ lecture des réponses

/// Extrait l'objet JSON d'une réponse (le modèle l'entoure parfois de texte ou de balises ```).
fn extract_json(raw: &str) -> Option<Value> {
    let start = raw.find('{')?;
    let end = raw.rfind('}')?;
    if end <= start {
        return None;
    }
    serde_json::from_str(&raw[start..=end]).ok()
}

pub fn clean_tag(t: &str) -> Option<String> {
    let t = t.trim().trim_start_matches('#').replace('_', " ").to_lowercase();
    let t = t.trim_matches(|c: char| !c.is_alphanumeric()).split_whitespace().collect::<Vec<_>>().join(" ");
    let n = t.chars().count();
    (2..=40).contains(&n).then_some(t).filter(|t| t.split(' ').count() <= 4)
}

/// Tags d'une liste JSON ou d'un texte « a, b, c » : propres, sans doublon, 14 au plus.
fn parse_tags(v: &Value) -> Vec<String> {
    let raw: Vec<String> = match v {
        Value::Array(a) => a.iter().filter_map(|x| x.as_str().map(String::from)).collect(),
        Value::String(s) => s.split([',', '\n']).map(String::from).collect(),
        _ => vec![],
    };
    let mut out: Vec<String> = vec![];
    for t in raw.iter().filter_map(|t| clean_tag(t)) {
        if !out.contains(&t) {
            out.push(t);
        }
        if out.len() == 14 {
            break;
        }
    }
    out
}

#[derive(Debug, PartialEq)]
enum Parsed {
    Data { description: String, tags: Vec<String> },
    /// Pas de JSON : le texte lui-même sert de description.
    Prose(String),
}

fn tidy(text: &str, max: usize) -> String {
    let t = text.split_whitespace().collect::<Vec<_>>().join(" ");
    t.chars().take(max).collect()
}

fn parse_reply(raw: &str) -> Parsed {
    let Some(j) = extract_json(raw) else { return Parsed::Prose(tidy(raw, 1500)) };
    let description = j["description"].as_str().map(|d| tidy(d, 1500)).unwrap_or_default();
    if description.is_empty() {
        return Parsed::Prose(tidy(raw, 1500));
    }
    Parsed::Data { description, tags: parse_tags(&j["tags"]) }
}

// ------------------------------------------------------------------------------------------------ ffmpeg

async fn ffmpeg_output(ff: &Path, args: &[String], limit: Duration) -> Result<Vec<u8>, ItemError> {
    let mut cmd = tokio::process::Command::new(ff);
    cmd.args(["-hide_banner", "-loglevel", "error"]).args(args)
        .env_remove("LD_LIBRARY_PATH")
        .stdin(std::process::Stdio::null()).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    let child = cmd.spawn().map_err(|e| ItemError::Media(format!("ffmpeg introuvable ({e})")))?;
    match tokio::time::timeout(limit, child.wait_with_output()).await {
        Err(_) => Err(ItemError::Media("ffmpeg trop long (fichier illisible ?)".into())),
        Ok(Err(e)) => Err(ItemError::Media(format!("ffmpeg : {e}"))),
        Ok(Ok(o)) if o.status.success() && !o.stdout.is_empty() => Ok(o.stdout),
        Ok(Ok(o)) => {
            let why = String::from_utf8_lossy(&o.stderr);
            Err(ItemError::Media(format!("ffmpeg n'a rien produit : {}", why.lines().last().unwrap_or("fichier illisible").chars().take(160).collect::<String>())))
        }
    }
}

/// Une image JPEG du fichier, réduite à `side` pixels au plus sur son grand côté.
/// Un WebP animé, que ffmpeg ne lit pas, est lu par sa première image.
async fn jpeg_of(ff: Option<&Path>, path: &Path, side: u32) -> Result<Vec<u8>, ItemError> {
    let first = jpeg_from(ff, path, side).await;
    let webp = path.extension().and_then(|e| e.to_str()).is_some_and(|e| e.eq_ignore_ascii_case("webp"));
    if first.is_err() && webp && ff.is_some() {
        let p = path.to_path_buf();
        if let Ok(Some(still)) = tokio::task::spawn_blocking(move || crate::webp::still_copy(&p)).await {
            let second = jpeg_from(ff, &still, side).await;
            let _ = std::fs::remove_file(&still);
            return second;
        }
    }
    first
}

async fn jpeg_from(ff: Option<&Path>, path: &Path, side: u32) -> Result<Vec<u8>, ItemError> {
    let Some(ff) = ff else {
        // Sans ffmpeg : on envoie le fichier tel quel s'il s'agit d'un JPEG ou d'un PNG de taille raisonnable.
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase();
        return match (ext.as_str(), std::fs::metadata(path).map(|m| m.len())) {
            ("jpg" | "jpeg" | "png", Ok(n)) if n <= 6_000_000 => std::fs::read(path).map_err(|e| ItemError::Media(e.to_string())),
            _ => Err(ItemError::Media("ffmpeg est nécessaire pour lire cette image (introuvable)".into())),
        };
    };
    let scale = format!("scale=w='if(gt(a,1),min({side},iw),-2)':h='if(gt(a,1),-2,min({side},ih))'");
    let args: Vec<String> = vec![
        "-i".into(), path.to_string_lossy().to_string(), "-frames:v".into(), "1".into(), "-vf".into(), scale,
        "-q:v".into(), "3".into(), "-f".into(), "mjpeg".into(), "-".into(),
    ];
    ffmpeg_output(ff, &args, Duration::from_secs(60)).await
}

pub fn base64(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for c in data.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        out.push(T[(n >> 18) as usize & 63] as char);
        out.push(T[(n >> 12) as usize & 63] as char);
        out.push(if c.len() > 1 { T[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if c.len() > 2 { T[n as usize & 63] as char } else { '=' });
    }
    out
}

// ------------------------------------------------------------------------------------------------ une image

struct Ctx<'a> {
    vlm: &'a Vlm,
    ff: Option<&'a Path>,
    cancel: &'a AtomicBool,
}

impl Ctx<'_> {
    fn check(&self) -> Result<(), ItemError> {
        if self.cancel.load(Ordering::Relaxed) { Err(ItemError::Cancelled) } else { Ok(()) }
    }

    async fn photo(&self, path: &Path) -> Result<Analysis, ItemError> {
        let img = jpeg_of(self.ff, path, PHOTO_SIDE).await?;
        self.check()?;
        let raw = self.vlm.ask(SYSTEM, &photo_prompt(), Some(&img), 900).await?;
        Ok(match parse_reply(&raw) {
            Parsed::Data { description, tags } => Analysis { description, tags },
            Parsed::Prose(description) => Analysis { description, tags: vec![] },
        })
    }
}

/// Fichier d'un média : `url` (« /images/x.png ») dans le dossier d'uploads, sans pouvoir en sortir.
pub fn media_path(uploads: &Path, url: &str) -> Option<PathBuf> {
    let rel = url.trim_start_matches('/');
    if rel.is_empty() || rel.split('/').any(|c| c == ".." || c.is_empty() || c.contains('\\')) {
        return None;
    }
    let p = uploads.join(rel);
    p.is_file().then_some(p)
}

async fn run(app: Arc<AppState>, items: Vec<Source>, opts: AnalyzeOptions, info: VisionInfo, status: Arc<Mutex<AnalyzeStatus>>, cancel: Arc<AtomicBool>, started: Instant) {
    let vlm = Vlm::new(&app.cfg.llm);
    let ff = crate::importer::ffmpeg_path();
    let ctx = Ctx { vlm: &vlm, ff: ff.as_deref(), cancel: &cancel };
    let mut broken_run = 0usize;

    for item in items {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        let label = if item.title.is_empty() { item.url.clone() } else { item.title.clone() };
        {
            let mut st = status.lock().unwrap();
            st.current = label.clone();
            st.seconds = started.elapsed().as_secs();
        }
        let res = match media_path(&app.cfg.server.uploads_dir, &item.url) {
            None => Err(ItemError::Media("fichier introuvable dans le dossier des médias".into())),
            Some(p) => ctx.photo(&p).await,
        };
        match res {
            Ok(a) => {
                broken_run = 0;
                let saved = app.kb.apply_analysis(item.id, &a, &info.model, opts.overwrite);
                let mut st = status.lock().unwrap();
                st.done += 1;
                match saved {
                    Ok(Some(m)) => {
                        st.described += 1;
                        st.last = Some(LastResult { id: m.id, title: label, kind: m.kind, description: m.description, tags: m.tags });
                    }
                    Ok(None) => {} // retirée de la base pendant l'analyse
                    Err(e) => {
                        st.failed += 1;
                        if st.errors.len() < MAX_ERRORS { st.errors.push(format!("{label} : base de données : {e}")); }
                    }
                }
            }
            Err(ItemError::Cancelled) => break,
            Err(e) => {
                let mut st = status.lock().unwrap();
                st.done += 1;
                st.failed += 1;
                if matches!(e, ItemError::Vlm(_)) { broken_run += 1 } else { broken_run = 0 }
                if st.errors.len() < MAX_ERRORS {
                    st.errors.push(format!("{label} : {}", e.text()));
                }
                if broken_run >= GIVE_UP_AFTER {
                    st.error = Some("Le serveur de vision ne répond plus (mémoire GPU saturée ? voir les journaux du modèle de texte).".into());
                    break;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_standard() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
        assert_eq!(base64(&[0xff, 0xfe, 0xfd, 0x00]), "//79AA==");
    }

    #[test]
    fn reponse_json_du_modele_lue_proprement() {
        let raw = "```json\n{\"description\": \"Fenêtre du client VPN : le message « Échec de l'authentification (erreur 691) » s'affiche.\\nLe mot de passe est [masqué].\", \"tags\": [\"VPN\", \"#erreur 691\", \"authentification\", \"vpn\", \"une phrase beaucoup trop longue pour etre un tag\", \"x\"]}\n```";
        match parse_reply(raw) {
            Parsed::Data { description, tags } => {
                assert_eq!(description, "Fenêtre du client VPN : le message « Échec de l'authentification (erreur 691) » s'affiche. Le mot de passe est [masqué].");
                assert_eq!(tags, vec!["vpn", "erreur 691", "authentification"], "minuscules, sans #, sans doublon, sans phrase");
            }
            other => panic!("{other:?}"),
        }
        // pas de JSON, ou JSON sans description : le texte sert de description
        assert_eq!(parse_reply("Une fenêtre d'erreur Windows."), Parsed::Prose("Une fenêtre d'erreur Windows.".into()));
        assert!(matches!(parse_reply("{\"tags\": [\"a\"]}"), Parsed::Prose(_)));
        assert!(matches!(parse_reply("{cassé"), Parsed::Prose(_)));
        // tags écrits « a, b_c, d »
        assert_eq!(parse_tags(&Value::String("vpn, mot_de_passe, erreur,\nréseau".into())), vec!["vpn", "mot de passe", "erreur", "réseau"]);
    }

    #[test]
    fn la_consigne_demande_du_json_et_interdit_de_recopier_les_secrets() {
        let p = photo_prompt();
        assert!(p.contains("JSON") && p.contains("description") && p.contains("tags"));
        assert!(SYSTEM.contains("messages d'erreur") && SYSTEM.contains("mot de passe") && SYSTEM.contains("[masqué]"));
    }

    #[test]
    fn chemins_de_medias_sans_sortie_du_dossier() {
        let d = std::env::temp_dir().join(format!("repere-media-path-{}", std::process::id()));
        std::fs::create_dir_all(d.join("images")).unwrap();
        std::fs::write(d.join("images/a.png"), b"x").unwrap();
        assert_eq!(media_path(&d, "/images/a.png"), Some(d.join("images/a.png")));
        assert_eq!(media_path(&d, "/images/../images/a.png"), None);
        assert_eq!(media_path(&d, "/images/absent.png"), None);
        assert_eq!(media_path(&d, "/"), None);
        assert_eq!(media_path(&d, "//etc/passwd"), None);
        std::fs::remove_dir_all(d).ok();
    }

    #[tokio::test]
    async fn sans_ffmpeg_seuls_les_jpeg_et_png_raisonnables_passent() {
        let d = std::env::temp_dir().join(format!("repere-jpeg-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("a.png"), b"png").unwrap();
        std::fs::write(d.join("b.gif"), b"gif").unwrap();
        assert_eq!(jpeg_of(None, &d.join("a.png"), 1024).await.unwrap(), b"png");
        let err = jpeg_of(None, &d.join("b.gif"), 1024).await.unwrap_err();
        assert!(err.text().contains("ffmpeg"), "{}", err.text());
        assert!(jpeg_of(None, &d.join("absent.png"), 1024).await.is_err());
        std::fs::remove_dir_all(d).ok();
    }

    /// Faux serveur de vision : renvoie `reply` et note chaque requête reçue.
    async fn fake_vlm(reply: &'static str) -> (String, Arc<Mutex<Vec<Value>>>) {
        use axum::{routing::post, Json, Router};
        let seen = Arc::new(Mutex::new(Vec::<Value>::new()));
        let s2 = seen.clone();
        let app = Router::new().route(
            "/v1/chat/completions",
            post(move |Json(req): Json<Value>| {
                let s = s2.clone();
                async move {
                    s.lock().unwrap().push(req);
                    Json(json!({"choices": [{"message": {"content": reply}, "finish_reason": "stop"}]}))
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        (url, seen)
    }

    async fn photo_with(url: &str, tag: &str) -> Result<Analysis, ItemError> {
        let dir = std::env::temp_dir().join(format!("repere-vlm-{}-{tag}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let img = dir.join("p.jpg");
        std::fs::write(&img, b"\xff\xd8\xff\xe0 faux jpeg").unwrap();
        let vlm = Vlm::new(&LlmConfig { url: url.to_string(), ..Default::default() });
        let cancel = AtomicBool::new(false);
        let ctx = Ctx { vlm: &vlm, ff: None, cancel: &cancel };
        let r = ctx.photo(&img).await;
        std::fs::remove_dir_all(dir).ok();
        r
    }

    #[tokio::test]
    async fn l_image_est_envoyee_au_modele_et_sa_reponse_est_lue() {
        let (url, seen) = fake_vlm(
            r#"{"description":"Message d'erreur 0x80070005 « Accès refusé » à l'ouverture d'un dossier partagé.","tags":["accès refusé","0x80070005","dossier partagé"]}"#,
        )
        .await;
        let a = photo_with(&url, "ok").await.unwrap();
        assert_eq!(a.description, "Message d'erreur 0x80070005 « Accès refusé » à l'ouverture d'un dossier partagé.");
        assert_eq!(a.tags, vec!["accès refusé", "0x80070005", "dossier partagé"]);
        let req = seen.lock().unwrap()[0].clone();
        assert!(req["messages"][0]["content"].as_str().unwrap().contains("base de connaissances"));
        let user = &req["messages"][1]["content"];
        assert!(user[0]["text"].as_str().unwrap().contains("JSON"));
        assert!(user[1]["image_url"]["url"].as_str().unwrap().starts_with("data:image/jpeg;base64,"));
        assert_eq!(seen.lock().unwrap().len(), 1, "une seule question par image");
    }

    #[tokio::test]
    async fn une_reponse_sans_json_sert_de_description() {
        let (url, _) = fake_vlm("Une fenêtre Outlook qui demande de saisir à nouveau les identifiants.").await;
        let a = photo_with(&url, "prose").await.unwrap();
        assert_eq!(a.description, "Une fenêtre Outlook qui demande de saisir à nouveau les identifiants.");
        assert!(a.tags.is_empty());
    }
}
