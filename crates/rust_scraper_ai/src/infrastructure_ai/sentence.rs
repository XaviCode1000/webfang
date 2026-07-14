//! Unicode-aware sentence segmentation
//!
//! Uses `unicode-segmentation` crate for proper sentence boundary detection
//! following Unicode Standard Annex #29.

use unicode_segmentation::UnicodeSegmentation;

/// Unicode-aware sentence splitter
///
/// Splits text into sentences using proper Unicode boundary detection.
/// Handles edge cases like:
/// - Abbreviations (Dr., Mr., etc.)
/// - Multiple punctuation (!!, ?!, etc.)
/// - Unicode punctuation (—, …, etc.)
///
/// # Examples
///
/// ```
/// # #[cfg(feature = "ai")]
/// # fn example() {
/// use rust_scraper::infrastructure::ai::SentenceSplitter;
///
/// let splitter = SentenceSplitter;
/// let sentences = splitter.split("Hello world. How are you?");
/// assert!(sentences.len() >= 2);
/// # }
/// ```
pub struct SentenceSplitter;

impl SentenceSplitter {
    /// Split text into sentences (Unicode-aware)
    ///
    /// Returns a vector of sentence slices.
    ///
    /// # Arguments
    ///
    /// * `text` - The text to split
    ///
    /// # Returns
    ///
    /// A vector of sentence slices
    ///
    /// # Examples
    ///
    /// ```
    /// # #[cfg(feature = "ai")]
    /// # fn example() {
    /// use rust_scraper::infrastructure::ai::SentenceSplitter;
    ///
    /// let splitter = SentenceSplitter;
    /// let text = "First sentence. Second sentence! Third?";
    /// let sentences = splitter.split(text);
    /// assert_eq!(sentences.len(), 3);
    /// # }
    /// ```
    #[must_use]
    pub fn split<'a>(&self, text: &'a str) -> Vec<&'a str> {
        text.split_sentence_bounds().collect()
    }

    /// Split text into sentences and trim whitespace
    ///
    /// Convenience method that splits and trims each sentence.
    ///
    /// # Arguments
    ///
    /// * `text` - The text to split
    ///
    /// # Returns
    ///
    /// A vector of trimmed sentence strings
    #[must_use]
    pub fn split_trimmed(&self, text: &str) -> Vec<String> {
        self.split(text)
            .iter()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect()
    }

    /// Count sentences in text
    ///
    /// # Arguments
    ///
    /// * `text` - The text to count sentences in
    ///
    /// # Returns
    ///
    /// The number of sentences detected
    ///
    /// # Examples
    ///
    /// ```
    /// # #[cfg(feature = "ai")]
    /// # fn example() {
    /// use rust_scraper::infrastructure::ai::SentenceSplitter;
    ///
    /// let splitter = SentenceSplitter;
    /// let count = splitter.count("One. Two. Three.");
    /// assert_eq!(count, 3);
    /// # }
    /// ```
    #[must_use]
    pub fn count(&self, text: &str) -> usize {
        self.split(text).len()
    }
}

impl Default for SentenceSplitter {
    fn default() -> Self {
        Self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sentence_splitter_basic() {
        let splitter = SentenceSplitter;
        let text = "Hello world. How are you?";
        let sentences = splitter.split(text);
        assert!(sentences.len() >= 2);
    }

    #[test]
    fn test_sentence_splitter_multiple() {
        let splitter = SentenceSplitter;
        let text = "First. Second! Third? Fourth.";
        let sentences = splitter.split(text);
        assert_eq!(sentences.len(), 4);
    }

    #[test]
    fn test_sentence_splitter_trimmed() {
        let splitter = SentenceSplitter;
        let text = "  First.  Second!  Third?  ";
        let sentences = splitter.split_trimmed(text);
        assert_eq!(sentences.len(), 3);
        assert_eq!(sentences[0], "First.");
        assert_eq!(sentences[1], "Second!");
        assert_eq!(sentences[2], "Third?");
    }

    #[test]
    fn test_sentence_splitter_count() {
        let splitter = SentenceSplitter;
        let text = "One. Two. Three.";
        assert_eq!(splitter.count(text), 3);
    }

    #[test]
    fn test_sentence_splitter_empty() {
        let splitter = SentenceSplitter;
        let text = "";
        let sentences = splitter.split(text);
        assert!(sentences.is_empty());
    }

    #[test]
    fn test_sentence_splitter_single() {
        let splitter = SentenceSplitter;
        let text = "Single sentence without punctuation";
        let sentences = splitter.split(text);
        assert_eq!(sentences.len(), 1);
    }

    #[test]
    fn test_sentence_splitter_unicode() {
        let splitter = SentenceSplitter;
        let text = "Hola mundo. ¿Cómo estás? ¡Bien!";
        let sentences = splitter.split(text);
        assert!(sentences.len() >= 2);
    }
}
