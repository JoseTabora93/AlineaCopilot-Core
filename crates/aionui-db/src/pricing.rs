//! Tabla de precios por modelo → costo USD (Alinea Fase 2 #3, blueprint §12).
//!
//! Precios en **USD por millón de tokens** (aprox. junio 2026; ajustar aquí). El
//! match es por **substring** del id del modelo (case-insensitive), así un id como
//! `claude-opus-4-8` o `anthropic/claude-3.5-sonnet` cae en su tarifa. Si no hay
//! match → [`DEFAULT`] (conservador). El cache-read es barato; el cache-write
//! tiene premium (se cobra como input ligeramente recargado).

/// Tarifa de un modelo, en USD por millón de tokens.
#[derive(Debug, Clone, Copy)]
pub struct Rate {
    pub input: f64,
    pub output: f64,
    pub cache_read: f64,
    pub cache_write: f64,
}

/// Tarifa por defecto cuando el modelo no está en la tabla (conservadora).
pub const DEFAULT: Rate = Rate {
    input: 3.0,
    output: 15.0,
    cache_read: 0.30,
    cache_write: 3.75,
};

/// (substring del id, tarifa). El primer substring que casa gana, así que van
/// de más específico a más general.
const TABLE: &[(&str, Rate)] = &[
    // Anthropic Claude
    ("claude-opus", Rate { input: 15.0, output: 75.0, cache_read: 1.50, cache_write: 18.75 }),
    ("claude-sonnet", Rate { input: 3.0, output: 15.0, cache_read: 0.30, cache_write: 3.75 }),
    ("claude-haiku", Rate { input: 0.80, output: 4.0, cache_read: 0.08, cache_write: 1.0 }),
    ("claude", Rate { input: 3.0, output: 15.0, cache_read: 0.30, cache_write: 3.75 }),
    // z.ai / GLM (baratos)
    ("glm", Rate { input: 0.60, output: 2.0, cache_read: 0.11, cache_write: 0.60 }),
    ("zai", Rate { input: 0.60, output: 2.0, cache_read: 0.11, cache_write: 0.60 }),
    // MiniMax
    ("minimax", Rate { input: 0.20, output: 1.10, cache_read: 0.04, cache_write: 0.20 }),
    // Qwen (vía OpenRouter)
    ("qwen", Rate { input: 0.40, output: 1.20, cache_read: 0.08, cache_write: 0.40 }),
];

/// Devuelve la tarifa para `model` (substring, case-insensitive) o [`DEFAULT`].
pub fn rate_for(model: Option<&str>) -> Rate {
    let Some(m) = model else { return DEFAULT };
    let lower = m.to_ascii_lowercase();
    for (key, rate) in TABLE {
        if lower.contains(key) {
            return *rate;
        }
    }
    DEFAULT
}

/// Costo estimado en USD para una llamada, dado el modelo y los conteos de tokens.
pub fn estimate_cost_usd(model: Option<&str>, tokens_in: i64, tokens_out: i64, cache_read: i64, cache_write: i64) -> f64 {
    let r = rate_for(model);
    let per_m = |toks: i64, price: f64| (toks.max(0) as f64) / 1_000_000.0 * price;
    per_m(tokens_in, r.input)
        + per_m(tokens_out, r.output)
        + per_m(cache_read, r.cache_read)
        + per_m(cache_write, r.cache_write)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_by_substring_case_insensitive() {
        assert_eq!(rate_for(Some("claude-opus-4-8")).output, 75.0);
        assert_eq!(rate_for(Some("anthropic/Claude-Sonnet-4")).input, 3.0);
        assert_eq!(rate_for(Some("GLM-5.1")).input, 0.60);
        assert_eq!(rate_for(Some("minimax-text")).output, 1.10);
    }

    #[test]
    fn unknown_model_uses_default() {
        let r = rate_for(Some("some-new-model"));
        assert_eq!(r.input, DEFAULT.input);
        assert_eq!(rate_for(None).output, DEFAULT.output);
    }

    #[test]
    fn cost_is_sum_of_components() {
        // 1M input @3 + 1M output @15 = 18.0 para sonnet.
        let c = estimate_cost_usd(Some("claude-sonnet"), 1_000_000, 1_000_000, 0, 0);
        assert!((c - 18.0).abs() < 1e-9, "got {c}");
    }

    #[test]
    fn negative_tokens_clamped_to_zero() {
        assert_eq!(estimate_cost_usd(Some("claude-sonnet"), -5, 0, 0, 0), 0.0);
    }
}
