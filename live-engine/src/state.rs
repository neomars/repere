use crate::config::Config;
use crate::conversation::ConversationStore;
use crate::engine::Engine;
use crate::hardware::LlmPlan;
use crate::llm::LlmClient;
use crate::kb::KnowledgeBase;
use crate::models::Models;
use crate::persona::PersonaStore;
use crate::voice::{Stt, Tts};
use std::sync::Arc;

pub struct AppState {
    pub cfg: Arc<Config>,
    pub kb: KnowledgeBase,
    pub importer: crate::importer::Importer,
    pub ingester: crate::ingest::Ingester,
    pub tickets: crate::tickets::TicketStore,
    pub analyzer: crate::analyzer::Analyzer,
    pub personas: PersonaStore,
    pub conversations: ConversationStore,
    pub llm: LlmClient,
    pub stt: Option<Stt>,
    pub tts: Option<Tts>,
    pub models: Arc<Models>,
    pub engine: Arc<Engine>,
}

impl AppState {
    /// État du micro (reconnaissance vocale) pour l'interface, avec la cause quand il est indisponible.
    pub async fn mic_status(&self) -> serde_json::Value {
        let Some(s) = &self.stt else {
            return serde_json::json!({"enabled": false, "ready": false,
                "problem": "Le micro est désactivé dans la configuration ([stt] enabled = false)."});
        };
        if s.healthy().await {
            serde_json::json!({"enabled": true, "ready": true, "problem": null})
        } else {
            serde_json::json!({"enabled": true, "ready": false, "problem": self.engine.mic_problem()})
        }
    }

    /// État de la voix pour l'interface : activée ? serveur joignable ? sinon pourquoi.
    pub async fn voice_status(&self) -> serde_json::Value {
        let Some(t) = &self.tts else {
            return serde_json::json!({"enabled": false, "ready": false,
                "problem": "La voix est désactivée dans la configuration ([tts] enabled = false)."});
        };
        if t.healthy().await {
            serde_json::json!({"enabled": true, "ready": true, "problem": null})
        } else {
            serde_json::json!({"enabled": true, "ready": false, "problem": self.engine.voice_problem()})
        }
    }

    /// Plan mémoire du LLM actuellement chargé (ou plan de repli de la configuration).
    pub fn plan(&self) -> LlmPlan {
        self.engine.llm_plan()
    }
}
