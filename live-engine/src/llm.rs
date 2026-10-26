//! Client LLM : API OpenAI-compatible (llama.cpp `llama-server`, Ollama, vLLM…), en streaming.

use futures_util::{stream, Stream, StreamExt};
use crate::persona::Sampling;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::VecDeque;
use std::pin::Pin;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMsg {
    pub role: String,
    pub content: String,
}

impl ChatMsg {
    pub fn new(role: &str, content: impl Into<String>) -> Self {
        Self { role: role.into(), content: content.into() }
    }
}

#[derive(Clone)]
pub struct LlmClient {
    http: reqwest::Client,
    base: String,
    model: String,
}

type ByteStream = Pin<Box<dyn Stream<Item = reqwest::Result<bytes::Bytes>> + Send>>;

/// Extrait le texte d'une ligne SSE `data: {...}`. `None` = rien à émettre ; `Some(None)` = fin.
fn parse_sse_line(line: &str) -> Option<Option<String>> {
    let line = line.trim();
    let data = line.strip_prefix("data:")?.trim();
    if data == "[DONE]" {
        return Some(None);
    }
    let v: serde_json::Value = serde_json::from_str(data).ok()?;
    // Réponse coupée par la longueur maximale : on le marque (sinon elle s'arrête au milieu d'une phrase sans prévenir).
    if v["choices"][0]["finish_reason"].as_str() == Some("length") {
        return Some(Some("…".to_string()));
    }
    let tok = v["choices"][0]["delta"]["content"].as_str()?;
    if tok.is_empty() { None } else { Some(Some(tok.to_string())) }
}

struct SseState {
    bytes: ByteStream,
    buf: Vec<u8>,
    pending: VecDeque<String>,
    done: bool,
}

impl LlmClient {
    pub fn new(base: &str, model: &str) -> Self {
        Self { http: reqwest::Client::new(), base: base.trim_end_matches('/').into(), model: model.into() }
    }

    pub async fn healthy(&self) -> bool {
        for path in ["/health", "/v1/models"] {
            if let Ok(r) = self.http.get(format!("{}{}", self.base, path)).send().await {
                if r.status().is_success() { return true; }
            }
        }
        false
    }

    /// Corps de la requête. Les noms des champs sont ceux de llama-server ; ce qui n'est pas réglé garde l'ancien comportement
    /// (top_p 0,95 · min_p 0,05 · répétition 1,08).
    fn body(&self, messages: &[ChatMsg], temperature: f32, stream: bool, max_tokens: Option<u32>, s: &Sampling) -> serde_json::Value {
        let mut b = json!({
            "model": self.model,
            "messages": messages,
            "temperature": temperature,
            "stream": stream,
            "repeat_penalty": s.repeat_penalty.unwrap_or(1.08),
        });
        if s.mirostat {
            // llama.cpp ignore top_k / top_p / min_p quand mirostat est actif : inutile de les envoyer.
            b["mirostat"] = json!(2);
            b["mirostat_tau"] = json!(s.mirostat_tau.unwrap_or(5.0));
            b["mirostat_eta"] = json!(s.mirostat_eta.unwrap_or(0.1));
        } else {
            b["top_p"] = json!(s.top_p.unwrap_or(0.95));
            b["min_p"] = json!(s.min_p.unwrap_or(0.05));
        }
        if let Some(p) = s.presence_penalty.filter(|p| *p > 0.0) { b["presence_penalty"] = json!(p); }
        if s.dry {
            b["dry_multiplier"] = json!(0.5); // « léger » (0,8 est la valeur courante, 0 = désactivé)
            b["dry_base"] = json!(1.75);
            b["dry_allowed_length"] = json!(3);
        }
        if s.xtc {
            b["xtc_probability"] = json!(0.25);
            b["xtc_threshold"] = json!(0.1);
        }
        if let Some(m) = max_tokens.or(s.max_tokens) { b["max_tokens"] = json!(m); }
        b
    }

    /// Flux de jetons de texte.
    pub async fn stream(
        &self,
        messages: &[ChatMsg],
        temperature: f32,
        sampling: &Sampling,
    ) -> anyhow::Result<impl Stream<Item = anyhow::Result<String>>> {
        let resp = self
            .http
            .post(format!("{}/v1/chat/completions", self.base))
            .json(&self.body(messages, temperature, true, None, sampling))
            .send()
            .await?;
        if !resp.status().is_success() {
            anyhow::bail!("LLM : HTTP {} {}", resp.status(), resp.text().await.unwrap_or_default());
        }
        let st = SseState { bytes: Box::pin(resp.bytes_stream()), buf: vec![], pending: VecDeque::new(), done: false };
        Ok(stream::unfold(st, |mut st| async move {
            loop {
                if let Some(t) = st.pending.pop_front() {
                    return Some((Ok(t), st));
                }
                if st.done {
                    return None;
                }
                if let Some(pos) = st.buf.iter().position(|&b| b == b'\n') {
                    let line: Vec<u8> = st.buf.drain(..=pos).collect();
                    match parse_sse_line(&String::from_utf8_lossy(&line)) {
                        Some(Some(t)) => st.pending.push_back(t),
                        Some(None) => st.done = true,
                        None => {}
                    }
                    continue;
                }
                match st.bytes.next().await {
                    Some(Ok(b)) => st.buf.extend_from_slice(&b),
                    Some(Err(e)) => {
                        st.done = true;
                        return Some((Err(e.into()), st));
                    }
                    None => st.done = true,
                }
            }
        }))
    }

    /// Réponse complète (utilisé pour résumer l'historique).
    pub async fn complete(&self, messages: &[ChatMsg], temperature: f32, max_tokens: u32) -> anyhow::Result<String> {
        let v: serde_json::Value = self
            .http
            .post(format!("{}/v1/chat/completions", self.base))
            .json(&self.body(messages, temperature, false, Some(max_tokens), &Sampling::default()))
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        Ok(v["choices"][0]["message"]["content"].as_str().unwrap_or("").trim().to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lignes_sse() {
        assert_eq!(parse_sse_line(r#"data: {"choices":[{"delta":{"content":"Salut"}}]}"#), Some(Some("Salut".into())));
        assert_eq!(parse_sse_line("data: [DONE]"), Some(None));
        assert_eq!(parse_sse_line(r#"data: {"choices":[{"delta":{}}]}"#), None);
        assert_eq!(parse_sse_line(": keep-alive"), None);
        // réponse coupée par la longueur maximale
        assert_eq!(parse_sse_line(r#"data: {"choices":[{"delta":{},"finish_reason":"length"}]}"#), Some(Some("…".into())));
        assert_eq!(parse_sse_line(r#"data: {"choices":[{"delta":{},"finish_reason":"stop"}]}"#), None);
    }

    #[test]
    fn corps_de_requete_selon_les_reglages() {
        let c = LlmClient::new("http://x", "m");
        let msgs = [ChatMsg::new("user", "salut")];
        // sans réglage : comme avant
        let b = c.body(&msgs, 0.85, true, None, &Sampling::default());
        assert_eq!((b["top_p"].as_f64(), b["min_p"].as_f64(), b["repeat_penalty"].as_f64()), (Some(0.949999988079071), Some(0.05000000074505806), Some(1.0800000429153442)));
        assert!(b.get("max_tokens").is_none() && b.get("presence_penalty").is_none() && b.get("mirostat").is_none() && b.get("dry_multiplier").is_none());
        // tous les réglages sont transmis
        let s = Sampling { top_p: Some(0.95), min_p: Some(0.04), repeat_penalty: Some(1.10), presence_penalty: Some(0.25), max_tokens: Some(350), dry: true, xtc: true, ..Default::default() };
        let b = c.body(&msgs, 1.2, true, None, &s);
        assert_eq!(b["max_tokens"], 350);
        assert!((b["presence_penalty"].as_f64().unwrap() - 0.25).abs() < 1e-6 && (b["min_p"].as_f64().unwrap() - 0.04).abs() < 1e-6);
        assert!(b["dry_multiplier"].as_f64().unwrap() > 0.0 && b["xtc_probability"].as_f64().unwrap() > 0.0);
        // mirostat 2 : ni top_p ni min_p (ignorés par llama.cpp)
        let s = Sampling { mirostat: true, mirostat_tau: Some(4.8), top_p: Some(0.9), ..Default::default() };
        let b = c.body(&msgs, 0.95, true, None, &s);
        assert_eq!((b["mirostat"].as_i64(), b["mirostat_tau"].as_f64().map(|t| (t * 10.0).round())), (Some(2), Some(48.0)));
        assert!(b.get("top_p").is_none() && b.get("min_p").is_none());
        // un résumé (max_tokens fixé par l'appelant) prime sur la longueur du profil
        assert_eq!(c.body(&msgs, 0.3, false, Some(400), &Sampling { max_tokens: Some(100), ..Default::default() })["max_tokens"], 400);
    }
}
