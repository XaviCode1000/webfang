use clap::Args;

#[cfg(feature = "ai")]
fn parse_threshold(s: &str) -> Result<f32, String> {
    let val: f32 = s
        .parse()
        .map_err(|_| format!("'{s}' no es un número válido"))?;
    if !(0.0..=1.0).contains(&val) {
        return Err(format!(
            "'{s}' está fuera de rango (rango válido: 0.0 a 1.0)"
        ));
    }
    Ok(val)
}

/// AI-powered semantic cleaning arguments.
#[derive(Args, Debug, Default)]
pub struct AiArgs {
    /// Relevance threshold for AI semantic filtering (0.0-1.0)
    #[cfg(feature = "ai")]
    #[arg(
        long,
        default_value = "0.3",
        env = "WEBFANG_THRESHOLD",
        value_parser = parse_threshold,
        allow_negative_numbers = true
    )]
    #[clap(next_help_heading = "AI Settings")]
    pub threshold: f32,

    /// Maximum tokens per chunk for AI processing
    #[cfg(feature = "ai")]
    #[arg(long, default_value = "32768", env = "WEBFANG_MAX_TOKENS")]
    #[clap(next_help_heading = "AI Settings")]
    pub max_tokens: usize,

    /// Run AI model in offline mode
    #[cfg(feature = "ai")]
    #[arg(long, default_value = "false", env = "WEBFANG_OFFLINE", action = clap::ArgAction::SetTrue)]
    #[clap(next_help_heading = "AI Settings")]
    pub offline: bool,

    // Raw string on purpose (#827): validation is deferred to the AI init
    // path (`build_ai_cleaner`) so a poisoned AI_MODEL_ID env var cannot
    // make unrelated CLI invocations fail at parse time.
    /// AI model to use: granite-97m (default, fast) or granite-311m (higher quality)
    #[cfg(feature = "ai")]
    #[arg(long, env = "AI_MODEL_ID")]
    #[clap(next_help_heading = "AI Settings")]
    pub ai_model: Option<String>,
}
