//! Voix : reconnaissance (whisper.cpp `whisper-server`) et synthèse (API OpenAI `/v1/audio/speech`).

use crate::config::{SttConfig, TtsConfig};
use serde_json::json;

#[derive(Clone)]
pub struct Stt {
    http: reqwest::Client,
    cfg: SttConfig,
}

impl Stt {
    pub fn new(cfg: SttConfig) -> Self {
        let http = reqwest::Client::builder()
            .connect_timeout(std::time::Duration::from_secs(3))
            .timeout(std::time::Duration::from_secs(120))
            .build()
            .unwrap_or_default();
        Self { http, cfg }
    }

    /// `wav` : PCM 16 kHz mono (le navigateur l'encode avant l'envoi).
    pub async fn transcribe(&self, wav: Vec<u8>) -> anyhow::Result<String> {
        let part = reqwest::multipart::Part::bytes(wav).file_name("audio.wav").mime_str("audio/wav")?;
        let form = reqwest::multipart::Form::new()
            .part("file", part)
            .text("response_format", "json")
            .text("temperature", "0.0")
            .text("language", self.cfg.language.clone());
        let v = self
            .http
            .post(format!("{}/inference", self.cfg.url.trim_end_matches('/')))
            .multipart(form)
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("serveur du micro injoignable ({e})"))?;
        let status = v.status();
        if !status.is_success() {
            let body = v.text().await.unwrap_or_default();
            anyhow::bail!("le serveur du micro a répondu {status} : {}", body.trim().chars().take(200).collect::<String>());
        }
        let v: serde_json::Value = v.json().await?;
        Ok(v["text"].as_str().unwrap_or("").trim().to_string())
    }

    pub async fn healthy(&self) -> bool {
        probe(&self.http, &self.cfg.url).await
    }
}

/// Sonde de santé : réponse HTTP (quelle qu'elle soit) en moins de 2 s. Un serveur occupé à synthétiser ne doit pas
/// bloquer l'interface plus longtemps.
async fn probe(http: &reqwest::Client, url: &str) -> bool {
    http.get(url).timeout(std::time::Duration::from_secs(2)).send().await.is_ok()
}

/// Réglages de style de voix (propres au persona).
#[derive(Debug, Clone, Copy, Default)]
pub struct TtsStyle {
    pub exaggeration: Option<f32>,
    pub cfg_weight: Option<f32>,
}

#[derive(Clone)]
pub struct Tts {
    http: reqwest::Client,
    cfg: TtsConfig,
    /// Une synthèse à la fois, dans l'ordre d'arrivée (le Mutex de tokio est équitable) : le moteur lance une requête
    /// par phrase en parallèle, et le serveur de voix les traiterait sinon dans le désordre.
    gate: std::sync::Arc<tokio::sync::Mutex<()>>,
}

impl Tts {
    pub fn new(cfg: TtsConfig) -> Self {
        // Une synthèse qui ne finit jamais bloquerait toutes les phrases suivantes (elles passent une à une) :
        // mieux vaut une erreur visible au bout de 2 minutes (première synthèse après chargement incluse).
        let http = reqwest::Client::builder()
            .connect_timeout(std::time::Duration::from_secs(3))
            .timeout(std::time::Duration::from_secs(120))
            .build()
            .unwrap_or_default();
        Self { http, cfg, gate: std::sync::Arc::new(tokio::sync::Mutex::new(())) }
    }

    pub fn mime(&self) -> &'static str {
        match self.cfg.format.as_str() {
            "wav" => "audio/wav",
            "opus" => "audio/ogg",
            "aac" => "audio/aac",
            "flac" => "audio/flac",
            _ => "audio/mpeg",
        }
    }

    pub async fn speak(&self, text: &str, voice: Option<&str>, style: &TtsStyle) -> anyhow::Result<Vec<u8>> {
        let _turn = self.gate.lock().await;
        let mut body = json!({
            "model": self.cfg.model,
            "input": text,
            "voice": voice.unwrap_or(&self.cfg.default_voice),
            "response_format": self.cfg.format,
            "language": self.cfg.language,
        });
        // Champs d'expressivité : ignorés par les serveurs compatibles OpenAI qui ne les connaissent pas.
        if let Some(v) = style.exaggeration { body["exaggeration"] = json!(v); }
        if let Some(v) = style.cfg_weight { body["cfg_weight"] = json!(v); }
        let resp = self
            .http
            .post(format!("{}/v1/audio/speech", self.cfg.url.trim_end_matches('/')))
            .json(&body)
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("serveur de voix injoignable ({e})"))?;
        let status = resp.status();
        if !status.is_success() {
            // Le serveur de voix explique l'échec dans un JSON {"error": "..."} : on le remonte tel quel.
            let raw = resp.text().await.unwrap_or_default();
            anyhow::bail!("{}", describe_error(status.as_u16(), &raw));
        }
        let bytes = resp.bytes().await?;
        if bytes.len() < 64 {
            anyhow::bail!("le serveur de voix a renvoyé un audio vide ({} octets)", bytes.len());
        }
        Ok(bytes.to_vec())
    }

    pub async fn healthy(&self) -> bool {
        probe(&self.http, &self.cfg.url).await
    }
}

/// Message lisible à partir d'une réponse d'erreur du serveur de voix (JSON `{"error": …}` ou texte brut).
fn describe_error(status: u16, body: &str) -> String {
    let detail = serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|v| v["error"].as_str().or_else(|| v["message"].as_str()).map(String::from))
        .unwrap_or_else(|| body.trim().to_string());
    let detail: String = detail.chars().take(300).collect();
    if detail.is_empty() { format!("le serveur de voix a répondu {status}") } else { format!("voix ({status}) : {detail}") }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remonte_l_erreur_du_serveur_de_voix() {
        assert_eq!(
            describe_error(500, r#"{"error":"aucune voix par défaut dans le modèle"}"#),
            "voix (500) : aucune voix par défaut dans le modèle"
        );
        assert_eq!(describe_error(502, "Bad gateway\n"), "voix (502) : Bad gateway");
        assert_eq!(describe_error(404, ""), "le serveur de voix a répondu 404");
        assert!(describe_error(500, &"x".repeat(1000)).len() < 330);
    }
}
