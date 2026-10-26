//! Session live (WebSocket) : texte ou voix en entrée ; texte, sources, réponses rapides
//! et audio synthétisé en sortie, le tout en streaming.
//!
//! Protocole client → serveur (JSON texte) :
//!   {"type":"start","persona":"id","tts":true,"user_name":"Marc"}  → ouvre (ou reprend) la conversation d'un profil d'assistant ;
//!                             `"resume":false` en commence une nouvelle
//!   {"type":"user","text":"..."}
//!   {"type":"interrupt"}
//!   {"type":"settings","tts":false}
//!   {"type":"reset","persona":"id"}  → supprime la conversation sauvegardée du profil (réponse « reset_done »)
//!   {"type":"status"}         → l'état des serveurs (réponse « status »)
//!   {"type":"ticket_draft"}   → prépare le ticket de la conversation en cours : titre, résumé, sources consultées
//!   {"type":"voice_test","persona":"id"}  → fait dire une phrase d'essai avec la voix du profil
//!   {"type":"speak","text":"...","persona":"id","req":3}  → lit un texte à voix haute (réécouter un message) ; ne coupe que la
//!                             voix en cours, jamais la réponse qui s'écrit
//! et en binaire : un WAV 16 kHz mono (parole de l'utilisateur).
//!
//! Serveur → client (JSON) : hello, status, persona, restored, reset_done, transcript, gen_start (une génération commence : son
//! identifiant arrive AVANT ses jetons), token, source (un extrait de document cité par la réponse en cours : lien cliquable),
//! media (l'image affichée à côté de la conversation), media_miss, replies (avec `ticket` : le bouton qui ouvre un ticket),
//! ticket_draft (le ticket prérempli demandé par `ticket_draft`),
//! audio (toujours suivi, dans le même envoi, de sa trame binaire), speak_start (identifiant d'une lecture à la demande, avec
//! le `req` de la demande), voice_end (la voix a envoyé toutes ses phrases : {"gen":id}), tts_error, done, interrupted, error.

use crate::conversation::{is_visible, key_for, Citation, Saved};
use crate::director::{chunk_for_tts, for_speech, Directive, Piece, SentenceSplitter, StreamParser, MAX_CHUNK};
use crate::kb::{KnowledgeBase, Source};
use crate::llm::ChatMsg;
use crate::persona::Persona;
use crate::prompt::{estimate_tokens, opening_instruction, system_prompt, SourcesCtx, UserInfo, OPENING_PREFIX};
use crate::state::AppState;
use crate::voice::{Tts, TtsStyle};
use axum::extract::ws::{Message, WebSocket};
use futures_util::{stream, Stream, StreamExt};
use serde_json::{json, Value};
use std::collections::HashSet;
use std::pin::Pin;
use std::sync::Arc;
use tokio::sync::{mpsc, Mutex};
use tokio::task::{AbortHandle, JoinHandle};

/// Un envoi = une suite de trames écrites d'une traite (une trame audio ne peut pas être séparée de ses métadonnées).
type Out = mpsc::UnboundedSender<Vec<Message>>;
type TokenStream = Pin<Box<dyn Stream<Item = anyhow::Result<String>> + Send>>;
/// Phrases à lire, dans l'ordre.
type VoiceTx = mpsc::UnboundedSender<String>;
/// Identifiants de lecture à la demande (« réécouter ») : au-dessus de ceux des générations, jamais confondus.
const SPEAK_BASE: u64 = 1 << 32;

struct Session {
    persona: Option<Persona>,
    history: Vec<ChatMsg>,
    summary: String,
    shown: HashSet<i64>,
    tts_on: bool,
    /// Qui parle à l'assistant (prénom donné au démarrage).
    user: UserInfo,
    /// Clé de sauvegarde de la conversation (le profil d'assistant).
    conv_key: Option<String>,
    gen_id: u64,
    handles: Vec<AbortHandle>,
    /// Voix en cours (synthèse + envoi) : s'annule seule, sans arrêter la génération du texte.
    voice: Option<AbortHandle>,
    speak_n: u64,
    /// Nombre de tours de parole depuis le début de la conversation.
    turn: u32,
    /// Numéros des sources proposées à l'IA à ce tour (extraits et images) : `[[source: #id]]` et `[[show: #id]]` ne peuvent viser
    /// que celles-là.
    offered: Vec<i64>,
    /// La source montrée en dernier (et à quel tour) : l'IA sait ce que l'utilisateur a sous les yeux.
    screen: Option<(u32, Source)>,
    /// Extraits cités depuis le début de la conversation (sans doublon) : les « sources consultées » du ticket.
    cited: Vec<i64>,
    /// Extraits cités par chaque message de l'assistant : (rang du message dans l'historique, numéros).
    citations: Vec<(usize, Vec<i64>)>,
    /// Extraits cités par la réponse en cours d'écriture.
    turn_cited: Vec<i64>,
    /// La réponse en cours a déjà proposé des boutons de réponse.
    replies_sent: bool,
}

impl Session {
    fn new(tts_on: bool) -> Self {
        Self {
            persona: None,
            history: vec![],
            summary: String::new(),
            shown: HashSet::new(),
            tts_on,
            user: UserInfo::default(),
            conv_key: None,
            gen_id: 0,
            handles: vec![],
            voice: None,
            speak_n: 0,
            turn: 0,
            offered: vec![],
            screen: None,
            cited: vec![],
            citations: vec![],
            turn_cited: vec![],
            replies_sent: false,
        }
    }

    /// Repart de zéro : plus d'historique, de résumé, de sources montrées ni citées.
    fn clear_conversation(&mut self) {
        self.history.clear();
        self.summary.clear();
        self.shown.clear();
        self.turn = 0;
        self.offered.clear();
        self.screen = None;
        self.cited.clear();
        self.citations.clear();
        self.turn_cited.clear();
    }
}

/// Nombre de sources proposées à l'IA à chaque tour.
pub const MAX_PICKS: usize = 4;
/// Place que prennent dans le prompt système les sources proposées et les consignes qui les accompagnent (estimation, par excès :
/// un extrait de 1 200 caractères pèse environ 370 jetons).
const SOURCES_TOKENS: usize = MAX_PICKS * 380 + 300;

/// Consigne du moteur (ouverture de la conversation) plutôt que parole de l'utilisateur.
fn is_instruction(m: &ChatMsg) -> bool {
    m.role == "user" && m.content.starts_with(OPENING_PREFIX)
}

/// Textes qui décrivent la question en cours, avec leur poids dans la recherche de sources : les derniers messages de l'utilisateur
/// (le plus récent compte le plus), puis le résumé de ce qui précède. Les réponses de l'assistant n'en font pas partie : son message
/// d'accueil et ses propres réponses (déjà tirées des extraits précédents) ramèneraient toujours les mêmes extraits, au lieu de suivre
/// le sujet de l'utilisateur.
pub(crate) fn scene_parts(history: &[ChatMsg], summary: &str) -> Vec<(String, f64)> {
    let said: Vec<&ChatMsg> = history.iter().filter(|m| m.role == "user" && !is_instruction(m)).collect();
    let mut parts: Vec<(String, f64)> = said.iter().rev().zip([1.0, 0.7, 0.5]).map(|(m, w)| (m.content.clone(), w)).collect();
    parts.push((summary.to_string(), 0.3));
    parts.retain(|(t, _)| !t.trim().is_empty());
    parts
}

/// La source n°`id`, si l'IA a le droit de la citer ou de l'afficher : elle figurait dans la liste proposée à ce tour, et si c'est une
/// image, elle n'a pas déjà été montrée. Un numéro inventé, ou proposé à un autre tour, ne donne rien.
fn allowed_pick(kb: &KnowledgeBase, s: &Session, id: i64) -> Option<Source> {
    if !s.offered.contains(&id) {
        return None;
    }
    let source = kb.get(id)?;
    if !source.is_passage() && s.shown.contains(&id) {
        return None;
    }
    Some(source)
}

/// Ce que le navigateur reçoit d'un extrait cité : de quoi afficher un lien (titre, section) et un aperçu au survol.
fn source_json(p: &Source) -> Value {
    json!({
        "id": p.id, "title": p.title, "section": p.section, "page": p.page, "doc_id": p.doc_id, "link": p.link,
        "excerpt": crate::prompt::short(&crate::secrets::mask(&p.description), 180),
    })
}

/// L'IA dit-elle qu'elle ne sait pas ? (la formule demandée par les règles : « Je ne sais pas : je n'ai pas trouvé cela… »)
fn says_unknown(text: &str) -> bool {
    let t = crate::terms::fold(text);
    ["je ne sais pas", "je n ai pas trouve", "je ne trouve pas", "aucune source ne"].iter().any(|m| t.contains(m))
}

/// Les boutons proposés quand l'IA ne sait pas et n'a pas proposé les siens.
const UNKNOWN_REPLIES: [&str; 2] = ["Ouvrir un ticket", "Reformuler"];

/// Parmi des boutons de réponse, celui qui ouvre un ticket (le modèle l'écrit « Ouvrir un ticket », « Créer un ticket »…).
fn ticket_button(items: &[String]) -> Option<&String> {
    items.iter().find(|i| crate::terms::fold(i).split_whitespace().any(|w| w == "ticket" || w == "tickets"))
}

fn replies_json(items: &[String]) -> Value {
    json!({"type":"replies","items":items,"ticket":ticket_button(items)})
}

/// État des serveurs, tel que l'interface l'affiche (et le rafraîchit toutes les quelques secondes).
async fn status_fields(app: &Arc<AppState>) -> Value {
    let (llm, mic, voice) = tokio::join!(app.llm.healthy(), app.mic_status(), app.voice_status());
    json!({"llm": llm, "stt": mic["ready"], "mic": mic, "tts": voice["ready"], "voice": voice})
}

fn send(out: &Out, v: Value) {
    let _ = out.send(vec![Message::Text(v.to_string().into())]);
}

/// Envoie un morceau d'audio : métadonnées puis trame binaire, sans que rien ne s'intercale.
fn send_audio(out: &Out, meta: Value, bytes: Vec<u8>) {
    let _ = out.send(vec![Message::Text(meta.to_string().into()), Message::Binary(bytes.into())]);
}

pub async fn handle_socket(socket: WebSocket, app: Arc<AppState>) {
    let (mut sink, mut source) = socket.split();
    let (out, mut rx) = mpsc::unbounded_channel::<Vec<Message>>();

    let writer = tokio::spawn(async move {
        use futures_util::SinkExt;
        while let Some(batch) = rx.recv().await {
            for m in batch {
                if sink.send(m).await.is_err() {
                    return;
                }
            }
        }
    });

    let session = Arc::new(Mutex::new(Session::new(app.tts.is_some())));

    let mut hello = status_fields(&app).await;
    hello["type"] = json!("hello");
    hello["plan"] = json!(app.plan());
    send(&out, hello);

    while let Some(Ok(msg)) = source.next().await {
        match msg {
            Message::Text(t) => {
                let Ok(v) = serde_json::from_str::<Value>(t.as_str()) else { continue };
                match v["type"].as_str().unwrap_or("") {
                    "start" => on_start(&app, &session, &out, &v).await,
                    "user" => {
                        let text = v["text"].as_str().unwrap_or("").trim().to_string();
                        if !text.is_empty() {
                            start_turn(&app, &session, &out, Some(text)).await;
                        }
                    }
                    "interrupt" => {
                        interrupt(&session).await;
                        send(&out, json!({"type":"interrupted"}));
                    }
                    "settings" => {
                        if let Some(b) = v["tts"].as_bool() {
                            session.lock().await.tts_on = b && app.tts.is_some();
                        }
                    }
                    // Les sondes sont lancées à part : un serveur lent ne doit pas geler la lecture des messages.
                    "status" => {
                        let (app, out) = (app.clone(), out.clone());
                        tokio::spawn(async move {
                            let mut st = status_fields(&app).await;
                            st["type"] = json!("status");
                            send(&out, st);
                        });
                    }
                    "reset" => {
                        reset_conversation(&app, &session, v["persona"].as_str().unwrap_or("")).await;
                        send(&out, json!({"type":"reset_done"}));
                    }
                    "ticket_draft" => ticket_draft(&app, &session, &out).await,
                    "voice_test" => voice_test(&app, &session, &out, v["persona"].as_str()).await,
                    "speak" => {
                        let text = v["text"].as_str().unwrap_or("").to_string();
                        // `req` (facultatif) : numéro de la demande, renvoyé par `speak_start` pour que le navigateur reconnaisse SA lecture
                        speak_text(&app, &session, &out, text, v["persona"].as_str(), v["req"].as_u64()).await;
                    }
                    _ => {}
                }
            }
            Message::Binary(wav) => {
                let Some(stt) = app.stt.clone() else {
                    send(&out, json!({"type":"error","message":"reconnaissance vocale désactivée"}));
                    continue;
                };
                // Barge-in : l'utilisateur parle, on coupe l'IA.
                interrupt(&session).await;
                match stt.transcribe(wav.to_vec()).await {
                    Ok(text) if !text.trim().is_empty() => {
                        send(&out, json!({"type":"transcript","text":text}));
                        start_turn(&app, &session, &out, Some(text)).await;
                    }
                    Ok(_) => send(&out, json!({"type":"transcript","text":""})),
                    Err(e) => {
                        tracing::warn!("micro : {e}");
                        let mic = app.mic_status().await;
                        let msg = match mic["problem"].as_str() {
                            Some(p) if mic["ready"] == json!(false) => p.to_string(),
                            _ => format!("Micro : {e}"),
                        };
                        send(&out, json!({"type":"error","message":msg}));
                    }
                }
            }
            Message::Close(_) => break,
            _ => {}
        }
    }
    interrupt(&session).await;
    writer.abort();
}

async fn interrupt(session: &Arc<Mutex<Session>>) {
    let mut s = session.lock().await;
    s.gen_id += 1;
    for h in s.handles.drain(..) {
        h.abort();
    }
    if let Some(h) = s.voice.take() {
        h.abort();
    }
}

async fn on_start(app: &Arc<AppState>, session: &Arc<Mutex<Session>>, out: &Out, v: &Value) {
    let id = v["persona"].as_str().unwrap_or("");
    let Some(persona) = app.personas.get(id) else {
        send(out, json!({"type":"error","message":"profil d'assistant introuvable ou invalide"}));
        return;
    };
    interrupt(session).await;
    {
        let mut s = session.lock().await;
        s.clear_conversation();
        if let Some(b) = v["tts"].as_bool() {
            s.tts_on = b && app.tts.is_some();
        }
        s.user = UserInfo::new(v["user_name"].as_str().unwrap_or(""));
        s.persona = Some(persona.clone());
        s.conv_key = key_for(&persona.id);
    }
    send(out, json!({"type":"persona","persona":persona}));
    let mut st = status_fields(app).await;
    st["type"] = json!("status");
    send(out, st);
    // Reprise automatique de la conversation sauvegardée, sauf demande contraire (resume: false = nouvelle conversation).
    if v["resume"].as_bool() == Some(false) {
        reset_conversation(app, session, &persona.id).await;
    } else if restore(app, session, out).await {
        return;
    }
    if persona.first_message.trim().is_empty() {
        // Pas de message d'accueil écrit à l'avance : l'assistant ouvre la conversation lui-même.
        start_turn(app, session, out, None).await;
    } else {
        let text = persona.first_message.clone();
        launch(app, session, out, Box::pin(stream::iter(vec![Ok(text)]))).await;
    }
}

/// Recharge la conversation sauvegardée dans la session et l'envoie au navigateur. `false` s'il n'y en a pas.
async fn restore(app: &Arc<AppState>, session: &Arc<Mutex<Session>>, out: &Out) -> bool {
    let mut s = session.lock().await;
    let Some(saved) = s.conv_key.as_deref().and_then(|k| app.conversations.load(k)).filter(|c| !c.history.is_empty()) else {
        return false;
    };
    s.history = saved.history.clone();
    s.summary = saved.summary.clone();
    s.shown = saved.shown.iter().copied().collect();
    s.citations = saved.citations.iter().filter(|c| c.at < saved.history.len()).map(|c| (c.at, c.ids.clone())).collect();
    s.cited = vec![];
    for id in s.citations.iter().flat_map(|(_, ids)| ids.clone()).collect::<Vec<_>>() {
        if !s.cited.contains(&id) {
            s.cited.push(id);
        }
    }
    // Le prénom donné aujourd'hui l'emporte ; sans prénom, on retrouve celui de la conversation.
    if s.user.name.is_empty() {
        s.user = UserInfo::new(&saved.user_name);
    }
    let messages: Vec<Value> = s
        .history
        .iter()
        .enumerate()
        .filter(|(_, m)| is_visible(m))
        .map(|(i, m)| {
            let mut v = json!({"role": m.role, "text": m.content});
            // les extraits que ce message citait (ceux qui ont été retirés de la base depuis n'y sont plus)
            let sources: Vec<Value> = s.citations.iter().filter(|(at, _)| *at == i).flat_map(|(_, ids)| ids).filter_map(|id| app.kb.get(*id)).filter(Source::is_passage).map(|p| source_json(&p)).collect();
            if !sources.is_empty() {
                v["sources"] = json!(sources);
            }
            v
        })
        .collect();
    send(out, json!({"type":"restored","messages":messages,"user_name":s.user.name,"updated":saved.updated}));
    true
}

/// Sauvegarde la conversation (appelé à la fin de chaque réponse). Une erreur d'écriture est journalisée, jamais bloquante.
fn save_conversation(app: &AppState, s: &Session) {
    let (Some(key), Some(p)) = (s.conv_key.as_deref(), s.persona.as_ref()) else { return };
    if s.history.is_empty() {
        return;
    }
    let saved = Saved {
        persona: p.id.clone(),
        history: s.history.clone(),
        summary: s.summary.clone(),
        shown: s.shown.iter().copied().collect(),
        user_name: s.user.name.clone(),
        updated: std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0),
        citations: s.citations.iter().map(|(at, ids)| Citation { at: *at, ids: ids.clone() }).collect(),
    };
    if let Err(e) = app.conversations.save(key, &saved) {
        tracing::warn!("sauvegarde de la conversation impossible : {e}");
    }
}

/// Nouvelle conversation : supprime la sauvegarde et vide la mémoire de la session si elle la concerne.
async fn reset_conversation(app: &Arc<AppState>, session: &Arc<Mutex<Session>>, persona: &str) {
    let Some(key) = key_for(persona) else { return };
    app.conversations.delete(&key);
    interrupt(session).await;
    let mut s = session.lock().await;
    if s.conv_key.as_deref() == Some(key.as_str()) {
        s.clear_conversation();
    }
}

/// Ajoute le message utilisateur (s'il y en a un), compacte l'historique, puis lance la génération.
async fn start_turn(app: &Arc<AppState>, session: &Arc<Mutex<Session>>, out: &Out, user: Option<String>) {
    interrupt(session).await;
    {
        let mut s = session.lock().await;
        if s.persona.is_none() {
            send(out, json!({"type":"error","message":"aucun profil d'assistant sélectionné"}));
            return;
        }
        s.turn += 1;
        match user {
            Some(u) => s.history.push(ChatMsg::new("user", u)),
            None => {
                let instruction = opening_instruction(&s.user);
                s.history.push(ChatMsg::new("user", instruction));
            }
        }
        save_conversation(app, &s); // le message de l'utilisateur n'est pas perdu si l'app se ferme avant la réponse
    }
    compact(app, session).await;

    let (messages, temp, sampling) = {
        let mut s = session.lock().await;
        let p = s.persona.clone().unwrap();
        // Les sources (extraits de documents, images) qui se rapprochent de la conversation sont proposées à l'IA à chaque tour : elle
        // répond d'après elles, cite celles qu'elle utilise et décide d'afficher une image ou non.
        let picks = app.kb.relevant(&scene_parts(&s.history, &s.summary), &s.shown, MAX_PICKS);
        s.offered = picks.iter().map(|m| m.id).collect();
        let on_screen = s.screen.as_ref().filter(|(t, _)| s.turn.saturating_sub(*t) <= 1).map(|(_, m)| m.clone());
        let sources = SourcesCtx { picks: &picks, on_screen: on_screen.as_ref() };
        let sys = system_prompt(&p, &s.user, &sources, &s.summary);
        let mut m = vec![ChatMsg::new("system", sys)];
        m.extend(s.history.iter().cloned());
        (m, p.temperature.unwrap_or(app.cfg.llm.temperature), p.sampling.clone())
    };
    match app.llm.stream(&messages, temp, &sampling).await {
        Ok(st) => launch(app, session, out, Box::pin(st)).await,
        Err(e) => send(out, json!({"type":"error","message":format!("LLM : {e}")})),
    }
}

/// Message affiché pour un échec de synthèse : si le serveur de voix ne répond pas du tout, la cause probable
/// (moteur non installé, modèle non chargé…) est plus utile que l'erreur réseau brute.
async fn tts_failure_message(app: &AppState, e: &anyhow::Error) -> String {
    let status = app.voice_status().await;
    match status["problem"].as_str() {
        Some(p) if status["ready"] == json!(false) => p.to_string(),
        _ => e.to_string(),
    }
}

/// Essai de la voix : lit une phrase avec la voix et les réglages du profil, par le même chemin que les réponses.
/// Une erreur (voix non chargée, échantillon illisible…) arrive à l'écran au lieu de rester dans les journaux.
async fn voice_test(app: &Arc<AppState>, session: &Arc<Mutex<Session>>, out: &Out, persona_id: Option<&str>) {
    let Some(tts) = app.tts.clone() else {
        send(out, json!({"type":"tts_error","message":"La voix est désactivée dans la configuration."}));
        return;
    };
    // Profil demandé (écran de départ), sinon celui de la conversation en cours.
    let chosen = persona_id.and_then(|id| app.personas.get(id));
    let (voice, style) = {
        let s = session.lock().await;
        voice_of(chosen.as_ref().or(s.persona.as_ref()))
    };
    let out = out.clone();
    let app = app.clone();
    tokio::spawn(async move {
        match tts.speak("Bonjour, c'est ma voix. Vous m'entendez bien ?", voice.as_deref(), &style).await {
            Ok(bytes) => send_audio(&out, json!({"type":"audio","gen":0,"seq":0,"mime":tts.mime(),"test":true}), bytes),
            Err(e) => send(&out, json!({"type":"tts_error","message":tts_failure_message(&app, &e).await})),
        }
    });
}

/// Voix de référence et réglages d'expressivité d'un profil.
fn voice_of(p: Option<&Persona>) -> (Option<String>, TtsStyle) {
    (
        p.and_then(|p| p.voice.clone()),
        TtsStyle { exaggeration: p.and_then(|p| p.tts_exaggeration), cfg_weight: p.and_then(|p| p.tts_cfg_weight) },
    )
}

/// Lit un texte à voix haute (« réécouter » un message déjà écrit) : même chemin que les réponses de l'IA. Ne coupe que
/// la voix en cours : une réponse encore en train de s'écrire continue (et se retrouve dans l'historique).
async fn speak_text(app: &Arc<AppState>, session: &Arc<Mutex<Session>>, out: &Out, text: String, persona_id: Option<&str>, req: Option<u64>) {
    if app.tts.is_none() {
        send(out, json!({"type":"tts_error","message":"La voix est désactivée dans la configuration."}));
        return;
    }
    // Nettoyage d'abord : un message qui n'a rien à lire ne coupe pas la voix en cours.
    let clean = for_speech(&text.chars().take(4000).collect::<String>());
    if !clean.chars().any(|c| c.is_alphanumeric()) {
        send(out, json!({"type":"tts_error","message":"Ce message ne contient rien à lire à voix haute."}));
        return;
    }
    let chosen = persona_id.and_then(|id| app.personas.get(id));
    let (audio_id, voice, style) = {
        let mut s = session.lock().await;
        if let Some(h) = s.voice.take() {
            h.abort(); // la voix précédente (réponse en cours ou autre lecture) se tait
        }
        s.speak_n += 1;
        // Le profil demandé d'abord (la session en direct n'a pas forcément été rouverte avec lui).
        let (voice, style) = voice_of(chosen.as_ref().or(s.persona.as_ref()));
        (SPEAK_BASE + s.speak_n, voice, style)
    };
    send(out, json!({"type":"speak_start","gen":audio_id,"req":req}));
    let Some((tx, worker)) = spawn_voice(app, out, audio_id, voice, style) else { return };
    let mut speech = Speech::new(tx);
    speech.feed_clean(&clean);
    speech.finish();
    drop(speech);
    session.lock().await.voice = Some(worker.abort_handle());
}

/// Un seul travailleur par lecture : il synthétise puis envoie chaque phrase À SON TOUR. (Lancer une tâche par phrase
/// les ferait passer dans le désordre devant le verrou du serveur de voix, et un nouvel essai se remettrait en queue.)
/// Annuler la tâche abandonne la requête en cours et les phrases en attente.
fn spawn_voice(app: &Arc<AppState>, out: &Out, audio_id: u64, voice: Option<String>, style: TtsStyle) -> Option<(VoiceTx, JoinHandle<()>)> {
    let tts = app.tts.clone()?;
    let (tx, mut rx) = mpsc::unbounded_channel::<String>();
    let (app, out) = (app.clone(), out.clone());
    let handle = tokio::spawn(async move {
        let (mut seq, mut warned) = (0u32, false);
        while let Some(text) = rx.recv().await {
            match synthesize(&tts, &text, voice.as_deref(), &style, seq).await {
                Ok(bytes) => send_audio(&out, json!({"type":"audio","gen":audio_id,"seq":seq,"mime":tts.mime()}), bytes),
                Err(e) => {
                    // Une seule alerte par lecture : si la voix est en panne, chaque phrase échouerait pareil.
                    if !std::mem::replace(&mut warned, true) {
                        let message = tts_failure_message(&app, &e).await;
                        send(&out, json!({"type":"tts_error","gen":audio_id,"message":message}));
                    }
                }
            }
            seq += 1;
        }
        // Tout est envoyé (une lecture interrompue n'arrive pas jusqu'ici) : le navigateur sait qu'aucune phrase ne suivra.
        send(&out, json!({"type":"voice_end","gen":audio_id}));
    });
    Some((tx, handle))
}

/// Synthétise une phrase ; un échec passager (serveur occupé, mémoire GPU saturée un instant) est retenté une fois.
async fn synthesize(tts: &Tts, text: &str, voice: Option<&str>, style: &TtsStyle, seq: u32) -> anyhow::Result<Vec<u8>> {
    let mut last = None;
    for attempt in 1..=2 {
        let started = std::time::Instant::now();
        match tts.speak(text, voice, style).await {
            Ok(bytes) => {
                tracing::info!(
                    "voix : phrase {seq} ({} car.) → {} Ko en {} ms",
                    text.chars().count(), bytes.len() / 1024, started.elapsed().as_millis()
                );
                return Ok(bytes);
            }
            Err(e) => {
                tracing::warn!("voix : phrase {seq}, essai {attempt}/2 : {e}");
                last = Some(e);
                if started.elapsed() > std::time::Duration::from_secs(30) {
                    break; // la requête a déjà été très longue : inutile de recommencer
                }
                if attempt == 1 {
                    tokio::time::sleep(std::time::Duration::from_millis(600)).await;
                }
            }
        }
    }
    Err(last.unwrap_or_else(|| anyhow::anyhow!("synthèse impossible")))
}

/// Texte de l'IA → voix : retire la mise en forme, découpe en phrases (puis en morceaux de taille raisonnable) et les confie
/// au travailleur de voix sans attendre la fin de la réponse.
struct Speech {
    tx: VoiceTx,
    splitter: SentenceSplitter,
}

impl Speech {
    fn new(tx: VoiceTx) -> Self {
        Self { tx, splitter: SentenceSplitter::default() }
    }

    /// Texte brut du LLM (mise en forme et emojis compris) : nettoyé AVANT le découpage en phrases.
    fn feed(&mut self, raw: &str) {
        self.feed_clean(&for_speech(raw));
    }

    /// Texte déjà débarrassé de sa mise en forme.
    fn feed_clean(&mut self, clean: &str) {
        for sentence in self.splitter.feed(clean) {
            self.say(&sentence);
        }
    }

    /// Envoie ce qui est en attente (phrase en cours de découpage).
    fn finish(&mut self) {
        if let Some(rest) = self.splitter.finish() {
            self.say(&rest);
        }
    }

    fn say(&mut self, sentence: &str) {
        let text = sentence.split_whitespace().collect::<Vec<_>>().join(" ");
        for chunk in chunk_for_tts(&text, MAX_CHUNK) {
            let _ = self.tx.send(chunk); // lecture annulée : plus personne n'écoute, rien à faire
        }
    }
}

/// Consigne du résumé de conversation d'assistance : elle sert à alléger l'historique (`compact`) et à préremplir un ticket.
const SUMMARY_RULES: &str = "Tu résumes des conversations d'assistance informatique de façon factuelle et compacte (≤ 200 mots), en gardant : \
    le problème de l'utilisateur, le matériel et les logiciels concernés, les messages d'erreur exacts, les étapes déjà essayées et leur \
    résultat, ce qui reste à faire. N'ajoute rien qui n'ait pas été dit. Ne conserve aucun mot de passe ni code secret.";

/// Résume des échanges en reprenant le résumé précédent. `None` si le modèle ne répond pas ou ne dit rien.
async fn summarize(app: &Arc<AppState>, previous: &str, messages: &[ChatMsg]) -> Option<String> {
    let transcript = messages.iter().filter(|m| !is_instruction(m)).map(|m| format!("{}: {}", m.role, m.content)).collect::<Vec<_>>().join("\n");
    let ask = vec![
        ChatMsg::new("system", SUMMARY_RULES),
        ChatMsg::new("user", format!("Résumé précédent :\n{previous}\n\nNouveaux échanges :\n{transcript}\n\nNouveau résumé :")),
    ];
    app.llm.complete(&ask, 0.3, 400).await.ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

/// Résume les plus anciens tours quand le contexte approche de sa limite.
async fn compact(app: &Arc<AppState>, session: &Arc<Mutex<Session>>) {
    let to_summarize = {
        let s = session.lock().await;
        let used: usize = s.history.iter().map(|m| estimate_tokens(&m.content)).sum::<usize>()
            + estimate_tokens(&s.summary) + 1500 // marge : prompt système
            + SOURCES_TOKENS; // ce que les sources proposées y ajoutent
        if used as f64 > app.plan().ctx_tokens as f64 * 0.75 && s.history.len() > 6 {
            let n = s.history.len() / 2;
            Some((s.history[..n].to_vec(), s.summary.clone(), n))
        } else {
            None
        }
    };
    let Some((old, prev, n)) = to_summarize else { return };
    let summary = summarize(app, &prev, &old).await.unwrap_or(prev);
    let mut s = session.lock().await;
    let n = n.min(s.history.len());
    s.history.drain(..n);
    s.citations.retain_mut(|(at, _)| match at.checked_sub(n) {
        Some(shifted) => {
            *at = shifted;
            true
        }
        None => false,
    });
    s.summary = summary;
}

/// Prépare le ticket de la conversation en cours : le résumé de tout ce qui s'est dit (même résumeur que `compact`), les sources
/// consultées, le prénom. L'utilisateur relit, corrige et valide ; rien n'est enregistré ici.
async fn ticket_draft(app: &Arc<AppState>, session: &Arc<Mutex<Session>>, out: &Out) {
    let (history, previous, user, cited) = {
        let s = session.lock().await;
        (s.history.iter().filter(|m| is_visible(m)).cloned().collect::<Vec<_>>(), s.summary.clone(), s.user.name.clone(), s.cited.clone())
    };
    if !history.iter().any(|m| m.role == "user") {
        send(out, json!({"type":"error","message":"Il n'y a encore rien à mettre dans un ticket : décrivez d'abord votre problème."}));
        return;
    }
    let summary = summarize(app, &previous, &history).await;
    let sources: Vec<crate::tickets::ConsultedSource> = cited.iter().filter_map(|id| app.kb.get(*id)).filter(Source::is_passage).map(|p| crate::tickets::ConsultedSource::of(&p)).collect();
    let draft = crate::tickets::compose(summary.as_deref(), &history, &user, &sources);
    let mut msg = json!(draft);
    msg["type"] = json!("ticket_draft");
    msg["user_name"] = json!(user);
    send(out, msg);
}

async fn launch(app: &Arc<AppState>, session: &Arc<Mutex<Session>>, out: &Out, tokens: TokenStream) {
    let (gen_id, tts_on, voice, style) = {
        let mut s = session.lock().await;
        s.gen_id += 1;
        s.turn_cited.clear();
        s.replies_sent = false;
        let (voice, style) = voice_of(s.persona.as_ref());
        (s.gen_id, s.tts_on, voice, style)
    };
    // Le navigateur apprend quelle génération est la bonne AVANT ses jetons : ceux d'une génération interrompue,
    // encore en route, ne peuvent plus ouvrir une bulle ni rouvrir le son.
    send(out, json!({"type":"gen_start","gen":gen_id}));
    let (mut worker, mut speech) = (None, None);
    if tts_on {
        if let Some((tx, handle)) = spawn_voice(app, out, gen_id, voice, style) {
            worker = Some(handle);
            speech = Some(Speech::new(tx));
        }
    }
    let (app2, session2, out2) = (app.clone(), session.clone(), out.clone());
    let gen_task = tokio::spawn(async move {
        run_generation(app2, session2, out2, tokens, gen_id, speech).await;
    });
    let mut s = session.lock().await;
    s.handles.push(gen_task.abort_handle());
    if let Some(h) = &worker {
        s.voice = Some(h.abort_handle());
    }
    // Une interruption a pu passer entre-temps (avant que ces tâches soient enregistrées) : on les arrête aussitôt.
    if s.gen_id != gen_id {
        gen_task.abort();
        if let Some(h) = worker {
            h.abort();
        }
    }
}

async fn run_generation(
    app: Arc<AppState>,
    session: Arc<Mutex<Session>>,
    out: Out,
    mut tokens: TokenStream,
    gen_id: u64,
    mut speech: Option<Speech>,
) {
    let mut parser = StreamParser::default();
    let mut raw = String::new();

    let mut handle_pieces = |pieces: Vec<Piece>, raw: &mut String| {
        let mut directives = vec![];
        for p in pieces {
            match p {
                Piece::Text(t) => {
                    raw.push_str(&t);
                    send(&out, json!({"type":"token","gen":gen_id,"text":t}));
                    if let Some(sp) = speech.as_mut() {
                        sp.feed(&t);
                    }
                }
                Piece::Directive(d) => directives.push(d),
            }
        }
        directives
    };

    let mut stream_error: Option<String> = None;
    while let Some(tok) = tokens.next().await {
        match tok {
            Ok(t) => {
                let pieces = parser.feed(&t);
                let ds = handle_pieces(pieces, &mut raw);
                apply_directives(&app, &session, &out, gen_id, ds).await;
            }
            Err(e) => { stream_error = Some(e.to_string()); break; }
        }
    }
    let pieces = parser.finish();
    let ds = handle_pieces(pieces, &mut raw);
    apply_directives(&app, &session, &out, gen_id, ds).await;
    if let Some(sp) = speech.as_mut() {
        sp.finish();
    }
    drop(speech); // ferme la file : l'envoi audio s'arrête quand tout ce qui est en attente est parti

    if let Some(e) = stream_error {
        send(&out, json!({"type":"error","message":format!("LLM : {e}")}));
    }
    let propose_ticket = {
        let mut s = session.lock().await;
        if s.gen_id == gen_id && !raw.trim().is_empty() {
            s.history.push(ChatMsg::new("assistant", raw.trim().to_string()));
            if !s.turn_cited.is_empty() {
                let (at, ids) = (s.history.len() - 1, std::mem::take(&mut s.turn_cited));
                s.citations.push((at, ids));
            }
            save_conversation(&app, &s);
        }
        // Filet de sécurité : une IA qui dit ne pas savoir propose toujours le ticket, même si elle a oublié les boutons.
        s.gen_id == gen_id && !s.replies_sent && says_unknown(&raw)
    };
    if propose_ticket {
        let items: Vec<String> = UNKNOWN_REPLIES.iter().map(|b| b.to_string()).collect();
        send(&out, replies_json(&items));
    }
    send(&out, json!({"type":"done","gen":gen_id}));
}

/// Applique les directives de l'IA.
async fn apply_directives(app: &Arc<AppState>, session: &Arc<Mutex<Session>>, out: &Out, gen_id: u64, ds: Vec<Directive>) {
    for d in ds {
        match d {
            Directive::ShowId { id } => pick(app, session, out, gen_id, &[id]).await,
            Directive::Source { ids } => pick(app, session, out, gen_id, &ids).await,
            Directive::Replies { items } => {
                session.lock().await.replies_sent = true;
                send(out, replies_json(&items));
            }
        }
    }
}

/// Cite des extraits ou affiche une image, à condition qu'ils aient été proposés à ce tour. Un extrait déjà cité par cette réponse ne
/// l'est pas deux fois ; `[[show]]` sur un extrait le cite, `[[source]]` sur une image l'affiche : seul compte ce que la source est.
async fn pick(app: &Arc<AppState>, session: &Arc<Mutex<Session>>, out: &Out, gen_id: u64, ids: &[i64]) {
    for &id in ids {
        let mut s = session.lock().await;
        match allowed_pick(&app.kb, &s, id) {
            Some(p) if p.is_passage() => {
                if !s.turn_cited.contains(&id) {
                    s.turn_cited.push(id);
                    if !s.cited.contains(&id) {
                        s.cited.push(id);
                    }
                    send(out, json!({"type":"source","gen":gen_id,"source":source_json(&p)}));
                }
            }
            Some(m) => {
                s.shown.insert(m.id);
                s.screen = Some((s.turn, m.clone()));
                send(out, json!({"type":"media","mode":"main","item":m}));
            }
            None => send(out, json!({"type":"media_miss","id":id})),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msg(role: &str, text: &str) -> ChatMsg {
        ChatMsg::new(role, text)
    }

    #[test]
    fn seule_la_consigne_d_ouverture_n_est_pas_une_parole() {
        assert!(is_instruction(&msg("user", &opening_instruction(&UserInfo::default()))));
        assert!(!is_instruction(&msg("user", "(je n'ai plus d'imprimante)")));
        assert!(!is_instruction(&msg("assistant", &format!("{OPENING_PREFIX}…)"))));
    }

    #[test]
    fn la_question_est_decrite_par_les_derniers_messages_de_l_utilisateur() {
        // ouverture : seule la consigne du moteur dans l'historique → rien à chercher
        let h = vec![msg("user", &opening_instruction(&UserInfo::default()))];
        assert!(scene_parts(&h, "").is_empty());
        // plus tard : les trois derniers messages de l'utilisateur (le plus récent d'abord, le plus lourd), puis le résumé
        let h = vec![msg("assistant", "a"), msg("user", "b"), msg("assistant", "c"), msg("user", "d"), msg("user", "e"), msg("user", "f")];
        assert_eq!(
            scene_parts(&h, "Résumé."),
            vec![("f".into(), 1.0), ("e".into(), 0.7), ("d".into(), 0.5), ("Résumé.".into(), 0.3)]
        );
        // la consigne d'ouverture au milieu de l'historique n'est pas une parole
        let h = vec![msg("user", "Bonjour"), msg("assistant", "Salut"), msg("user", &opening_instruction(&UserInfo::default())), msg("user", "Mon VPN ne marche pas")];
        let parts = scene_parts(&h, "");
        assert_eq!(parts.iter().map(|(t, _)| t.as_str()).collect::<Vec<_>>(), vec!["Mon VPN ne marche pas", "Bonjour"]);
    }

    #[test]
    fn les_reponses_de_l_assistant_ne_pilotent_pas_la_recherche() {
        // Le message d'accueil (« …support informatique… décrivez votre problème ») et les réponses précédentes, tirées des extraits
        // déjà proposés, ramèneraient toujours les mêmes extraits : seule la parole de l'utilisateur décrit la question.
        let h = vec![
            msg("assistant", "Bonjour, je suis Repère, l'assistant du support informatique. Décrivez votre problème."),
            msg("user", "Mon VPN affiche l'erreur 809"),
            msg("assistant", "Autorisez les ports UDP 500 et 4500 dans le pare-feu."),
            msg("user", "Ça ne marche toujours pas"),
        ];
        let parts = scene_parts(&h, "");
        assert_eq!(parts.iter().map(|(t, _)| t.as_str()).collect::<Vec<_>>(), vec!["Ça ne marche toujours pas", "Mon VPN affiche l'erreur 809"]);
    }

    #[test]
    fn l_ia_ne_montre_que_les_sources_proposees_et_pas_deja_montrees() {
        let lib = KnowledgeBase::open_in_memory().unwrap();
        let item = |url: &str| Source { url: url.into(), description: "Erreur VPN 809".into(), ..Default::default() };
        let (a, b, c) = (lib.insert(&item("/a.png")).unwrap(), lib.insert(&item("/b.png")).unwrap(), lib.insert(&item("/c.png")).unwrap());
        let mut s = Session::new(false);
        s.offered = vec![a, b];
        s.shown.insert(77);
        assert_eq!(allowed_pick(&lib, &s, a).map(|m| m.id), Some(a));
        assert!(allowed_pick(&lib, &s, c).is_none(), "jamais proposée à ce tour : un numéro inventé ne montre rien");
        assert!(allowed_pick(&lib, &s, 999).is_none());
        s.shown.insert(a);
        assert!(allowed_pick(&lib, &s, a).is_none(), "déjà montrée");
        assert_eq!(allowed_pick(&lib, &s, b).map(|m| m.id), Some(b));
    }

    fn document_de_test(lib: &KnowledgeBase) -> Vec<i64> {
        use crate::kb::{NewDocument, NewPassage};
        let d = NewDocument { name: "vpn.md".into(), title: "Guide VPN".into(), format: "md".into(), hash: "h".repeat(64), url: Some("https://intranet.exemple.fr/vpn".into()), ..Default::default() };
        let passages = [("Erreur 809", "Autorisez les ports UDP 500 et 4500."), ("Erreur 691", "Vérifiez le mot de passe : Printemps2024!")]
            .map(|(section, text)| NewPassage { section: section.into(), text: text.into(), page: None });
        let (doc, _) = lib.add_document(&d, &passages).unwrap();
        lib.passages_of(doc).unwrap().into_iter().map(|p| p.id).collect()
    }

    #[test]
    fn l_ia_ne_cite_que_les_extraits_proposes_a_ce_tour() {
        let lib = KnowledgeBase::open_in_memory().unwrap();
        let ids = document_de_test(&lib);
        let mut s = Session::new(false);
        s.offered = vec![ids[0]];
        let p = allowed_pick(&lib, &s, ids[0]).expect("proposé : citable");
        assert!(p.is_passage() && p.section == "Erreur 809");
        assert!(allowed_pick(&lib, &s, ids[1]).is_none(), "existe dans la base mais n'a pas été proposé à ce tour");
        assert!(allowed_pick(&lib, &s, 99_999).is_none(), "numéro inventé");
        // un extrait peut être cité à plusieurs tours (contrairement à une image, montrée une seule fois)
        s.shown.insert(ids[0]);
        assert!(allowed_pick(&lib, &s, ids[0]).is_some());
        // le navigateur reçoit le titre, la section, l'adresse d'origine et un aperçu sans mot de passe
        let j = source_json(&allowed_pick(&lib, &{ let mut t = Session::new(false); t.offered = vec![ids[1]]; t }, ids[1]).unwrap());
        assert_eq!((j["title"].as_str(), j["section"].as_str(), j["link"].as_str()), (Some("Guide VPN"), Some("Erreur 691"), Some("https://intranet.exemple.fr/vpn")));
        let apercu = j["excerpt"].as_str().unwrap();
        assert!(apercu.contains("[masqué]") && !apercu.contains("Printemps2024"), "{apercu}");
    }

    #[test]
    fn une_ia_qui_ne_sait_pas_est_reconnue_et_le_bouton_de_ticket_aussi() {
        for t in [
            "Je ne sais pas : je n'ai pas trouvé cela dans la base de connaissances.",
            "Désolé, je n’ai pas trouvé de procédure pour ce logiciel.",
            "JE NE SAIS PAS répondre à cela.",
            "Aucune source ne parle de cette imprimante.",
        ] {
            assert!(says_unknown(t), "{t}");
        }
        for t in ["Voici la marche à suivre : 1. Redémarrez.", "Je sais que cela peut gêner, mais voici la solution.", "Bonjour !", ""] {
            assert!(!says_unknown(t), "{t}");
        }
        let items = |l: &[&str]| l.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(ticket_button(&items(&["Ça a marché", "Ouvrir un ticket"])).map(String::as_str), Some("Ouvrir un ticket"));
        assert_eq!(ticket_button(&items(&["Créer un Ticket", "Reformuler"])).map(String::as_str), Some("Créer un Ticket"));
        assert_eq!(ticket_button(&items(&["Ça a marché", "Reformuler"])), None);
        assert_eq!(ticket_button(&items(&["Mon ticketing est lent"])), None, "le mot entier seulement");
        let j = replies_json(&items(&UNKNOWN_REPLIES));
        assert_eq!((j["type"].as_str(), j["items"].as_array().map(Vec::len), j["ticket"].as_str()), (Some("replies"), Some(2), Some("Ouvrir un ticket")));
        assert!(replies_json(&items(&["Oui"]))["ticket"].is_null());
    }

    #[test]
    fn les_citations_suivent_les_messages_quand_les_plus_anciens_sont_resumes() {
        let mut s = Session::new(false);
        s.citations = vec![(1, vec![5]), (3, vec![6, 7]), (7, vec![8])];
        // le décalage fait par compact() : les 4 premiers messages disparaissent
        let n = 4;
        s.citations.retain_mut(|(at, _)| match at.checked_sub(n) {
            Some(shifted) => {
                *at = shifted;
                true
            }
            None => false,
        });
        assert_eq!(s.citations, vec![(3, vec![8])]);
        s.cited = vec![1];
        s.clear_conversation();
        assert!(s.citations.is_empty() && s.cited.is_empty() && s.history.is_empty());
    }

    /// Ce que le travailleur de voix recevrait pour un texte livré par petits jetons.
    fn jobs(chunks: &[&str]) -> Vec<String> {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut sp = Speech::new(tx);
        let mut parser = StreamParser::default();
        let handle = |pieces: Vec<Piece>, sp: &mut Speech| {
            for p in pieces {
                if let Piece::Text(t) = p {
                    sp.feed(&t);
                }
            }
        };
        for c in chunks {
            handle(parser.feed(c), &mut sp);
        }
        handle(parser.finish(), &mut sp);
        sp.finish();
        drop(sp);
        let mut out = vec![];
        while let Ok(j) = rx.try_recv() {
            out.push(j);
        }
        out
    }

    #[test]
    fn la_voix_lit_le_texte_sans_la_mise_en_forme_ni_les_directives() {
        let j = jobs(&["**Important :** redémarrez ", "le poste. Puis [[show: #3]]", " vérifiez la connexion. [[replies: Ça marche | Ouvrir un ticket]]"]);
        assert_eq!(j, vec!["Important : redémarrez le poste.", "Puis vérifiez la connexion."]);
    }

    #[test]
    fn chaque_etape_numerotee_est_lue_a_part() {
        let j = jobs(&["Voici comment faire. 1. Ouvrez le menu.\n2. Cliquez sur OK."]);
        assert_eq!(j, vec!["Voici comment faire.", "1. Ouvrez le menu.", "2. Cliquez sur OK."]);
    }

    #[test]
    fn rien_a_lire_rien_envoye() {
        assert!(jobs(&["✅ ", "**", " 😀"]).is_empty());
    }

    #[test]
    fn la_voix_et_son_style_viennent_du_profil() {
        let p = Persona { voice: Some("marie".into()), tts_exaggeration: Some(0.5), tts_cfg_weight: None, ..Default::default() };
        let (voice, style) = voice_of(Some(&p));
        assert_eq!((voice.as_deref(), style.exaggeration, style.cfg_weight), (Some("marie"), Some(0.5), None));
        let (voice, style) = voice_of(None);
        assert!(voice.is_none() && style.exaggeration.is_none());
    }
}
