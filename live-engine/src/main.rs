mod analyzer;
mod api;
mod config;
mod conversation;
mod director;
mod engine;
mod files;
mod gguf;
mod hardware;
mod importer;
mod llm;
mod media;
mod models;
mod persona;
mod prompt;
mod state;
mod terms;
mod thesaurus;
mod voice;
mod webp;
mod ws;

use crate::config::Config;
use crate::state::AppState;
use axum::Router;
use std::path::PathBuf;
use std::sync::Arc;
use tower_http::services::{ServeDir, ServeFile};

/// Fin de la lecture de stdin : l'app Electron qui nous a lancés est morte (même brutalement) →
/// on s'arrête, ce qui décharge les modèles et libère le GPU. Activé par LIVE_EXIT_ON_STDIN_EOF=1.
async fn parent_gone() {
    if std::env::var("LIVE_EXIT_ON_STDIN_EOF").ok().as_deref() != Some("1") {
        return std::future::pending().await;
    }
    use tokio::io::AsyncReadExt;
    let mut sink = [0u8; 256];
    let mut stdin = tokio::io::stdin();
    while matches!(stdin.read(&mut sink).await, Ok(n) if n > 0) {}
}

async fn shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        let mut term = signal(SignalKind::terminate()).expect("SIGTERM");
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = term.recv() => {}
            _ = parent_gone() => {}
        }
    }
    #[cfg(not(unix))]
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {}
        _ = parent_gone() => {}
    }
}

/// `live-engine plan [id]` : affiche le plan mémoire (lu dans le GGUF si le modèle est installé).
async fn print_plan(cfg: &Config, models: &models::Models, id: Option<String>) -> anyhow::Result<()> {
    // Sans identifiant : le premier modèle de texte installé ; à défaut, un plan estimé avec les valeurs de repli de la configuration.
    let id = id.or_else(|| models.catalog.iter().find(|e| e.kind == models::Kind::Llm && models.installed(&e.id).is_some()).map(|e| e.id.clone()));
    let installed = id.as_deref().and_then(|id| models.installed(id));
    let (params, weights_gb, info) = match &installed {
        Some(i) => {
            let info = gguf::read_info(&i.files[0])?;
            println!("architecture lue dans le GGUF : {info:?}");
            (engine::llm_params(Some(&info), &cfg.llm), i.size_bytes as f64 / (1024.0 * 1024.0 * 1024.0), Some(info))
        }
        None => {
            match &id {
                Some(id) => println!("« {id} » n'est pas installé : plan estimé (prudent) avec les valeurs de repli de la configuration."),
                None => println!("Aucun modèle de texte installé : plan estimé (prudent) avec les valeurs de repli de la configuration."),
            }
            let size = id.as_deref().and_then(|id| models.entry(id)).map(|e| e.size_gb).filter(|s| *s > 0.0).unwrap_or(cfg.llm.model_size_gb);
            (cfg.llm.clone(), size, None)
        }
    };
    let mut hw = cfg.hardware.clone();
    if let Some(g) = hardware::detect_gpu().await {
        println!("GPU détecté : {} — {} Mo (dont {} Mo utilisés)", g.name, g.total_mb, g.used_mb);
        if hw.vram_gb <= 0.0 { hw.vram_gb = g.total_mb as f64 / 1024.0; }
    }
    println!("(pire cas : micro et voix réservés ; au chargement réel, seule la réserve des composants choisis et non chargés compte)");
    hw.vram_other_models_gb += hw.vram_tts_gb + 1.2; // pire cas : voix + micro réservés
    let plan = match &info {
        Some(i) => hardware::plan_for_gguf(&hw, &params, weights_gb, i),
        None => hardware::plan(&hw, &params, weights_gb),
    };
    println!("{}", serde_json::to_string_pretty(&plan)?);
    Ok(())
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();

    let cfg_path = PathBuf::from(std::env::var("LIVE_CONFIG").unwrap_or_else(|_| "live.toml".into()));
    let mut cfg = Config::load(&cfg_path)?;
    if cfg.hardware.vram_gb <= 0.0 {
        // VRAM automatique : la vraie valeur de la carte (ex. 12 Go d'une RTX 4000 Ada portable), pas une hypothèse.
        match hardware::detect_gpu().await {
            Some(g) => {
                cfg.hardware.vram_gb = g.total_mb as f64 / 1024.0;
                tracing::info!("GPU : {} — {:.1} Go de VRAM, pilote {}", g.name, cfg.hardware.vram_gb, g.driver);
            }
            None => {
                cfg.hardware.vram_gb = 12.0;
                tracing::warn!("nvidia-smi indisponible : VRAM supposée à 12 Go (réglable avec hardware.vram_gb)");
            }
        }
    }
    let cfg = Arc::new(cfg);

    let catalog = models::load_catalog(&cfg.server.data_dir);
    let models = models::Models::new(catalog, cfg.models_dir());

    if std::env::args().nth(1).as_deref() == Some("plan") {
        return print_plan(&cfg, &models, std::env::args().nth(2)).await;
    }

    let engine = engine::Engine::new(cfg.clone(), models.clone());
    if cfg.engine.managed && cfg.engine.autoload {
        let e = engine.clone();
        tokio::spawn(async move { e.autoload().await });
    }

    let media = media::MediaLibrary::open(&cfg.server.data_dir.join("live.db"))?;
    let personas = persona::PersonaStore::new(&cfg.server.data_dir.join("personas"))?;
    if personas.ensure_default() {
        tracing::info!("profil d'assistant « Repère » créé au premier lancement");
    }
    let conversations = conversation::ConversationStore::new(&cfg.server.data_dir.join("conversations"))?;
    let bind = cfg.server.bind.clone();
    let uploads = cfg.server.uploads_dir.clone();
    let frontend = cfg.server.frontend_dir.clone();

    let state = Arc::new(AppState {
        llm: llm::LlmClient::new(&cfg.llm.url, &cfg.llm.model),
        stt: cfg.stt.enabled.then(|| voice::Stt::new(cfg.stt.clone())),
        tts: cfg.tts.enabled.then(|| voice::Tts::new(cfg.tts.clone())),
        media,
        importer: importer::Importer::new(),
        analyzer: analyzer::Analyzer::new(),
        personas,
        conversations,
        models,
        engine: engine.clone(),
        cfg,
    });

    // /api/live/*  → moteur ; le reste → médias uploadés, puis frontend compilé (SPA).
    let spa = ServeDir::new(&frontend).fallback(ServeFile::new(frontend.join("index.html")));
    let app = Router::new()
        .nest("/api/live", api::router())
        .fallback_service(ServeDir::new(&uploads).fallback(spa))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(&bind).await?;
    tracing::info!("Repère — http://{bind}");
    axum::serve(listener, app).with_graceful_shutdown(shutdown_signal()).await?;
    tracing::info!("arrêt : libération des modèles");
    engine.shutdown().await;
    Ok(())
}
