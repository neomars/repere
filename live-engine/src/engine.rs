//! Superviseur : charge / décharge les modèles en lançant llama-server, whisper-server et le serveur
//! TTS comme processus enfants, avec le plan mémoire calculé depuis l'architecture RÉELLE du GGUF.

use crate::config::{Config, LlmConfig};
use crate::gguf;
use crate::hardware::{self, GpuInfo, LlmPlan};
use crate::models::{CatalogEntry, Installed, Kind, Models};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

const LOG_LINES: usize = 400;
const READY_TIMEOUT: Duration = Duration::from_secs(300);

#[derive(Debug, Clone, Serialize)]
pub struct ComponentStatus {
    /// stopped | loading | ready | error
    pub state: String,
    pub model: Option<String>,
    pub error: Option<String>,
    pub plan: Option<LlmPlan>,
    /// Alerte non bloquante (ex. le serveur semble tourner sur le CPU).
    pub warning: Option<String>,
    pub pid: Option<u32>,
    /// Modèle de texte chargé avec son projecteur multimodal : il voit les images (analyse de la médiathèque).
    pub vision: bool,
    #[serde(skip)]
    gen: u64,
}

impl Default for ComponentStatus {
    fn default() -> Self {
        Self { state: "stopped".into(), model: None, error: None, plan: None, warning: None, pid: None, vision: false, gen: 0 }
    }
}

struct Running {
    kill: oneshot::Sender<()>,
    task: JoinHandle<()>,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct Selection {
    pub llm: Option<String>,
    /// Le modèle de texte se recharge avec sa vision (+ ≈ 1 Go de VRAM).
    #[serde(default)]
    pub llm_vision: bool,
    pub stt: Option<String>,
    pub tts: Option<String>,
}

/// Dernier relevé nvidia-smi (heure, GPU, mémoire par PID, mémoire par PID demandée ?).
type GpuSnapshot = (std::time::Instant, Option<GpuInfo>, Option<HashMap<u32, u64>>, bool);

pub struct Engine {
    cfg: Arc<Config>,
    models: Arc<Models>,
    status: Arc<Mutex<HashMap<Kind, ComponentStatus>>>,
    logs: Arc<Mutex<HashMap<Kind, VecDeque<String>>>>,
    running: tokio::sync::Mutex<HashMap<Kind, Running>>,
    counter: AtomicU64,
    tts_runtime: Mutex<RuntimeStatus>,
    gpu_cache: Mutex<Option<GpuSnapshot>>,
}

fn port_of(url: &str, default: u16) -> u16 {
    url.trim_end_matches('/').rsplit(':').next().and_then(|p| p.parse().ok()).unwrap_or(default)
}

fn fill(arg: &str, model: &str, dir: &str, voices: &str, host: &str, port: u16) -> String {
    arg.replace("{model}", model)
        .replace("{dir}", dir)
        .replace("{voices}", voices)
        .replace("{host}", host)
        .replace("{port}", &port.to_string())
}

/// Environnement Python isolé du moteur de voix (même convention que scripts/tts/tts-server).
pub fn tts_venv() -> PathBuf {
    if let Ok(v) = std::env::var("REPERE_TTS_VENV") {
        if !v.is_empty() { return v.into(); }
    }
    let base = std::env::var("XDG_DATA_HOME").ok().filter(|v| !v.is_empty()).map(PathBuf::from).unwrap_or_else(|| {
        PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| ".".into())).join(".local/share")
    });
    base.join("repere").join("tts-venv")
}

/// Version de l'installation du moteur de voix (écrite dans `.ready` par scripts/tts/setup-tts.sh).
/// 2 = setuptools<81 (pkg_resources pour le filigrane « perth » de Chatterbox). Une installation plus ancienne
/// est proposée à la mise à jour au lieu d'échouer au chargement.
pub const TTS_RUNTIME_VERSION: &str = "2";

/// Le moteur de voix est prêt si l'environnement installé est de la bonne version (ou en mode test TTS_FAKE=1).
pub fn tts_runtime_ready() -> bool {
    std::env::var("TTS_FAKE").ok().as_deref() == Some("1")
        || std::fs::read_to_string(tts_venv().join(".ready")).map(|v| v.trim() == TTS_RUNTIME_VERSION).unwrap_or(false)
}

/// Environnement présent mais d'une ancienne version (ou installation interrompue).
fn tts_runtime_outdated() -> bool {
    tts_venv().join("bin").join("python").exists()
}

/// Faits servant à expliquer pourquoi la voix est muette (séparés de `Engine` pour être testables).
struct VoiceFacts<'a> {
    managed: bool,
    url: &'a str,
    /// État du serveur de voix : stopped | loading | ready | error
    state: &'a str,
    error: Option<&'a str>,
    /// État du moteur Python : absent | outdated | installing | ready | error
    runtime: &'a str,
    downloaded: bool,
}

fn explain_voice_problem(f: &VoiceFacts) -> String {
    const MODELES: &str = "Admin → Modèles IA → Voix de l'IA";
    if !f.managed {
        return format!("Le serveur de voix ne répond pas sur {} (moteur non géré par l'app : lancez-le vous-même).", f.url);
    }
    match f.state {
        "loading" => "La voix est en cours de chargement (la première fois, plusieurs minutes et un accès à internet) : patientez.".into(),
        "error" => format!(
            "Le chargement de la voix a échoué : {} ({MODELES}, journaux).",
            f.error.unwrap_or("erreur inconnue")
        ),
        "ready" => format!("Le serveur de voix est annoncé prêt mais ne répond plus : rechargez-le ({MODELES})."),
        _ => match f.runtime {
            "absent" => format!("Le moteur de voix n'est pas installé : {MODELES} → « Installer le moteur de voix »."),
            "outdated" => format!("Le moteur de voix est périmé : {MODELES} → « Mettre à jour le moteur de voix »."),
            "installing" => "Le moteur de voix est en cours d'installation : patientez.".into(),
            "error" => format!("L'installation du moteur de voix a échoué ({MODELES}, journaux)."),
            _ if !f.downloaded => format!("Le modèle de voix n'est pas téléchargé : {MODELES} → Télécharger, puis Charger."),
            _ => format!("La voix n'est pas chargée : {MODELES} → Charger."),
        },
    }
}

/// Faits servant à expliquer pourquoi le micro (whisper-server) n'est pas utilisable.
struct MicFacts<'a> {
    managed: bool,
    url: &'a str,
    state: &'a str,
    error: Option<&'a str>,
    /// `whisper-server` existe dans le dossier `bin` ou dans le PATH.
    binary: bool,
    downloaded: bool,
}

fn explain_mic_problem(f: &MicFacts) -> String {
    const MODELES: &str = "Admin → Modèles IA → Micro";
    if !f.managed {
        return format!("Le serveur du micro ne répond pas sur {} (moteur non géré par l'app : lancez-le vous-même).", f.url);
    }
    match f.state {
        "loading" => "Le micro est en cours de chargement (quelques secondes) : patientez.".into(),
        "error" => format!("Le chargement du micro a échoué : {} ({MODELES}, journaux).", f.error.unwrap_or("erreur inconnue")),
        "ready" => format!("Le serveur du micro est annoncé prêt mais ne répond plus : rechargez-le ({MODELES})."),
        _ if !f.binary => "« whisper-server » est absent de l'application : il est compilé pendant « npm run dist:linux » (sans CUDA, il \
            tourne sur le processeur ; SKIP_WHISPER=1 l'exclut). Sans lui, le micro ne peut pas fonctionner.".into(),
        _ if !f.downloaded => format!("Le modèle du micro n'est pas téléchargé : {MODELES} → Télécharger, puis Charger."),
        _ => format!("Le micro n'est pas chargé : {MODELES} → Charger."),
    }
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct RuntimeStatus {
    /// absent | outdated | installing | ready | error
    pub state: String,
    pub error: Option<String>,
}

/// Paramètres du modèle d'après son GGUF (repli : valeurs de la configuration).
pub fn llm_params(info: Option<&gguf::GgufInfo>, base: &LlmConfig) -> LlmConfig {
    let mut p = base.clone();
    if let Some(i) = info {
        p.n_layers = i.block_count;
        p.n_kv_heads = i.head_count_kv.max(1);
        p.head_dim = i.head_dim();
        if i.context_length > 0 {
            p.ctx_tokens = p.ctx_tokens.min(i.context_length);
        }
    }
    p
}

impl Engine {
    pub fn new(cfg: Arc<Config>, models: Arc<Models>) -> Arc<Self> {
        Arc::new(Self {
            cfg,
            models,
            status: Arc::new(Mutex::new(HashMap::new())),
            logs: Arc::new(Mutex::new(HashMap::new())),
            running: tokio::sync::Mutex::new(HashMap::new()),
            counter: AtomicU64::new(1),
            tts_runtime: Mutex::new(RuntimeStatus::default()),
            gpu_cache: Mutex::new(None),
        })
    }

    /// VRAM à garder pour le micro et la voix choisis qui doivent encore se charger, plus la réserve manuelle.
    /// Micro : taille du fichier du modèle choisi + contexte CUDA (≈ 0,5 Go) ; voix : `hardware.vram_tts_gb`, seulement
    /// si le moteur de voix est installé à jour et que la voix n'est pas déjà en erreur (sinon on n'immobilise pas
    /// 4,5 Go pour rien).
    pub fn pending_other_gb(&self) -> f64 {
        let sel = self.selection();
        let mut gb = self.cfg.hardware.vram_other_models_gb;
        if self.cfg.stt.enabled && self.stt_uses_gpu() && self.status_of(Kind::Stt).state != "ready" {
            if let Some(id) = &sel.stt {
                gb += self.models.entry(id).map(|e| e.size_gb * 1.1 + 0.5).unwrap_or(1.2);
            }
        }
        if self.cfg.tts.enabled
            && sel.tts.is_some()
            && !matches!(self.status_of(Kind::Tts).state.as_str(), "ready" | "error")
            && tts_runtime_ready()
        {
            gb += self.cfg.hardware.vram_tts_gb;
        }
        gb
    }

    pub fn status_of(&self, k: Kind) -> ComponentStatus {
        self.status.lock().unwrap().get(&k).cloned().unwrap_or_default()
    }

    fn set_status(&self, k: Kind, gen: u64, f: impl FnOnce(&mut ComponentStatus)) {
        let mut g = self.status.lock().unwrap();
        let st = g.entry(k).or_default();
        if gen == 0 || st.gen == gen {
            f(st);
        }
    }

    /// Plan du LLM chargé, ou plan de repli calculé depuis la configuration.
    pub fn llm_plan(&self) -> LlmPlan {
        self.status_of(Kind::Llm)
            .plan
            .unwrap_or_else(|| hardware::plan(&self.cfg.hardware, &self.cfg.llm, self.cfg.llm.model_size_gb))
    }

    pub fn logs(&self, k: Kind, n: usize) -> Vec<String> {
        let g = self.logs.lock().unwrap();
        g.get(&k).map(|d| d.iter().rev().take(n).rev().cloned().collect()).unwrap_or_default()
    }

    fn push_log(logs: &Mutex<HashMap<Kind, VecDeque<String>>>, k: Kind, line: String) {
        let mut g = logs.lock().unwrap();
        let d = g.entry(k).or_default();
        if d.len() >= LOG_LINES { d.pop_front(); }
        d.push_back(line);
    }

    fn resolve_bin(&self, name: &str) -> PathBuf {
        let p = self.cfg.server.bin_dir.join(name);
        if p.exists() { p } else { PathBuf::from(name) }
    }

    fn selection_file(&self) -> PathBuf {
        self.cfg.server.data_dir.join("live-state.json")
    }

    pub fn selection(&self) -> Selection {
        std::fs::read_to_string(self.selection_file()).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
    }

    fn remember(&self, k: Kind, id: Option<&str>) {
        let mut s = self.selection();
        let v = id.map(String::from);
        match k {
            Kind::Llm => { if v.is_none() { s.llm_vision = false } s.llm = v }
            Kind::Stt => s.stt = v,
            Kind::Tts => s.tts = v,
        }
        let _ = std::fs::create_dir_all(&self.cfg.server.data_dir);
        let _ = std::fs::write(self.selection_file(), serde_json::to_vec_pretty(&s).unwrap_or_default());
    }

    fn remember_vision(&self, v: bool) {
        let mut s = self.selection();
        s.llm_vision = v;
        let _ = std::fs::write(self.selection_file(), serde_json::to_vec_pretty(&s).unwrap_or_default());
    }

    pub async fn gpu(&self) -> Option<GpuInfo> {
        hardware::detect_gpu().await
    }

    /// Charge un modèle installé : lance le serveur correspondant. Retourne dès que le processus est lancé ;
    /// l'état passe à `ready` quand le serveur répond (voir `status`).
    pub async fn load(self: &Arc<Self>, id: &str) -> Result<(), String> {
        self.load_opts(id, None).await
    }

    /// `vision` : `Some(true)` charge le modèle avec son projecteur multimodal (il voit les images), `Some(false)` sans ;
    /// `None` reprend le dernier choix pour ce modèle (rechargement au démarrage).
    pub async fn load_opts(self: &Arc<Self>, id: &str, vision: Option<bool>) -> Result<(), String> {
        if !self.cfg.engine.managed {
            return Err("moteur non géré (engine.managed = false) : lance les serveurs toi-même".into());
        }
        let entry = self.models.entry(id).ok_or("modèle inconnu")?.clone();
        if entry.kind == Kind::Tts && !tts_runtime_ready() {
            return Err(if tts_runtime_outdated() {
                "moteur de voix à mettre à jour : clique sur « Mettre à jour le moteur de voix »"
            } else {
                "moteur de voix non installé : clique sur « Installer le moteur de voix »"
            }
            .into());
        }
        let installed = self.models.installed(id).ok_or("modèle non installé : télécharge-le d'abord")?;
        let kind = entry.kind;
        // Vision : seulement pour un modèle de texte qui a son projecteur.
        let want_vision = vision.unwrap_or_else(|| {
            let s = self.selection();
            s.llm.as_deref() == Some(id) && s.llm_vision
        });
        if kind == Kind::Llm && vision == Some(true) && installed.mmproj.is_none() {
            return Err("ce modèle n'a pas son projecteur de vision : clique sur « Ajouter la vision »".into());
        }
        let vision = kind == Kind::Llm && installed.mmproj.is_some() && want_vision;
        self.unload(kind).await;

        let (bin, args, url, any_status, plan) = self.build_spawn(&entry, &installed, vision).await?;
        let gen = self.counter.fetch_add(1, Ordering::SeqCst);
        self.set_status(kind, 0, |s| {
            *s = ComponentStatus { state: "loading".into(), model: Some(id.into()), error: None, plan: plan.clone(), warning: None, pid: None, vision, gen };
        });
        self.remember(kind, Some(id));
        if kind == Kind::Llm {
            self.remember_vision(vision);
        }

        let mut cmd = Command::new(self.resolve_bin(&bin));
        cmd.args(&args).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true);
        // llama-server ne surveille pas son parent : si le moteur est tué brutalement, le noyau lui envoie
        // SIGTERM (sinon ~11 Go de VRAM resteraient occupés par un orphelin).
        #[cfg(target_os = "linux")]
        unsafe {
            cmd.pre_exec(|| {
                libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM as libc::c_ulong);
                Ok(())
            });
        }
        if let Ok(abs) = std::fs::canonicalize(&self.cfg.server.bin_dir) {
            let cur = std::env::var("LD_LIBRARY_PATH").unwrap_or_default();
            cmd.env("LD_LIBRARY_PATH", format!("{}:{cur}", abs.display()));
        }
        Self::push_log(&self.logs, kind, format!("$ {bin} {}", args.join(" ")));
        let mut child = cmd.spawn().map_err(|e| {
            let msg = format!("impossible de lancer « {bin} » ({e}) — est-il dans {} ou dans le PATH ?", self.cfg.server.bin_dir.display());
            self.set_status(kind, gen, |s| { s.state = "error".into(); s.error = Some(msg.clone()) });
            msg
        })?;
        let child_pid = child.id();
        self.set_status(kind, gen, |s| s.pid = child_pid);
        // llama-server / whisper-server démarrent sans erreur sur le CPU si les bibliothèques CUDA manquent :
        // on repère dans leurs journaux qu'un périphérique CUDA a bien été initialisé.
        let gpu_before = self.gpu().await;
        let used_before = gpu_before.as_ref().map(|g| g.used_mb);
        let planned_gpu_layers = plan.as_ref().map(|p| p.n_gpu_layers);
        let expect_cuda = gpu_before.is_some()
            && planned_gpu_layers != Some(0)
            && (kind == Kind::Llm || (kind == Kind::Stt && self.stt_uses_gpu()));
        let cuda_seen = Arc::new(std::sync::atomic::AtomicBool::new(false));
        if let Some(o) = child.stdout.take() { spawn_log_reader(o, self.logs.clone(), kind, false, Some(cuda_seen.clone())); }
        if let Some(e) = child.stderr.take() { spawn_log_reader(e, self.logs.clone(), kind, true, Some(cuda_seen.clone())); }

        let (kill_tx, mut kill_rx) = oneshot::channel::<()>();
        let me = self.clone();
        let task = tokio::spawn(async move {
            let ready = wait_ready(url, any_status);
            tokio::pin!(ready);
            let mut is_ready = false;
            loop {
                tokio::select! {
                    exit = child.wait() => {
                        let msg = match exit {
                            Ok(st) => format!("le processus s'est arrêté ({st}) — voir les journaux"),
                            Err(e) => format!("erreur d'attente du processus : {e}"),
                        };
                        me.set_status(kind, gen, |s| { s.state = "error".into(); s.error = Some(msg) });
                        break;
                    }
                    _ = &mut kill_rx => {
                        terminate(&mut child).await;
                        me.set_status(kind, gen, |s| *s = ComponentStatus { gen, ..Default::default() });
                        break;
                    }
                    ok = &mut ready, if !is_ready => {
                        is_ready = true;
                        if ok {
                            me.set_status(kind, gen, |s| s.state = "ready".into());
                            if expect_cuda {
                                // Tâche séparée : nvidia-smi peut être lent, le moniteur doit rester à l'écoute de l'arrêt.
                                let (me2, seen) = (me.clone(), cuda_seen.clone());
                                tokio::spawn(async move {
                                    tokio::time::sleep(Duration::from_millis(800)).await; // laisser les journaux arriver
                                    if !gpu_in_use(child_pid, &seen, used_before).await {
                                        let driver = hardware::detect_gpu().await.map(|g| g.driver).unwrap_or_default();
                                        let hint = if !driver.is_empty() && !hardware::driver_at_least(&driver, 570, 26) {
                                            format!("Votre pilote NVIDIA ({driver}) est trop ancien pour CUDA 12.8 (≥ 570.26 requis) : mettez-le à jour.")
                                        } else {
                                            "Vérifiez que libggml-cuda.so, libcudart et libcublas sont dans bin/ (voir les journaux).".to_string()
                                        };
                                        me2.set_status(kind, gen, |s| s.warning = Some(format!(
                                            "Ce serveur ne semble pas utiliser le GPU (ni mémoire GPU pour son processus, ni mot-clé CUDA dans ses journaux, ni mémoire GPU en hausse) : il tourne sans doute sur le CPU, très lent. {hint}")));
                                    }
                                });
                            }
                        } else {
                            me.set_status(kind, gen, |s| { s.state = "error".into(); s.error = Some("délai de démarrage dépassé (5 min)".into()) });
                        }
                    }
                }
            }
        });
        self.running.lock().await.insert(kind, Running { kill: kill_tx, task });
        Ok(())
    }

    #[allow(clippy::type_complexity)]
    async fn build_spawn(&self, e: &CatalogEntry, inst: &Installed, vision: bool) -> Result<(String, Vec<String>, String, bool, Option<LlmPlan>), String> {
        let model = inst.files.first().ok_or("modèle sans fichier")?.to_string_lossy().to_string();
        let dir = inst.dir.to_string_lossy().to_string();
        match e.kind {
            Kind::Llm => {
                let info = gguf::read_info(inst.files.first().unwrap()).map_err(|er| format!("GGUF illisible : {er}"))?;
                let params = llm_params(Some(&info), &self.cfg.llm);
                let mut hw = self.cfg.hardware.clone();
                let gpu = self.gpu().await;
                if hw.vram_gb <= 0.0 {
                    hw.vram_gb = gpu.as_ref().map(|g| g.total_mb as f64 / 1024.0).unwrap_or(12.0);
                }
                // On ne réserve de la VRAM que pour la voix et le micro choisis et pas encore chargés.
                hw.vram_other_models_gb = self.pending_other_gb();
                // Le projecteur multimodal et ses tampons de calcul occupent aussi la carte.
                if vision {
                    hw.vram_other_models_gb += inst.mmproj_bytes as f64 / (1024.0 * 1024.0 * 1024.0) * 1.2 + 0.3;
                }
                // Garde-fou : jamais plus que la VRAM réellement libre (navigateur, autres applications…).
                if let Some(g) = &gpu {
                    let free_budget = g.total_mb.saturating_sub(g.used_mb) as f64 / 1024.0 - 0.3 - hw.vram_other_models_gb;
                    if free_budget < hw.vram_gb - hw.vram_reserve_gb - hw.vram_other_models_gb {
                        hw.vram_reserve_gb = (hw.vram_gb - hw.vram_other_models_gb - free_budget).max(hw.vram_reserve_gb);
                    }
                }
                const GB: f64 = 1024.0 * 1024.0 * 1024.0;
                let weights_gb = inst.size_bytes.saturating_sub(info.cpu_resident_bytes) as f64 / GB;
                let mut plan = hardware::plan_for_gguf(&hw, &params, weights_gb, &info);
                if info.cpu_resident_bytes > 0 {
                    let ram = info.cpu_resident_bytes as f64 / GB;
                    plan.ram_used_gb += ram;
                    plan.notes.push(format!("{ram:.1} Go d'embeddings par couche restent en RAM (comptés hors du GPU)"));
                }
                let port = port_of(&self.cfg.llm.url, 8080);
                let mut extra = self.cfg.llm.extra_args.clone();
                extra.extend(e.args.iter().cloned());
                if let (true, Some(mm)) = (vision, &inst.mmproj) {
                    extra.extend(["--mmproj".to_string(), mm.to_string_lossy().to_string()]);
                }
                let args = hardware::llama_server_args(&plan, &model, port, &extra);
                Ok(("llama-server".into(), args, format!("{}/health", self.cfg.llm.url.trim_end_matches('/')), false, Some(plan)))
            }
            Kind::Stt => {
                let port = port_of(&self.cfg.stt.url, 8081);
                let mut args: Vec<String> = vec!["-m".into(), model, "--host".into(), "127.0.0.1".into(), "--port".into(), port.to_string(),
                                                 "-l".into(), self.cfg.stt.language.clone()];
                if !self.stt_uses_gpu() { args.push("-ng".into()); }
                args.extend(["-t".into(), self.cfg.stt.threads.max(1).to_string()]);
                args.extend(e.args.iter().cloned());
                Ok(("whisper-server".into(), args, self.cfg.stt.url.clone(), true, None))
            }
            Kind::Tts => {
                let exec = e.exec.clone().ok_or("entrée TTS sans `exec` dans le catalogue")?;
                let port = port_of(&self.cfg.tts.url, 8880);
                let voices = self.cfg.voices_dir();
                let _ = std::fs::create_dir_all(&voices);
                let args = e.args.iter().map(|a| fill(a, &model, &dir, &voices.to_string_lossy(), "127.0.0.1", port)).collect();
                Ok((exec, args, self.cfg.tts.url.clone(), true, None))
            }
        }
    }

    pub async fn unload(&self, kind: Kind) {
        let r = self.running.lock().await.remove(&kind);
        if let Some(r) = r {
            let _ = r.kill.send(());
            let _ = tokio::time::timeout(Duration::from_secs(15), r.task).await;
        }
        let gen = self.status_of(kind).gen;
        self.set_status(kind, gen, |s| *s = ComponentStatus { gen, ..Default::default() });
        self.remember(kind, None);
    }

    /// Arrêt de l'app : on libère le GPU sans oublier la sélection (rechargée au prochain démarrage).
    pub async fn shutdown(&self) {
        let sel = self.selection();
        for k in [Kind::Tts, Kind::Stt, Kind::Llm] {
            if let Some(r) = self.running.lock().await.remove(&k) {
                let _ = r.kill.send(());
                let _ = tokio::time::timeout(Duration::from_secs(10), r.task).await;
            }
        }
        let _ = std::fs::write(self.selection_file(), serde_json::to_vec_pretty(&sel).unwrap_or_default());
    }

    /// Recharge les derniers modèles choisis (LLM d'abord, puis STT et TTS, pour éviter une course à la VRAM).
    pub async fn autoload(self: &Arc<Self>) {
        let sel = self.selection();
        for (kind, id) in [(Kind::Llm, sel.llm), (Kind::Stt, sel.stt), (Kind::Tts, sel.tts)] {
            let Some(id) = id else { continue };
            if self.models.installed(&id).is_none() { continue }
            match self.load(&id).await {
                Ok(()) => {
                    for _ in 0..(READY_TIMEOUT.as_secs() * 2) {
                        if self.status_of(kind).state != "loading" { break }
                        tokio::time::sleep(Duration::from_millis(500)).await;
                    }
                }
                Err(e) => tracing::warn!("rechargement de {id} impossible : {e}"),
            }
        }
    }

    pub fn tts_runtime_status(&self) -> RuntimeStatus {
        let mut st = self.tts_runtime.lock().unwrap().clone();
        if st.state != "installing" && (st.state != "error" || tts_runtime_ready()) {
            st.state = if tts_runtime_ready() { "ready" } else if tts_runtime_outdated() { "outdated" } else { "absent" }.into();
        }
        st
    }

    /// Ce qui empêche la voix de parler, en français, quand son serveur ne répond pas.
    pub fn voice_problem(&self) -> String {
        let st = self.status_of(Kind::Tts);
        let rt = self.tts_runtime_status();
        let downloaded = self.models.catalog.iter().any(|e| e.kind == Kind::Tts && self.models.installed(&e.id).is_some());
        explain_voice_problem(&VoiceFacts {
            managed: self.cfg.engine.managed,
            url: &self.cfg.tts.url,
            state: &st.state,
            error: st.error.as_deref(),
            runtime: &rt.state,
            downloaded,
        })
    }

    /// Whisper tourne sur le GPU, sauf réglage `stt.use_gpu = false` ou `whisper-server` compilé sans CUDA
    /// (`scripts/build-sidecars.sh` pose alors `bin/whisper-server.cpu`) : pas de VRAM à lui réserver dans ce cas.
    fn stt_uses_gpu(&self) -> bool {
        self.cfg.stt.use_gpu && !self.cfg.server.bin_dir.join("whisper-server.cpu").exists()
    }

    /// `true` si l'exécutable existe dans le dossier `bin` de l'app ou dans le PATH.
    fn bin_available(&self, name: &str) -> bool {
        self.cfg.server.bin_dir.join(name).is_file()
            || std::env::var_os("PATH").is_some_and(|p| std::env::split_paths(&p).any(|d| d.join(name).is_file()))
    }

    /// Ce qui empêche le micro de fonctionner, en français, quand son serveur ne répond pas.
    pub fn mic_problem(&self) -> String {
        let st = self.status_of(Kind::Stt);
        let downloaded = self.models.catalog.iter().any(|e| e.kind == Kind::Stt && self.models.installed(&e.id).is_some());
        explain_mic_problem(&MicFacts {
            managed: self.cfg.engine.managed,
            url: &self.cfg.stt.url,
            state: &st.state,
            error: st.error.as_deref(),
            binary: self.bin_available("whisper-server"),
            downloaded,
        })
    }

    /// Installe le moteur de voix en lançant `setup-tts.sh` ; la sortie va dans les journaux du TTS.
    pub async fn install_tts_runtime(self: &Arc<Self>) -> Result<(), String> {
        {
            let mut g = self.tts_runtime.lock().unwrap();
            if g.state == "installing" { return Ok(()) }
            *g = RuntimeStatus { state: "installing".into(), error: None };
        }
        let script = self.cfg.server.bin_dir.join("setup-tts.sh");
        if !script.exists() {
            let msg = format!("{} introuvable : lance scripts/build-sidecars.sh", script.display());
            *self.tts_runtime.lock().unwrap() = RuntimeStatus { state: "error".into(), error: Some(msg.clone()) };
            return Err(msg);
        }
        let mut cmd = Command::new("bash");
        cmd.arg(&script).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true);
        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                let msg = format!("lancement impossible : {e}");
                *self.tts_runtime.lock().unwrap() = RuntimeStatus { state: "error".into(), error: Some(msg.clone()) };
                return Err(msg);
            }
        };
        Self::push_log(&self.logs, Kind::Tts, format!("$ bash {}", script.display()));
        if let Some(o) = child.stdout.take() { spawn_log_reader(o, self.logs.clone(), Kind::Tts, false, None); }
        if let Some(e) = child.stderr.take() { spawn_log_reader(e, self.logs.clone(), Kind::Tts, true, None); }
        let me = self.clone();
        tokio::spawn(async move {
            let st = match child.wait().await {
                Ok(s) if s.success() && tts_runtime_ready() => RuntimeStatus { state: "ready".into(), error: None },
                Ok(s) => RuntimeStatus { state: "error".into(), error: Some(format!("l'installation a échoué ({s}) — voir les journaux")) },
                Err(e) => RuntimeStatus { state: "error".into(), error: Some(e.to_string()) },
            };
            *me.tts_runtime.lock().unwrap() = st;
        });
        Ok(())
    }

    /// GPU + mémoire par processus pour l'écran Modèles (sondé toutes les 1,5 s) : les deux appels nvidia-smi tournent en
    /// parallèle, la mémoire par processus seulement si un serveur tourne, et le résultat est gardé 1 s.
    async fn gpu_snapshot(&self, want_apps: bool) -> (Option<GpuInfo>, Option<HashMap<u32, u64>>) {
        {
            let c = self.gpu_cache.lock().unwrap();
            if let Some((t, g, a, had_apps)) = c.as_ref() {
                if t.elapsed() < Duration::from_secs(1) && (*had_apps || !want_apps) {
                    return (g.clone(), a.clone());
                }
            }
        }
        let (g, a) = tokio::join!(self.gpu(), async { if want_apps { hardware::gpu_memory_by_pid().await } else { None } });
        *self.gpu_cache.lock().unwrap() = Some((std::time::Instant::now(), g.clone(), a.clone(), want_apps));
        (g, a)
    }

    pub async fn status_json(&self) -> Value {
        let any_pid = [Kind::Llm, Kind::Stt, Kind::Tts].iter().any(|k| self.status_of(*k).pid.is_some());
        let (gpu, apps) = self.gpu_snapshot(any_pid).await;
        let comp = |k: Kind| {
            let st = self.status_of(k);
            let mut v = serde_json::to_value(&st).unwrap_or(Value::Null);
            if let (Some(pid), Some(map)) = (st.pid, &apps) {
                if let Some(mb) = map.get(&pid) { v["gpu_mb"] = json!(mb); }
            }
            v
        };
        json!({
            "managed": self.cfg.engine.managed,
            "gpu": gpu,
            "configured_vram_gb": self.cfg.hardware.vram_gb,
            "llm": comp(Kind::Llm), "stt": comp(Kind::Stt), "tts": comp(Kind::Tts),
            "selection": self.selection(),
            "tts_runtime": self.tts_runtime_status(),
            "hf_token": self.models.has_hf_token(),
        })
    }
}

/// Arrêt en douceur : SIGTERM (les serveurs libèrent proprement le GPU), puis SIGKILL après 4 s.
async fn terminate(child: &mut tokio::process::Child) {
    #[cfg(unix)]
    if let Some(pid) = child.id() {
        // SAFETY : envoi d'un signal à un processus qu'on a lancé nous-mêmes.
        unsafe { libc::kill(pid as libc::pid_t, libc::SIGTERM) };
        if tokio::time::timeout(Duration::from_secs(4), child.wait()).await.is_ok() {
            return;
        }
    }
    let _ = child.kill().await;
}

/// Le processus utilise-t-il vraiment le GPU ? `true` dès qu'un indice positif existe : mot-clé CUDA dans les journaux,
/// ≥ 200 Mo attribués à son PID par nvidia-smi, ou mémoire GPU globale en hausse d'au moins 200 Mo depuis avant le
/// lancement (ce dernier indice couvre les conteneurs/Flatpak/WSL où les PID sont masqués). `false` seulement si
/// toutes ces sources étaient lisibles et négatives ; sinon on ne conclut rien (pas de fausse alerte).
async fn gpu_in_use(pid: Option<u32>, cuda_seen: &std::sync::atomic::AtomicBool, used_before: Option<u64>) -> bool {
    if cuda_seen.load(Ordering::Relaxed) {
        return true;
    }
    let apps = hardware::gpu_memory_by_pid().await;
    if let (Some(pid), Some(apps)) = (pid, &apps) {
        if apps.get(&pid).copied().unwrap_or(0) >= 200 {
            return true;
        }
    }
    let after = hardware::detect_gpu().await.map(|g| g.used_mb);
    if let (Some(b), Some(a)) = (used_before, after) {
        if a.saturating_sub(b) >= 200 {
            return true;
        }
    }
    !(pid.is_some() && apps.is_some() && used_before.is_some() && after.is_some())
}

/// Lit un flux ligne par ligne SANS jamais s'arrêter sur un octet invalide (llama.cpp tronque certaines valeurs de
/// métadonnées au milieu d'un caractère UTF-8 : `lines()` s'arrêtait alors, et les journaux se figeaient).
fn spawn_log_reader(
    stream: impl tokio::io::AsyncRead + Send + Unpin + 'static,
    logs: Arc<Mutex<HashMap<Kind, VecDeque<String>>>>,
    kind: Kind,
    is_err: bool,
    cuda_seen: Option<Arc<std::sync::atomic::AtomicBool>>,
) {
    tokio::spawn(async move {
        let mut r = BufReader::new(stream);
        let mut buf = Vec::new();
        loop {
            buf.clear();
            match r.read_until(b'\n', &mut buf).await {
                Ok(0) | Err(_) => break,
                Ok(_) => {}
            }
            let l = String::from_utf8_lossy(&buf).trim_end().to_string();
            if let Some(seen) = &cuda_seen {
                if mentions_cuda_device(&l) { seen.store(true, Ordering::Relaxed); }
            }
            Engine::push_log(&logs, kind, if is_err { format!("[err] {l}") } else { l });
        }
    });
}

/// Ligne de journal prouvant qu'un backend CUDA est actif (llama.cpp / whisper.cpp).
pub fn mentions_cuda_device(line: &str) -> bool {
    line.contains("ggml_cuda_init: found") || line.contains("loaded CUDA backend") || line.contains("CUDA0")
        || line.contains("Device 0: NVIDIA")
}

async fn wait_ready(url: String, any_status: bool) -> bool {
    let http = reqwest::Client::builder().timeout(Duration::from_secs(2)).build().expect("client");
    let deadline = tokio::time::Instant::now() + READY_TIMEOUT;
    while tokio::time::Instant::now() < deadline {
        if let Ok(r) = http.get(&url).send().await {
            if any_status || r.status().is_success() { return true }
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detection_du_backend_cuda_dans_les_journaux() {
        assert!(mentions_cuda_device("ggml_cuda_init: found 1 CUDA devices:"));
        assert!(mentions_cuda_device("load_backend: loaded CUDA backend from /app/libggml-cuda.so"));
        assert!(mentions_cuda_device("llama_kv_cache: CUDA0 KV buffer size = 391.00 MiB"));
        assert!(!mentions_cuda_device("load_tensors: CPU_Mapped model buffer size = 7400.00 MiB"));
        assert!(!mentions_cuda_device("srv  load_model: loading model"));
    }

    fn engine_with(data: &std::path::Path, stt_gpu: bool) -> Arc<Engine> {
        let mut cfg = Config::default();
        cfg.server.data_dir = data.to_path_buf();
        cfg.stt.use_gpu = stt_gpu;
        cfg.hardware.vram_other_models_gb = 0.0; // les tests comptent les réserves du micro et de la voix seules
        let models = Models::new(crate::models::builtin_catalog(), data.join("models"));
        Engine::new(Arc::new(cfg), models)
    }

    #[test]
    fn reserve_de_vram_pour_le_micro_et_la_voix_choisis() {
        let dir = std::env::temp_dir().join(format!("repere-pending-{}", std::process::id()));
        let venv = dir.join("venv");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&venv).unwrap();
        std::env::set_var("REPERE_TTS_VENV", &venv);
        let write_sel = |stt: Option<&str>, tts: Option<&str>| {
            let sel = Selection { llm: None, llm_vision: false, stt: stt.map(String::from), tts: tts.map(String::from) };
            std::fs::write(dir.join("live-state.json"), serde_json::to_vec(&sel).unwrap()).unwrap();
        };

        let e = engine_with(&dir, true);
        write_sel(None, None);
        assert_eq!(e.pending_other_gb(), 0.0, "rien de choisi : rien à réserver");

        // Whisper small Q5 (0,19 Go de fichier) ⇒ ≈ 0,7 Go, et non un 1,2 Go fixe.
        write_sel(Some("whisper-small-q5"), None);
        assert!((e.pending_other_gb() - (0.19 * 1.1 + 0.5)).abs() < 0.01, "{}", e.pending_other_gb());

        // Voix choisie mais moteur de voix absent/périmé : on n'immobilise pas 4,5 Go pour rien.
        write_sel(None, Some("chatterbox-multilingual"));
        assert_eq!(e.pending_other_gb(), 0.0);
        // Moteur de voix à jour : 4,5 Go réservés.
        std::fs::write(venv.join(".ready"), "2\n").unwrap();
        assert_eq!(e.pending_other_gb(), 4.0);
        // Ancienne installation (.ready vide ou version 1) : « à mettre à jour », donc pas de réservation.
        std::fs::write(venv.join(".ready"), "").unwrap();
        assert!(!tts_runtime_ready());
        assert_eq!(e.pending_other_gb(), 0.0);
        std::fs::write(venv.join(".ready"), "2").unwrap();

        // Whisper sur CPU : seule la voix compte.
        let cpu = engine_with(&dir, false);
        write_sel(Some("whisper-large-v3-turbo-q5"), Some("chatterbox-multilingual"));
        assert_eq!(cpu.pending_other_gb(), 4.0);
        assert!(e.pending_other_gb() > 4.0, "avec Whisper sur GPU : voix + micro");

        // Après le chargement de la voix, elle n'est plus « en attente ».
        e.set_status(Kind::Tts, 0, |s| s.state = "ready".into());
        assert!(e.pending_other_gb() < 4.0);
        std::env::remove_var("REPERE_TTS_VENV");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn explique_pourquoi_la_voix_est_muette() {
        let facts = |state: &'static str, runtime: &'static str, downloaded: bool| VoiceFacts {
            managed: true, url: "http://127.0.0.1:8880", state, error: Some("CUDA out of memory"), runtime, downloaded,
        };
        assert!(explain_voice_problem(&facts("stopped", "absent", false)).contains("Installer le moteur de voix"));
        assert!(explain_voice_problem(&facts("stopped", "outdated", true)).contains("Mettre à jour"));
        assert!(explain_voice_problem(&facts("stopped", "ready", false)).contains("pas téléchargé"));
        assert!(explain_voice_problem(&facts("stopped", "ready", true)).contains("pas chargée"));
        assert!(explain_voice_problem(&facts("loading", "ready", true)).contains("patientez"));
        assert!(explain_voice_problem(&facts("error", "ready", true)).contains("CUDA out of memory"));
        let ext = VoiceFacts { managed: false, ..facts("stopped", "absent", false) };
        assert!(explain_voice_problem(&ext).contains("http://127.0.0.1:8880"));
    }

    #[test]
    fn whisper_compile_sans_cuda_ne_reserve_pas_de_vram() {
        let dir = std::env::temp_dir().join(format!("repere-whisper-cpu-{}", std::process::id()));
        let bin = dir.join("bin");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&bin).unwrap();
        let mut cfg = Config::default();
        cfg.server.data_dir = dir.clone();
        cfg.server.bin_dir = bin.clone();
        cfg.hardware.vram_other_models_gb = 0.0;
        cfg.stt.use_gpu = true; // le test porte sur le marqueur « compilé sans CUDA », pas sur le défaut de la configuration
        let e = Engine::new(Arc::new(cfg), Models::new(crate::models::builtin_catalog(), dir.join("models")));
        let sel = Selection { llm: None, llm_vision: false, stt: Some("whisper-small-q5".into()), tts: None };
        std::fs::write(dir.join("live-state.json"), serde_json::to_vec(&sel).unwrap()).unwrap();
        assert!(e.stt_uses_gpu() && e.pending_other_gb() > 0.5, "binaire CUDA : de la VRAM est à prévoir");
        std::fs::write(bin.join("whisper-server.cpu"), "cpu").unwrap();
        assert!(!e.stt_uses_gpu());
        assert_eq!(e.pending_other_gb(), 0.0, "binaire sans CUDA : rien à réserver");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn explique_pourquoi_le_micro_ne_marche_pas() {
        let facts = |state: &'static str, binary: bool, downloaded: bool| MicFacts {
            managed: true, url: "http://127.0.0.1:8081", state, error: Some("CUDA error"), binary, downloaded,
        };
        assert!(explain_mic_problem(&facts("stopped", false, true)).contains("whisper-server"));
        assert!(explain_mic_problem(&facts("stopped", true, false)).contains("pas téléchargé"));
        assert!(explain_mic_problem(&facts("stopped", true, true)).contains("pas chargé"));
        assert!(explain_mic_problem(&facts("error", true, true)).contains("CUDA error"));
        assert!(explain_mic_problem(&facts("loading", true, true)).contains("patientez"));
        // Une erreur de chargement prime sur « binaire absent » : c'est la vraie cause quand le processus a démarré.
        assert!(explain_mic_problem(&facts("error", false, true)).contains("CUDA error"));
    }

    #[test]
    fn port_depuis_url() {
        assert_eq!(port_of("http://127.0.0.1:8081/", 1), 8081);
        assert_eq!(port_of("http://localhost", 9), 9);
    }

    #[test]
    fn jokers_des_arguments() {
        assert_eq!(fill("--m={model}:{port}@{host} {dir} {voices}", "/m.onnx", "/d", "/v", "h", 7), "--m=/m.onnx:7@h /d /v");
    }

    #[test]
    fn parametres_lus_du_gguf() {
        let info = gguf::GgufInfo {
            architecture: "x".into(), block_count: 48, context_length: 8192, head_count: 16, head_count_kv: 8,
            key_length: Some(256), ..Default::default()
        };
        let p = llm_params(Some(&info), &LlmConfig::default());
        assert_eq!((p.n_layers, p.n_kv_heads, p.head_dim, p.ctx_tokens), (48, 8, 256, 8192));
    }
}
