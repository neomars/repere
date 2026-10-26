//! Planificateur de budget mémoire : répartit la VRAM entre poids du modèle,
//! cache KV, tampons de calcul et autres modèles résidents, puis déduit les
//! paramètres de `llama-server` (couches sur GPU, contexte, quantification du KV).

use crate::config::{HardwareConfig, LlmConfig};
use crate::gguf::GgufInfo;
use serde::Serialize;

const COMPUTE_BUFFER_GB: f64 = 0.8;
const GB: f64 = 1024.0 * 1024.0 * 1024.0;

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct LlmPlan {
    pub n_gpu_layers: u32,
    pub ctx_tokens: u32,
    /// Quantification du cache KV : "f16" ou "q8_0".
    pub kv_type: String,
    pub weights_gb: f64,
    pub kv_gb: f64,
    pub vram_used_gb: f64,
    pub vram_budget_gb: f64,
    pub ram_used_gb: f64,
    pub full_offload: bool,
    pub notes: Vec<String>,
}

/// Estimation prudente : attention complète sur toutes les couches (surestime les modèles à fenêtre glissante).
fn kv_gb(llm: &LlmConfig, ctx: u32, bytes_per_elem: f64) -> f64 {
    // K et V : 2 × couches × têtes KV × dim × contexte × octets
    2.0 * llm.n_layers as f64 * llm.n_kv_heads as f64 * llm.head_dim as f64 * ctx as f64
        * bytes_per_elem
        / GB
}

const BPE_Q8_0: f64 = 1.0625; // q8_0 : 34 octets / 32 éléments
/// Taille du micro-lot de llama.cpp (-ub, défaut 512) : s'ajoute à la fenêtre glissante dans le cache SWA.
const N_UBATCH: u32 = 512;

fn pad256(n: u32) -> u32 {
    n.div_ceil(256) * 256
}

/// Cache KV réel de llama.cpp d'après l'en-tête GGUF, couche par couche :
/// couches globales → `pad256(ctx)` cellules ; couches à fenêtre glissante → `pad256(min(ctx, fenêtre + ubatch))`
/// cellules ; octets = cellules × têtes KV × (dim K + dim V) × octets par élément (cf. llama-kv-cache-iswa).
pub fn kv_gb_gguf(info: &GgufInfo, ctx: u32, bytes_per_elem: f64) -> f64 {
    let n = info.block_count as usize;
    let size_base = pad256(ctx);
    let n_swa = info.sliding_window.unwrap_or(0);
    let size_swa = if n_swa > 0 { pad256(size_base.min(n_swa + N_UBATCH)) } else { size_base };
    let d_k_full = info.key_length.filter(|k| *k > 0).unwrap_or_else(|| info.head_dim());
    let d_v_full = info.value_length.filter(|k| *k > 0).unwrap_or(d_k_full);
    let kv_layers = n.saturating_sub(info.shared_kv_layers as usize);
    // Architectures hybrides (Qwen 3.5 / Qwen3-Next) : seule 1 couche sur N est une couche d'attention (avec KV).
    let attn_interval = info
        .full_attention_interval
        .or_else(|| matches!(info.architecture.as_str(), "qwen35" | "qwen35moe" | "qwen3next").then_some(4))
        .filter(|n| *n > 1);
    let mut total = 0.0;
    for il in 0..kv_layers {
        if let Some(every) = attn_interval {
            if !(il as u32 + 1).is_multiple_of(every) { continue; }
        }
        // Sans motif connu, on traite la couche comme globale (estimation prudente).
        let is_swa = n_swa > 0
            && match (info.sliding_window_pattern.get(il), info.sliding_window_pattern_n) {
                (Some(b), _) => *b,
                (None, _) if !info.sliding_window_pattern.is_empty() => false,
                (None, Some(p)) if p > 0 => (il as u32 % p) < p - 1,
                _ => false,
            };
        let heads = info.head_count_kv_per_layer.get(il).copied().unwrap_or(info.head_count_kv).max(1);
        let (dk, dv) = if is_swa {
            let dk = info.key_length_swa.filter(|k| *k > 0).unwrap_or(d_k_full);
            (dk, info.value_length_swa.filter(|k| *k > 0).unwrap_or(dk))
        } else {
            (d_k_full, d_v_full)
        };
        let cells = if is_swa { size_swa } else { size_base } as f64;
        total += cells * heads as f64 * (dk + dv) as f64 * bytes_per_elem;
    }
    total / GB
}

/// Plan mémoire avec l'estimation prudente (sans GGUF).
pub fn plan(hw: &HardwareConfig, llm: &LlmConfig, weights_gb: f64) -> LlmPlan {
    plan_with(hw, llm, weights_gb, &|ctx| kv_gb(llm, ctx, BPE_Q8_0))
}

/// Plan mémoire avec le vrai cache KV calculé depuis l'en-tête GGUF.
pub fn plan_for_gguf(hw: &HardwareConfig, llm: &LlmConfig, weights_gb: f64, info: &GgufInfo) -> LlmPlan {
    plan_with(hw, llm, weights_gb, &|ctx| kv_gb_gguf(info, ctx, BPE_Q8_0))
}

fn plan_with(hw: &HardwareConfig, llm: &LlmConfig, weights_gb: f64, kv_of: &dyn Fn(u32) -> f64) -> LlmPlan {
    let budget = (hw.vram_gb - hw.vram_reserve_gb - hw.vram_other_models_gb).max(0.0);
    let mut notes = Vec::new();

    // 1) KV en q8_0 (≈ moitié moins gros, perte de qualité négligeable) ; on ne réduit le contexte
    //    que si le modèle ne tient vraiment pas.
    let kv_type = "q8_0";
    let mut ctx = llm.ctx_tokens;
    let min_ctx = 4096;

    // 2) Tout sur GPU ?
    let fits_full = |ctx: u32| weights_gb + kv_of(ctx) + COMPUTE_BUFFER_GB <= budget;
    let mut full = fits_full(ctx);
    while !full && ctx > 8192 {
        ctx -= 2048;
        full = fits_full(ctx);
    }
    if full && ctx < llm.ctx_tokens {
        notes.push(format!("contexte réduit à {ctx} tokens pour tout garder sur le GPU"));
    }
    if !full {
        // Réduire le contexte n'a pas suffi : on ne le garde réduit que s'il libère au moins une couche de poids
        // (avec un cache KV minuscule, comme Gemma, couper le contexte de moitié ne rapporte presque rien).
        let per_layer = weights_gb / llm.n_layers as f64;
        if kv_of(llm.ctx_tokens) - kv_of(ctx) < per_layer {
            ctx = llm.ctx_tokens;
        }
    }

    let (n_gpu_layers, vram_weights, ram_weights);
    if full {
        n_gpu_layers = llm.n_layers + 1; // +1 : couche de sortie
        vram_weights = weights_gb;
        ram_weights = 0.0;
    } else {
        // Offload partiel : le KV reste sur GPU (accès fréquents), on remplit le reste de couches.
        ctx = ctx.max(min_ctx);
        let kv = kv_of(ctx);
        let per_layer = weights_gb / llm.n_layers as f64;
        let room = (budget - kv - COMPUTE_BUFFER_GB).max(0.0);
        let layers = ((room / per_layer).floor() as u32).min(llm.n_layers);
        n_gpu_layers = layers;
        vram_weights = layers as f64 * per_layer;
        ram_weights = weights_gb - vram_weights;
        notes.push(format!(
            "offload partiel : {layers}/{} couches sur GPU, {:.1} Go de poids en RAM (génération plus lente)",
            llm.n_layers, ram_weights
        ));
        if ram_weights > hw.ram_gb * 0.8 {
            notes.push("⚠ le modèle dépasse la RAM disponible".into());
        }
    }

    let kv = kv_of(ctx);
    LlmPlan {
        n_gpu_layers,
        ctx_tokens: ctx,
        kv_type: kv_type.into(),
        weights_gb,
        kv_gb: kv,
        vram_used_gb: vram_weights + kv + COMPUTE_BUFFER_GB,
        vram_budget_gb: budget,
        ram_used_gb: ram_weights,
        full_offload: full,
        notes,
    }
}

/// Arguments de ligne de commande pour `llama-server`.
pub fn llama_server_args(plan: &LlmPlan, model_path: &str, port: u16, extra: &[String]) -> Vec<String> {
    let mut a: Vec<String> = vec![
        "-m".into(), model_path.into(),
        "--host".into(), "127.0.0.1".into(),
        "--port".into(), port.to_string(),
        "-c".into(), plan.ctx_tokens.to_string(),
        "-ngl".into(), plan.n_gpu_layers.to_string(),
        "--flash-attn".into(), "on".into(),
        "--cache-type-k".into(), plan.kv_type.clone(),
        "--cache-type-v".into(), plan.kv_type.clone(),
        "--parallel".into(), "1".into(),
        "--jinja".into(),
    ];
    a.extend(extra.iter().cloned());
    a
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct GpuInfo {
    pub name: String,
    pub total_mb: u64,
    pub used_mb: u64,
    /// Version du pilote (ex. « 610.57.04 »), vide si inconnue.
    pub driver: String,
}

/// « 610.57.04 » → 610.57 (comparaison numérique des deux premiers champs).
pub fn driver_at_least(driver: &str, major: u32, minor: u32) -> bool {
    let mut it = driver.split('.').map(|p| p.trim().parse::<u32>().unwrap_or(0));
    let (a, b) = (it.next().unwrap_or(0), it.next().unwrap_or(0));
    (a, b) >= (major, minor)
}

/// Sortie de `nvidia-smi --query-gpu=name,memory.total,memory.used,driver_version --format=csv,noheader,nounits`.
pub fn parse_nvidia_smi(out: &str) -> Vec<GpuInfo> {
    out.lines()
        .filter_map(|l| {
            let mut p = l.rsplitn(4, ',');
            let driver = p.next()?.trim().to_string();
            let used = p.next()?.trim().parse().ok()?;
            let total = p.next()?.trim().parse().ok()?;
            Some(GpuInfo { name: p.next()?.trim().to_string(), total_mb: total, used_mb: used, driver })
        })
        .collect()
}

/// Sortie de `nvidia-smi --query-compute-apps=pid,used_memory --format=csv,noheader,nounits` :
/// mémoire GPU (Mo) par processus. `None` si la mémoire n'est pas disponible (« [N/A] », pilote WDDM, conteneur…),
/// pour ne jamais conclure à tort qu'un processus n'utilise pas le GPU.
pub fn parse_compute_apps(out: &str) -> Option<std::collections::HashMap<u32, u64>> {
    let mut map = std::collections::HashMap::new();
    for l in out.lines().map(str::trim).filter(|l| !l.is_empty()) {
        let (pid, mem) = l.split_once(',')?;
        map.insert(pid.trim().parse().ok()?, mem.trim().parse().ok()?);
    }
    Some(map)
}

/// Exécute nvidia-smi avec un délai maximum : un pilote bloqué (reprise de veille, GPU en erreur) ne doit pas figer
/// l'API ni accumuler des processus.
async fn nvidia_smi(args: &[&str]) -> Option<String> {
    let fut = tokio::process::Command::new("nvidia-smi").args(args).kill_on_drop(true).output();
    let out = tokio::time::timeout(std::time::Duration::from_secs(3), fut).await.ok()?.ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

pub async fn gpu_memory_by_pid() -> Option<std::collections::HashMap<u32, u64>> {
    parse_compute_apps(&nvidia_smi(&["--query-compute-apps=pid,used_memory", "--format=csv,noheader,nounits"]).await?)
}

pub async fn detect_gpu() -> Option<GpuInfo> {
    let out = nvidia_smi(&["--query-gpu=name,memory.total,memory.used,driver_version", "--format=csv,noheader,nounits"]).await?;
    parse_nvidia_smi(&out).into_iter().next()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lit_la_sortie_de_nvidia_smi() {
        let g = parse_nvidia_smi("NVIDIA GeForce RTX 4080 SUPER, 16376, 1204, 610.57.04\n");
        assert_eq!(g, vec![GpuInfo { name: "NVIDIA GeForce RTX 4080 SUPER".into(), total_mb: 16376, used_mb: 1204, driver: "610.57.04".into() }]);
        assert!(parse_nvidia_smi("").is_empty());
    }

    #[test]
    fn memoire_gpu_par_processus() {
        // Sortie réelle d'une machine : llama-server utilise 6700 Mo.
        let m = parse_compute_apps("752372, 6700\n").unwrap();
        assert_eq!(m.get(&752372), Some(&6700));
        assert!(parse_compute_apps("").unwrap().is_empty(), "aucun processus : information positive");
        assert!(parse_compute_apps("1234, [N/A]\n").is_none(), "mémoire illisible : on ne conclut rien");
    }

    #[test]
    fn comparaison_de_versions_du_pilote() {
        assert!(driver_at_least("610.57.04", 570, 26));
        assert!(driver_at_least("570.26", 570, 26));
        assert!(!driver_at_least("550.144.03", 570, 26));
        assert!(!driver_at_least("", 570, 26));
    }

    fn hw() -> HardwareConfig {
        HardwareConfig { vram_gb: 15.0, ram_gb: 64.0, vram_reserve_gb: 1.0, vram_other_models_gb: 1.0, vram_tts_gb: 4.5 }
    }

    fn gemma4_12b() -> GgufInfo {
        // 48 couches : motif 5 locales (8 têtes KV × 256) puis 1 globale (1 tête KV × 512), fenêtre 1024.
        GgufInfo {
            architecture: "gemma4".into(), block_count: 48, context_length: 262144, head_count: 16, head_count_kv: 8,
            key_length: Some(512), key_length_swa: Some(256), sliding_window: Some(1024),
            head_count_kv_per_layer: (0..48).map(|i| if i % 6 == 5 { 1 } else { 8 }).collect(),
            sliding_window_pattern: (0..48).map(|i| i % 6 != 5).collect(),
            ..Default::default()
        }
    }

    #[test]
    fn kv_de_gemma4_12b_calcule_comme_llama_cpp() {
        // Attendu (q8_0 K et V) : SWA 40 × 1536 cellules × 8 × 512 o… = 255 Mio ; global 8 × 16384 × 1 × 1024 = 136 Mio.
        let kv = kv_gb_gguf(&gemma4_12b(), 16384, BPE_Q8_0);
        let mib = kv * 1024.0;
        assert!((mib - 391.0).abs() < 1.0, "{mib} Mio");
        // Le cache des couches glissantes est constant : doubler le contexte n'ajoute que la part globale.
        let kv32 = kv_gb_gguf(&gemma4_12b(), 32768, BPE_Q8_0) * 1024.0;
        assert!((kv32 - mib - 136.0).abs() < 1.0, "{kv32} Mio");
    }

    #[test]
    fn qwen35_hybride_une_couche_d_attention_sur_quatre() {
        let base = GgufInfo {
            architecture: "qwen35".into(), block_count: 32, head_count: 16, head_count_kv: 4, key_length: Some(256),
            ..Default::default()
        };
        let full = GgufInfo { architecture: "llama".into(), ..base.clone() }; // même gabarit, attention partout
        let hybrid = kv_gb_gguf(&base, 16384, BPE_Q8_0);
        assert!((kv_gb_gguf(&full, 16384, BPE_Q8_0) / hybrid - 4.0).abs() < 1e-9, "1 couche sur 4 porte un cache KV");
        // Un intervalle fourni par le GGUF l'emporte sur la valeur par défaut.
        let every2 = GgufInfo { full_attention_interval: Some(2), ..base };
        assert!((kv_gb_gguf(&full, 16384, BPE_Q8_0) / kv_gb_gguf(&every2, 16384, BPE_Q8_0) - 2.0).abs() < 1e-9);
    }

    #[test]
    fn motif_scalaire_gemma3() {
        // n=6 : 5 couches locales (indices 0-4) puis 1 globale (5), sans tableau dans le GGUF.
        let info = GgufInfo {
            block_count: 12, head_count: 8, head_count_kv: 4, key_length: Some(256), sliding_window: Some(512),
            sliding_window_pattern_n: Some(6), ..Default::default()
        };
        let swa_cells = (512 + 512) as f64; // fenêtre + ubatch, déjà multiple de 256
        let glob_cells = 8192.0;
        let per = |cells: f64| cells * 4.0 * 512.0 * BPE_Q8_0;
        let expected = (10.0 * per(swa_cells) + 2.0 * per(glob_cells)) / GB;
        assert!((kv_gb_gguf(&info, 8192, BPE_Q8_0) - expected).abs() < 1e-9);
    }

    #[test]
    fn gemma4_12b_avec_voix_et_whisper_sur_15_go() {
        let llm = LlmConfig { n_layers: 48, ..LlmConfig::default() }; // comme engine::llm_params avec ce GGUF
        // Whisper turbo (≈ 1,2 Go) sur GPU + voix Chatterbox (≈ 4,5 Go) + 1 Go pour l'OS : très juste,
        // le planificateur met quelques couches en RAM plutôt que d'échouer.
        let hw = HardwareConfig { vram_gb: 15.0, ram_gb: 64.0, vram_reserve_gb: 1.0, vram_other_models_gb: 5.7, vram_tts_gb: 4.5 };
        let p = plan_for_gguf(&hw, &llm, 7.4, &gemma4_12b());
        assert!(p.vram_used_gb <= p.vram_budget_gb + 1e-9, "{p:?}");
        assert!(p.n_gpu_layers >= 45, "au plus quelques couches en RAM : {p:?}");
        // Whisper sur CPU (stt.use_gpu = false) : ≈ 4,5 Go d'autres modèles → tout sur GPU, contexte 16k.
        let hw = HardwareConfig { vram_other_models_gb: 4.5, ..hw };
        let p = plan_for_gguf(&hw, &llm, 7.4, &gemma4_12b());
        assert!(p.full_offload, "{p:?}");
        assert_eq!(p.ctx_tokens, 16384);
        // L'estimation prudente (attention complète sur 48 couches) est bien plus pessimiste que le calcul exact.
        let cautious = LlmConfig { n_kv_heads: 8, head_dim: 256, ..llm };
        assert!(kv_gb(&cautious, 16384, BPE_Q8_0) > 5.0 * kv_gb_gguf(&gemma4_12b(), 16384, BPE_Q8_0));
    }

    #[test]
    fn model_12b_q5_tient_entierement_sur_gpu() {
        let p = plan(&hw(), &LlmConfig::default(), 8.7);
        assert!(p.full_offload, "{p:?}");
        assert_eq!(p.ctx_tokens, 16384);
        assert!(p.vram_used_gb <= p.vram_budget_gb);
    }

    #[test]
    fn contexte_conserve_quand_le_reduire_ne_sert_a_rien() {
        // Gemma 4 12B (KV ≈ 0,4 Go) dans un budget trop petit pour tout mettre sur le GPU : on décharge quelques
        // couches mais on garde le contexte de 16k au lieu de le couper à 8k pour rien.
        let llm = LlmConfig { n_layers: 48, ..LlmConfig::default() };
        let hw = HardwareConfig { vram_gb: 12.0, ram_gb: 64.0, vram_reserve_gb: 0.5, vram_other_models_gb: 4.5, vram_tts_gb: 4.5 };
        let p = plan_for_gguf(&hw, &llm, 7.4, &gemma4_12b());
        assert!(!p.full_offload && p.n_gpu_layers > 0, "{p:?}");
        assert_eq!(p.ctx_tokens, 16384, "{p:?}");
        assert!(p.vram_used_gb <= p.vram_budget_gb + 1e-9, "{p:?}");
    }

    #[test]
    fn modele_32b_q4_est_offload_partiel() {
        let llm = LlmConfig { n_layers: 64, n_kv_heads: 8, head_dim: 128, ..LlmConfig::default() };
        let p = plan(&hw(), &llm, 19.0);
        assert!(!p.full_offload);
        assert!(p.n_gpu_layers > 0 && p.n_gpu_layers < 64, "{p:?}");
        assert!(p.vram_used_gb <= p.vram_budget_gb + 0.01, "{p:?}");
        assert!(p.ram_used_gb > 0.0);
    }

    #[test]
    fn contexte_reduit_si_juste() {
        // 13 Go de poids : ne tient pas avec 16k de contexte mais tient avec moins.
        let llm = LlmConfig { n_layers: 48, n_kv_heads: 8, head_dim: 128, ..LlmConfig::default() };
        let p = plan(&hw(), &llm, 11.4);
        assert!(p.full_offload, "{p:?}");
        assert!(p.ctx_tokens < 16384);
    }
}
