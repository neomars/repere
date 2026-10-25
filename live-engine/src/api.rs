//! Routes REST d'administration + point d'entrée WebSocket.

use crate::media::MediaPatch;
use crate::models::Kind;
use crate::persona::Persona;
use crate::state::AppState;
use crate::voice::TtsStyle;
use crate::ws::handle_socket;
use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, Path, Query, State, WebSocketUpgrade};
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;

type App = Arc<AppState>;
type ApiResult = Result<Json<Value>, (StatusCode, Json<Value>)>;

fn err(code: StatusCode, msg: impl ToString) -> (StatusCode, Json<Value>) {
    (code, Json(json!({ "message": msg.to_string() })))
}

fn authorize(app: &AppState, headers: &HeaderMap) -> Result<(), (StatusCode, Json<Value>)> {
    let Some(token) = &app.cfg.server.admin_token else { return Ok(()) };
    let ok = headers
        .get("authorization")
        .and_then(|h| h.to_str().ok())
        .and_then(|h| h.strip_prefix("Bearer "))
        .is_some_and(|t| t == token);
    if ok { Ok(()) } else { Err(err(StatusCode::UNAUTHORIZED, "jeton admin requis")) }
}

pub fn router() -> Router<App> {
    Router::new()
        .route("/health", get(health))
        .route("/plan", get(|State(a): State<App>| async move { Json(json!(a.plan())) }))
        .route("/models", get(models_list))
        .route("/models/{id}/download", post(model_download))
        .route("/models/{id}/vision", post(model_add_vision))
        .route("/models/{id}/cancel", post(model_cancel))
        .route("/models/{id}", axum::routing::delete(model_delete))
        .route("/engine/status", get(engine_status))
        .route("/engine/load", post(engine_load))
        .route("/engine/unload", post(engine_unload))
        .route("/engine/logs/{kind}", get(engine_logs))
        .route("/engine/tts-runtime/install", post(tts_runtime_install))
        .route("/hf-token", axum::routing::put(hf_token_put).delete(hf_token_delete))
        .route("/voices", get(voices_list))
        .route(
            "/voices/{name}",
            axum::routing::put(voice_put).delete(voice_delete).layer(DefaultBodyLimit::max(MAX_VOICE_BYTES)),
        )
        .route("/tts/preview", post(tts_preview))
        .route("/ws", get(ws_upgrade))
        .route("/media", get(media_list))
        .route("/media/analyze", get(media_analyze_status).post(media_analyze_start))
        .route("/media/analyze/cancel", post(media_analyze_cancel))
        .route("/media/import", get(media_import_status).post(media_import_start))
        .route("/media/import/cancel", post(media_import_cancel))
        .route("/media/delete", post(media_delete))
        .route("/media/scan", post(media_scan))
        .route("/media/bulk", post(media_bulk))
        .route("/media/{id}", axum::routing::patch(media_patch))
        .route("/conversations", get(conversations_list))
        .route("/personas", get(personas_list))
        .route("/personas/{id}", get(persona_get).put(persona_put).delete(persona_delete))
}

/// Un site web quelconque ne doit pas pouvoir ouvrir une session sur ton moteur local (et ton GPU) :
/// on n'accepte que les origines locales ou identiques à l'hôte demandé. Sans en-tête Origin
/// (client non-navigateur), on accepte.
pub fn origin_allowed(origin: Option<&str>, host: Option<&str>) -> bool {
    let Some(origin) = origin else { return true };
    let after = origin.split_once("://").map(|(_, r)| r).unwrap_or(origin);
    let origin_host = after.split('/').next().unwrap_or("");
    let hostname = |h: &str| -> String {
        if let Some(rest) = h.strip_prefix('[') { format!("[{}", rest.split(']').next().unwrap_or("") ) + "]" }
        else { h.split(':').next().unwrap_or("").to_lowercase() }
    };
    let oh = hostname(origin_host);
    matches!(oh.as_str(), "localhost" | "127.0.0.1" | "[::1]") || host.is_some_and(|h| hostname(h) == oh)
}

async fn ws_upgrade(ws: WebSocketUpgrade, headers: HeaderMap, State(app): State<App>) -> axum::response::Response {
    let get = |k: &str| headers.get(k).and_then(|v| v.to_str().ok());
    if !origin_allowed(get("origin"), get("host")) {
        return (StatusCode::FORBIDDEN, "origine non autorisée").into_response();
    }
    ws.on_upgrade(move |socket| handle_socket(socket, app)).into_response()
}

async fn health(State(a): State<App>) -> Json<Value> {
    let stt = match &a.stt { Some(s) => s.healthy().await, None => false };
    let tts = match &a.tts { Some(t) => t.healthy().await, None => false };
    Json(json!({
        "llm": a.llm.healthy().await, "stt": stt, "tts": tts,
        "voice": a.voice_status().await,
        "plan": a.plan(),
        "media_count": a.media.list().map(|l| l.len()).unwrap_or(0),
    }))
}

/// Sans paramètre : toute la médiathèque (tableau). Avec `limit` : une page, `{ total, items }`, filtrable
/// (`q` = mots cherchés dans titre/tags/description, `kind` = video|photo, `untagged=1`, `analysis` = todo|done, `offset`).
async fn media_list(State(a): State<App>, Query(q): Query<std::collections::HashMap<String, String>>) -> ApiResult {
    if q.contains_key("limit") {
        let f = crate::media::MediaFilter {
            text: q.get("q").cloned().unwrap_or_default(),
            kind: q.get("kind").filter(|k| matches!(k.as_str(), "video" | "photo")).cloned(),
            untagged: q.get("untagged").is_some_and(|v| v == "1" || v == "true"),
            analysis: q.get("analysis").filter(|v| matches!(v.as_str(), "todo" | "done")).cloned(),
            offset: q.get("offset").and_then(|v| v.parse().ok()).unwrap_or(0),
            limit: q.get("limit").and_then(|v| v.parse().ok()).unwrap_or(60),
        };
        let (total, items) = a.media.query(&f).map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e))?;
        return Ok(Json(json!({ "total": total, "items": items })));
    }
    let items = a.media.list().map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e))?;
    Ok(Json(json!(items)))
}

/// Retire des médias de la médiathèque. Avec `files: true`, efface aussi leurs fichiers (et miniatures) du disque : le lien dans
/// l'app, pas l'original d'un import « par lien ».
async fn media_delete(State(a): State<App>, h: HeaderMap, Json(b): Json<IdsBody>) -> ApiResult {
    authorize(&a, &h)?;
    let items: Vec<_> = if b.files { b.ids.iter().filter_map(|id| a.media.get(*id)).collect() } else { vec![] };
    let n = a.media.delete_many(&b.ids).map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e))?;
    let r = crate::files::remove_media_files(&a.cfg.server.uploads_dir, &items, &a.media);
    Ok(Json(json!({ "deleted": n, "files_removed": r.removed, "files_failed": r.failed })))
}

#[derive(Deserialize)]
struct IdsBody {
    ids: Vec<i64>,
    #[serde(default)]
    files: bool,
}

/// Avancement de l'analyse des médias, avec ce qu'il faut pour l'écran : combien sont déjà analysés, et le modèle de vision est-il prêt.
async fn analyze_state(a: &App) -> Value {
    let mut v = serde_json::to_value(a.analyzer.status()).unwrap_or(Value::Null);
    let (total, analyzed) = a.media.analysis_counts().unwrap_or((0, 0));
    v["counts"] = json!({ "total": total, "analyzed": analyzed, "todo": total.saturating_sub(analyzed) });
    v["vision"] = json!(crate::analyzer::vision_info(a).await);
    v
}

async fn media_analyze_status(State(a): State<App>) -> Json<Value> {
    Json(analyze_state(&a).await)
}

async fn media_analyze_start(State(a): State<App>, h: HeaderMap, Json(o): Json<crate::analyzer::AnalyzeOptions>) -> ApiResult {
    authorize(&a, &h)?;
    a.analyzer.start(a.clone(), o).await.map_err(|e| err(StatusCode::BAD_REQUEST, e))?;
    Ok(Json(analyze_state(&a).await))
}

async fn media_analyze_cancel(State(a): State<App>, h: HeaderMap) -> ApiResult {
    authorize(&a, &h)?;
    a.analyzer.cancel();
    Ok(Json(json!({ "cancelling": true })))
}

async fn media_import_status(State(a): State<App>) -> Json<Value> {
    Json(json!(a.importer.status()))
}

async fn media_import_start(State(a): State<App>, h: HeaderMap, Json(o): Json<crate::importer::ImportOptions>) -> ApiResult {
    authorize(&a, &h)?;
    a.importer.start(a.clone(), o).map_err(|e| err(StatusCode::BAD_REQUEST, e))?;
    Ok(Json(json!(a.importer.status())))
}

async fn media_import_cancel(State(a): State<App>, h: HeaderMap) -> ApiResult {
    authorize(&a, &h)?;
    a.importer.cancel();
    Ok(Json(json!({ "cancelling": true })))
}

async fn media_patch(State(a): State<App>, h: HeaderMap, Path(id): Path<i64>, Json(p): Json<MediaPatch>) -> ApiResult {
    authorize(&a, &h)?;
    match a.media.patch(id, &p).map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e))? {
        Some(m) => Ok(Json(json!(m))),
        None => Err(err(StatusCode::NOT_FOUND, "média introuvable")),
    }
}

#[derive(Deserialize)]
struct Bulk {
    ids: Vec<i64>,
    #[serde(flatten)]
    patch: MediaPatch,
}

async fn media_bulk(State(a): State<App>, h: HeaderMap, Json(b): Json<Bulk>) -> ApiResult {
    authorize(&a, &h)?;
    let mut n = 0;
    for id in b.ids {
        if a.media.patch(id, &b.patch).map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e))?.is_some() {
            n += 1;
        }
    }
    Ok(Json(json!({ "updated": n })))
}

async fn media_scan(State(a): State<App>, h: HeaderMap) -> ApiResult {
    authorize(&a, &h)?;
    let n = a.media.scan_uploads(&a.cfg.server.uploads_dir).map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e))?;
    Ok(Json(json!({ "added": n })))
}

/// Conversations sauvegardées (reprises automatiquement) : clé, profil d'assistant, nombre de messages, date.
async fn conversations_list(State(a): State<App>) -> Json<Value> {
    let items: Vec<Value> = a
        .conversations
        .list()
        .into_iter()
        .map(|(key, persona, messages, updated)| json!({"key": key, "persona": persona, "messages": messages, "updated": updated}))
        .collect();
    Json(json!(items))
}

async fn personas_list(State(a): State<App>) -> Json<Value> {
    Json(json!(a.personas.list()))
}

async fn persona_get(State(a): State<App>, Path(id): Path<String>) -> ApiResult {
    a.personas.get(&id).map(|p| Json(json!(p))).ok_or_else(|| err(StatusCode::NOT_FOUND, "profil d'assistant introuvable"))
}

async fn persona_put(State(a): State<App>, h: HeaderMap, Path(id): Path<String>, Json(mut p): Json<Persona>) -> ApiResult {
    authorize(&a, &h)?;
    p.id = id;
    a.personas.save(&p).map_err(|e| err(StatusCode::BAD_REQUEST, e))?;
    Ok(Json(json!(p.normalized())))
}

async fn persona_delete(State(a): State<App>, h: HeaderMap, Path(id): Path<String>) -> ApiResult {
    authorize(&a, &h)?;
    if a.personas.delete(&id) { Ok(Json(json!({"deleted": id}))) } else { Err(err(StatusCode::NOT_FOUND, "profil d'assistant introuvable")) }
}

// ---------- Modèles et moteur ----------

async fn models_list(State(a): State<App>) -> Json<Value> {
    let sel = a.engine.selection();
    let items: Vec<Value> = a
        .models
        .catalog
        .iter()
        .map(|e| {
            let inst = a.models.installed(&e.id);
            let active = [&sel.llm, &sel.stt, &sel.tts].iter().any(|s| s.as_deref() == Some(e.id.as_str()));
            json!({
                "entry": e,
                "installed": inst.is_some(),
                "size_bytes": inst.as_ref().map(|i| i.size_bytes),
                // Vision : le catalogue prévoit un projecteur ; est-il téléchargé ? (sinon « Ajouter la vision »)
                "vision": { "supported": e.mmproj.is_some(), "installed": inst.as_ref().is_some_and(|i| i.mmproj.is_some()),
                            "bytes": inst.as_ref().map(|i| i.mmproj_bytes) },
                "download": a.models.state(&e.id),
                "active": active,
            })
        })
        .collect();
    Json(json!(items))
}

async fn model_download(State(a): State<App>, h: HeaderMap, Path(id): Path<String>) -> ApiResult {
    authorize(&a, &h)?;
    let started = a.models.start_download(&id).map_err(|e| err(StatusCode::NOT_FOUND, e))?;
    Ok(Json(json!({ "started": started })))
}

/// Télécharge seulement le projecteur de vision d'un modèle déjà installé (le reste est reconnu comme complet).
async fn model_add_vision(State(a): State<App>, h: HeaderMap, Path(id): Path<String>) -> ApiResult {
    authorize(&a, &h)?;
    let started = a.models.start_download_opts(&id, true).map_err(|e| err(StatusCode::BAD_REQUEST, e))?;
    Ok(Json(json!({ "started": started })))
}

async fn model_cancel(State(a): State<App>, h: HeaderMap, Path(id): Path<String>) -> ApiResult {
    authorize(&a, &h)?;
    a.models.cancel(&id);
    Ok(Json(json!({ "cancelled": id })))
}

async fn model_delete(State(a): State<App>, h: HeaderMap, Path(id): Path<String>) -> ApiResult {
    authorize(&a, &h)?;
    // On décharge d'abord le modèle s'il est en cours d'utilisation (fichier ouvert par le serveur).
    if let Some(e) = a.models.entry(&id) {
        let kind = e.kind;
        if a.engine.status_of(kind).model.as_deref() == Some(id.as_str()) {
            a.engine.unload(kind).await;
        }
    }
    if a.models.delete(&id) { Ok(Json(json!({ "deleted": id }))) } else { Err(err(StatusCode::NOT_FOUND, "modèle introuvable")) }
}

async fn engine_status(State(a): State<App>) -> Json<Value> {
    Json(a.engine.status_json().await)
}

#[derive(Deserialize)]
struct LoadReq {
    id: String,
    /// Charger le modèle de texte avec sa vision (il voit les images).
    #[serde(default)]
    vision: Option<bool>,
}

async fn engine_load(State(a): State<App>, h: HeaderMap, Json(r): Json<LoadReq>) -> ApiResult {
    authorize(&a, &h)?;
    a.engine.load_opts(&r.id, r.vision).await.map_err(|e| err(StatusCode::BAD_REQUEST, e))?;
    Ok(Json(json!({ "loading": r.id })))
}

#[derive(Deserialize)]
struct UnloadReq {
    kind: Kind,
}

async fn engine_unload(State(a): State<App>, h: HeaderMap, Json(r): Json<UnloadReq>) -> ApiResult {
    authorize(&a, &h)?;
    a.engine.unload(r.kind).await;
    Ok(Json(json!({ "unloaded": r.kind })))
}

async fn engine_logs(State(a): State<App>, Path(kind): Path<String>) -> ApiResult {
    let k = match kind.as_str() {
        "llm" => Kind::Llm,
        "stt" => Kind::Stt,
        "tts" => Kind::Tts,
        _ => return Err(err(StatusCode::NOT_FOUND, "composant inconnu")),
    };
    Ok(Json(json!({ "lines": a.engine.logs(k, 200) })))
}

// ---------- Voix de référence et synthèse ----------

const MAX_VOICE_BYTES: usize = 25 * 1024 * 1024;

pub fn valid_voice_name(n: &str) -> bool {
    !n.is_empty() && n.len() <= 64 && n.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// Reconnaît le format par son en-tête (on ne fait pas confiance au Content-Type).
pub fn detect_audio_ext(b: &[u8]) -> Option<&'static str> {
    if b.len() > 12 && &b[..4] == b"RIFF" && &b[8..12] == b"WAVE" { return Some("wav") }
    if b.len() > 4 && &b[..4] == b"fLaC" { return Some("flac") }
    if b.len() > 4 && &b[..4] == b"OggS" { return Some("ogg") }
    if b.len() > 3 && (&b[..3] == b"ID3" || (b[0] == 0xFF && b[1] & 0xE0 == 0xE0)) { return Some("mp3") }
    None
}

const VOICE_EXTS: [&str; 4] = ["wav", "mp3", "flac", "ogg"];

async fn voices_list(State(a): State<App>) -> Json<Value> {
    let mut names: Vec<String> = std::fs::read_dir(a.cfg.voices_dir())
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let p = e.path();
            let ext = p.extension()?.to_str()?.to_lowercase();
            let stem = p.file_stem()?.to_str()?.to_string();
            (VOICE_EXTS.contains(&ext.as_str()) && valid_voice_name(&stem)).then_some(stem)
        })
        .collect();
    names.sort();
    names.dedup();
    Json(json!({ "voices": names }))
}

async fn voice_put(State(a): State<App>, h: HeaderMap, Path(name): Path<String>, body: Bytes) -> ApiResult {
    authorize(&a, &h)?;
    if !valid_voice_name(&name) {
        return Err(err(StatusCode::BAD_REQUEST, "nom de voix invalide (lettres, chiffres, - et _, 64 max)"));
    }
    let ext = detect_audio_ext(&body)
        .ok_or_else(|| err(StatusCode::UNSUPPORTED_MEDIA_TYPE, "format audio non reconnu (wav, mp3, flac ou ogg)"))?;
    let dir = a.cfg.voices_dir();
    std::fs::create_dir_all(&dir).map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e))?;
    for other in VOICE_EXTS { let _ = std::fs::remove_file(dir.join(format!("{name}.{other}"))); }
    std::fs::write(dir.join(format!("{name}.{ext}")), &body).map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e))?;
    Ok(Json(json!({ "voice": name, "bytes": body.len() })))
}

async fn voice_delete(State(a): State<App>, h: HeaderMap, Path(name): Path<String>) -> ApiResult {
    authorize(&a, &h)?;
    if !valid_voice_name(&name) { return Err(err(StatusCode::BAD_REQUEST, "nom de voix invalide")) }
    let dir = a.cfg.voices_dir();
    let removed = VOICE_EXTS.iter().filter(|e| std::fs::remove_file(dir.join(format!("{name}.{e}"))).is_ok()).count();
    if removed > 0 { Ok(Json(json!({ "deleted": name }))) } else { Err(err(StatusCode::NOT_FOUND, "voix introuvable")) }
}

#[derive(Deserialize)]
struct PreviewReq {
    text: String,
    voice: Option<String>,
    exaggeration: Option<f32>,
    cfg_weight: Option<f32>,
}

/// Écoute d'essai : la voix lit un texte avec les réglages du formulaire (pour trouver le bon ton).
async fn tts_preview(State(a): State<App>, h: HeaderMap, Json(r): Json<PreviewReq>) -> Result<axum::response::Response, (StatusCode, Json<Value>)> {
    authorize(&a, &h)?;
    let tts = a.tts.as_ref().ok_or_else(|| err(StatusCode::BAD_REQUEST, "synthèse vocale désactivée"))?;
    let text: String = r.text.chars().take(600).collect();
    if text.trim().is_empty() { return Err(err(StatusCode::BAD_REQUEST, "texte vide")) }
    let style = TtsStyle { exaggeration: r.exaggeration, cfg_weight: r.cfg_weight };
    let audio = tts
        .speak(&text, r.voice.as_deref().filter(|v| !v.is_empty()), &style)
        .await
        .map_err(|e| err(StatusCode::BAD_GATEWAY, format!("voix : {e}")))?;
    Ok(([(axum::http::header::CONTENT_TYPE, tts.mime())], audio).into_response())
}

#[derive(Deserialize)]
struct HfTokenBody {
    token: String,
}

/// Enregistre le jeton Hugging Face (téléchargements de modèles). Il n'est jamais renvoyé.
async fn hf_token_put(State(a): State<App>, h: HeaderMap, Json(b): Json<HfTokenBody>) -> ApiResult {
    authorize(&a, &h)?;
    a.models.set_hf_token(Some(&b.token)).map_err(|e| err(StatusCode::BAD_REQUEST, e))?;
    Ok(Json(json!({ "hf_token": true })))
}

async fn hf_token_delete(State(a): State<App>, h: HeaderMap) -> ApiResult {
    authorize(&a, &h)?;
    a.models.set_hf_token(None).map_err(|e| err(StatusCode::BAD_REQUEST, e))?;
    Ok(Json(json!({ "hf_token": a.models.has_hf_token() })))
}

async fn tts_runtime_install(State(a): State<App>, h: HeaderMap) -> ApiResult {
    authorize(&a, &h)?;
    a.engine.install_tts_runtime().await.map_err(|e| err(StatusCode::BAD_REQUEST, e))?;
    Ok(Json(json!({ "installing": true })))
}

#[cfg(test)]
mod tests {
    use super::{detect_audio_ext, origin_allowed, valid_voice_name};

    #[test]
    fn formats_audio_reconnus_par_en_tete() {
        assert_eq!(detect_audio_ext(b"RIFF\0\0\0\0WAVEfmt "), Some("wav"));
        assert_eq!(detect_audio_ext(b"fLaC\0\0\0\0"), Some("flac"));
        assert_eq!(detect_audio_ext(b"OggS\0\0\0\0"), Some("ogg"));
        assert_eq!(detect_audio_ext(b"ID3\x04\0\0\0\0"), Some("mp3"));
        assert_eq!(detect_audio_ext(&[0xFF, 0xFB, 0x90, 0x00, 0x00]), Some("mp3"));
        assert_eq!(detect_audio_ext(b"MZ\x90\0 un executable"), None);
        assert_eq!(detect_audio_ext(b""), None);
    }

    #[test]
    fn noms_de_voix() {
        assert!(valid_voice_name("voix_25-b"));
        assert!(!valid_voice_name("../etc/passwd"));
        assert!(!valid_voice_name("a b"));
        assert!(!valid_voice_name(""));
    }

    #[test]
    fn origines_locales_acceptees() {
        assert!(origin_allowed(None, Some("127.0.0.1:3001")));
        assert!(origin_allowed(Some("http://localhost:5173"), Some("127.0.0.1:3001")));
        assert!(origin_allowed(Some("http://127.0.0.1:3000"), Some("127.0.0.1:3001")));
        assert!(origin_allowed(Some("http://[::1]:3000"), Some("[::1]:3001")));
        assert!(origin_allowed(Some("http://mon-pc.local:3000"), Some("mon-pc.local:3001")));
    }

    #[test]
    fn sites_externes_refuses() {
        assert!(!origin_allowed(Some("https://evil.example"), Some("127.0.0.1:3001")));
        assert!(!origin_allowed(Some("http://localhost.evil.example"), Some("127.0.0.1:3001")));
    }
}
