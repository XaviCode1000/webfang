//! Scrape Summary
//!
//! T-060: ScrapeSummary struct with emoji/ASCII display.

use std::time::Duration;

/// Summary of a scraping run.
pub struct ScrapeSummary {
    /// Total URLs discovered during crawling
    pub urls_discovered: usize,
    /// URLs successfully scraped
    pub urls_scraped: usize,
    /// URLs that failed to scrape
    pub urls_failed: usize,
    /// URLs skipped (e.g., resume mode duplicates)
    pub urls_skipped: usize,
    /// Total content elements extracted
    pub elements_extracted: usize,
    /// Total assets (images, documents) downloaded
    pub assets_downloaded: usize,
    /// Total duration of the scraping run
    pub duration: Duration,
    /// Number of embedding requests made (remote or local)
    pub embedding_requests: usize,
    /// Total prompt tokens consumed by embedding requests
    pub embedding_prompt_tokens: u64,
    /// Total tokens consumed by embedding requests
    pub embedding_total_tokens: u64,
    /// Semantic threshold used for adaptive selectors (Tier 2)
    pub threshold_used: f32,
}

impl ScrapeSummary {
    /// Create a new ScrapeSummary
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        urls_discovered: usize,
        urls_scraped: usize,
        urls_failed: usize,
        urls_skipped: usize,
        elements_extracted: usize,
        assets_downloaded: usize,
        duration: Duration,
        embedding_requests: usize,
        embedding_prompt_tokens: u64,
        embedding_total_tokens: u64,
        threshold_used: f32,
    ) -> Self {
        Self {
            urls_discovered,
            urls_scraped,
            urls_failed,
            urls_skipped,
            elements_extracted,
            assets_downloaded,
            duration,
            embedding_requests,
            embedding_prompt_tokens,
            embedding_total_tokens,
            threshold_used,
        }
    }

    /// Create a ScrapeSummary without embedding metrics (for backward compatibility / non-AI runs)
    pub fn new_basic(
        urls_discovered: usize,
        urls_scraped: usize,
        urls_failed: usize,
        urls_skipped: usize,
        elements_extracted: usize,
        assets_downloaded: usize,
        duration: Duration,
    ) -> Self {
        Self {
            urls_discovered,
            urls_scraped,
            urls_failed,
            urls_skipped,
            elements_extracted,
            assets_downloaded,
            duration,
            embedding_requests: 0,
            embedding_prompt_tokens: 0,
            embedding_total_tokens: 0,
            threshold_used: 0.0,
        }
    }

    /// Format the summary for display.
    ///
    /// When `no_color` is true, uses ASCII markers (`[OK]`, `[FAIL]`, etc.)
    /// instead of emojis.
    pub fn display(&self, no_color: bool) -> String {
        if no_color {
            self.display_ascii()
        } else {
            self.display_emoji()
        }
    }

    fn display_emoji(&self) -> String {
        let secs = self.duration.as_secs();
        let mins = secs / 60;
        let remain_secs = secs % 60;
        let dur = if mins > 0 {
            format!("{mins}m {remain_secs}s")
        } else {
            format!("{remain_secs}s")
        };

        let mut base = format!(
            "\n¡Scrapeo completado! ✅\n\
             ✅ scrapeados: {scraped}\n\
             ❌ fallidos: {failed}\n\
             ⏭️  omitidos: {skipped}\n\
             📊 resumen: {discovered} descubiertos, {elements} elementos extraídos, \
             {assets} assets descargados, {dur} de duración",
            scraped = self.urls_scraped,
            failed = self.urls_failed,
            skipped = self.urls_skipped,
            discovered = self.urls_discovered,
            elements = self.elements_extracted,
            assets = self.assets_downloaded,
        );

        if self.embedding_requests > 0 {
            base.push_str(&format!(
                "\n🧠 embeddings: {reqs} requests, {prompt} prompt tokens, {total} total tokens, threshold={thr:.2}",
                reqs = self.embedding_requests,
                prompt = self.embedding_prompt_tokens,
                total = self.embedding_total_tokens,
                thr = self.threshold_used,
            ));
        }

        base
    }

    fn display_ascii(&self) -> String {
        let secs = self.duration.as_secs();
        let mins = secs / 60;
        let remain_secs = secs % 60;
        let dur = if mins > 0 {
            format!("{mins}m {remain_secs}s")
        } else {
            format!("{remain_secs}s")
        };

        let mut base = format!(
            "\n[OK] scrapeados: {scraped}\n\
             [FALLIDO] fallidos: {failed}\n\
             [OMITIDO] omitidos: {skipped}\n\
             [RESUMEN] {discovered} descubiertos, {elements} elementos extraídos, \
             {assets} assets descargados, {dur} de duración",
            scraped = self.urls_scraped,
            failed = self.urls_failed,
            skipped = self.urls_skipped,
            discovered = self.urls_discovered,
            elements = self.elements_extracted,
            assets = self.assets_downloaded,
        );

        if self.embedding_requests > 0 {
            base.push_str(&format!(
                "\n[EMBEDDINGS] {reqs} requests, {prompt} prompt tokens, {total} total tokens, threshold={thr:.2}",
                reqs = self.embedding_requests,
                prompt = self.embedding_prompt_tokens,
                total = self.embedding_total_tokens,
                thr = self.threshold_used,
            ));
        }

        base
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_summary() -> ScrapeSummary {
        ScrapeSummary::new_basic(10, 7, 2, 1, 42, 5, Duration::from_secs(45))
    }

    fn make_summary_with_embeddings() -> ScrapeSummary {
        ScrapeSummary::new(
            10,
            7,
            2,
            1,
            42,
            5,
            Duration::from_secs(45),
            15,
            12500,
            15000,
            0.75,
        )
    }

    #[test]
    fn test_summary_display_emoji() {
        let s = make_summary();
        let out = s.display(false);
        assert!(out.contains("¡Scrapeo completado!"));
        assert!(out.contains("✅"));
        assert!(out.contains("❌ fallidos: 2"));
        assert!(out.contains("⏭️  omitidos: 1"));
        assert!(out.contains("📊 resumen"));
        assert!(out.contains("45s"));
    }

    #[test]
    fn test_summary_display_no_color() {
        let s = make_summary();
        let out = s.display(true);
        assert!(!out.contains("✅"));
        assert!(!out.contains("❌"));
        assert!(!out.contains("⏭️"));
        assert!(!out.contains("📊"));
        assert!(out.contains("[OK] scrapeados: 7"));
        assert!(out.contains("[FALLIDO] fallidos: 2"));
        assert!(out.contains("[OMITIDO] omitidos: 1"));
        assert!(out.contains("[RESUMEN]"));
        assert!(out.contains("45s"));
    }

    #[test]
    fn test_summary_display_dur_minutes() {
        let s = ScrapeSummary::new_basic(1, 1, 0, 0, 1, 0, Duration::from_secs(90));
        let out = s.display(false);
        assert!(out.contains("1m 30s"));
    }

    #[test]
    fn test_summary_with_embeddings_emoji() {
        let s = make_summary_with_embeddings();
        let out = s.display(false);
        assert!(out.contains("🧠 embeddings:"));
        assert!(out.contains("15 requests"));
        assert!(out.contains("12500 prompt tokens"));
        assert!(out.contains("15000 total tokens"));
        assert!(out.contains("threshold=0.75"));
    }

    #[test]
    fn test_summary_with_embeddings_ascii() {
        let s = make_summary_with_embeddings();
        let out = s.display(true);
        assert!(out.contains("[EMBEDDINGS]"));
        assert!(out.contains("15 requests"));
        assert!(out.contains("12500 prompt tokens"));
        assert!(out.contains("15000 total tokens"));
        assert!(out.contains("threshold=0.75"));
    }

    #[test]
    fn test_summary_without_embeddings_no_embedding_line() {
        let s = make_summary();
        let out = s.display(false);
        assert!(!out.contains("🧠 embeddings"));
        assert!(!out.contains("[EMBEDDINGS]"));
    }
}
