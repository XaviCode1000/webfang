//! Threshold configuration with builder pattern
//!
//! Provides type-safe configuration for relevance thresholds
//! following the builder pattern (`api-builder-pattern` rust-skill).

/// Builder pattern for threshold configuration
///
/// Configures relevance scoring thresholds with validation.
///
/// # Examples
///
/// ```
/// # #[cfg(feature = "ai")]
/// # fn example() {
/// use rust_scraper::infrastructure::ai::ThresholdConfig;
///
/// let config = ThresholdConfig::new()
///     .with_min_threshold(0.2)
///     .with_max_threshold(0.8)
///     .with_default_threshold(0.5)
///     .build();
///
/// assert_eq!(config.default_threshold(), 0.5);
/// # }
/// ```
#[derive(Debug, Clone)]
pub struct ThresholdConfig {
    /// Minimum allowed threshold (validation boundary)
    min_threshold: f32,
    /// Maximum allowed threshold (validation boundary)
    max_threshold: f32,
    /// Default threshold value
    default_threshold: f32,
}

impl ThresholdConfig {
    /// Create a new ThresholdConfig with default values
    ///
    /// # Defaults
    ///
    /// - `min_threshold`: 0.0
    /// - `max_threshold`: 1.0
    /// - `default_threshold`: 0.3 (moderate relevance)
    #[must_use]
    pub fn new() -> Self {
        Self {
            min_threshold: 0.0,
            max_threshold: 1.0,
            default_threshold: 0.3,
        }
    }

    /// Set the minimum threshold
    ///
    /// # Arguments
    ///
    /// * `threshold` - Minimum allowed value (must be in [0.0, 1.0])
    ///
    /// # Panics
    ///
    /// Panics if threshold is outside [0.0, 1.0] range
    #[must_use]
    pub fn with_min_threshold(mut self, threshold: f32) -> Self {
        assert!(
            (0.0..=1.0).contains(&threshold),
            "Min threshold must be between 0.0 and 1.0, got {}",
            threshold
        );
        self.min_threshold = threshold;
        self
    }

    /// Set the maximum threshold
    ///
    /// # Arguments
    ///
    /// * `threshold` - Maximum allowed value (must be in [0.0, 1.0])
    ///
    /// # Panics
    ///
    /// Panics if threshold is outside [0.0, 1.0] range
    #[must_use]
    pub fn with_max_threshold(mut self, threshold: f32) -> Self {
        assert!(
            (0.0..=1.0).contains(&threshold),
            "Max threshold must be between 0.0 and 1.0, got {}",
            threshold
        );
        self.max_threshold = threshold;
        self
    }

    /// Set the default threshold
    ///
    /// # Arguments
    ///
    /// * `threshold` - Default value (must be in [0.0, 1.0])
    ///
    /// # Panics
    ///
    /// Panics if threshold is outside [0.0, 1.0] range
    #[must_use]
    pub fn with_default_threshold(mut self, threshold: f32) -> Self {
        assert!(
            (0.0..=1.0).contains(&threshold),
            "Default threshold must be between 0.0 and 1.0, got {}",
            threshold
        );
        self.default_threshold = threshold;
        self
    }

    /// Build the configuration
    ///
    /// # Returns
    ///
    /// The configured ThresholdConfig
    ///
    /// # Panics
    ///
    /// Panics if min > max or default is outside [min, max]
    #[must_use]
    pub fn build(self) -> Self {
        assert!(
            self.min_threshold <= self.max_threshold,
            "Min threshold ({}) cannot be greater than max threshold ({})",
            self.min_threshold,
            self.max_threshold
        );

        assert!(
            (self.min_threshold..=self.max_threshold).contains(&self.default_threshold),
            "Default threshold ({}) must be between min ({}) and max ({})",
            self.default_threshold,
            self.min_threshold,
            self.max_threshold
        );

        self
    }

    /// Get the minimum threshold
    #[must_use]
    pub fn min_threshold(&self) -> f32 {
        self.min_threshold
    }

    /// Get the maximum threshold
    #[must_use]
    pub fn max_threshold(&self) -> f32 {
        self.max_threshold
    }

    /// Get the default threshold
    #[must_use]
    pub fn default_threshold(&self) -> f32 {
        self.default_threshold
    }

    /// Validate a threshold value against this config
    ///
    /// # Arguments
    ///
    /// * `threshold` - Value to validate
    ///
    /// # Returns
    ///
    /// `true` if threshold is within [min, max] range
    #[must_use]
    pub fn is_valid(&self, threshold: f32) -> bool {
        (self.min_threshold..=self.max_threshold).contains(&threshold)
    }

    /// Clamp a threshold value to valid range
    ///
    /// # Arguments
    ///
    /// * `threshold` - Value to clamp
    ///
    /// # Returns
    ///
    /// Threshold clamped to [min, max] range
    #[must_use]
    pub fn clamp(&self, threshold: f32) -> f32 {
        threshold.clamp(self.min_threshold, self.max_threshold)
    }

    /// Create a preset configuration for strict filtering
    ///
    /// High threshold for precision-focused filtering.
    ///
    /// # Returns
    ///
    /// ThresholdConfig with min=0.5, max=1.0, default=0.7
    #[must_use]
    pub fn strict() -> Self {
        Self {
            min_threshold: 0.5,
            max_threshold: 1.0,
            default_threshold: 0.7,
        }
    }

    /// Create a preset configuration for lenient filtering
    ///
    /// Low threshold for recall-focused filtering.
    ///
    /// # Returns
    ///
    /// ThresholdConfig with min=0.0, max=0.5, default=0.2
    #[must_use]
    pub fn lenient() -> Self {
        Self {
            min_threshold: 0.0,
            max_threshold: 0.5,
            default_threshold: 0.2,
        }
    }

    /// Create a preset configuration for balanced filtering
    ///
    /// Moderate threshold for balanced precision/recall.
    ///
    /// # Returns
    ///
    /// ThresholdConfig with min=0.1, max=0.9, default=0.4
    #[must_use]
    pub fn balanced() -> Self {
        Self {
            min_threshold: 0.1,
            max_threshold: 0.9,
            default_threshold: 0.4,
        }
    }
}

impl Default for ThresholdConfig {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_threshold_config_new() {
        let config = ThresholdConfig::new();
        assert_eq!(config.min_threshold(), 0.0);
        assert_eq!(config.max_threshold(), 1.0);
        assert_eq!(config.default_threshold(), 0.3);
    }

    #[test]
    fn test_threshold_config_builder() {
        let config = ThresholdConfig::new()
            .with_min_threshold(0.2)
            .with_max_threshold(0.8)
            .with_default_threshold(0.5)
            .build();

        assert_eq!(config.min_threshold(), 0.2);
        assert_eq!(config.max_threshold(), 0.8);
        assert_eq!(config.default_threshold(), 0.5);
    }

    #[test]
    fn test_threshold_config_is_valid() {
        let config = ThresholdConfig::new()
            .with_min_threshold(0.2)
            .with_max_threshold(0.8)
            .build();

        assert!(config.is_valid(0.5));
        assert!(config.is_valid(0.2));
        assert!(config.is_valid(0.8));
        assert!(!config.is_valid(0.1));
        assert!(!config.is_valid(0.9));
    }

    #[test]
    fn test_threshold_config_clamp() {
        let config = ThresholdConfig::new()
            .with_min_threshold(0.2)
            .with_max_threshold(0.8)
            .build();

        assert_eq!(config.clamp(0.5), 0.5);
        assert_eq!(config.clamp(0.1), 0.2);
        assert_eq!(config.clamp(0.9), 0.8);
    }

    #[test]
    fn test_threshold_config_strict() {
        let config = ThresholdConfig::strict();
        assert_eq!(config.min_threshold(), 0.5);
        assert_eq!(config.max_threshold(), 1.0);
        assert_eq!(config.default_threshold(), 0.7);
    }

    #[test]
    fn test_threshold_config_lenient() {
        let config = ThresholdConfig::lenient();
        assert_eq!(config.min_threshold(), 0.0);
        assert_eq!(config.max_threshold(), 0.5);
        assert_eq!(config.default_threshold(), 0.2);
    }

    #[test]
    fn test_threshold_config_balanced() {
        let config = ThresholdConfig::balanced();
        assert_eq!(config.min_threshold(), 0.1);
        assert_eq!(config.max_threshold(), 0.9);
        assert_eq!(config.default_threshold(), 0.4);
    }

    #[test]
    #[should_panic(expected = "Min threshold (0.8) cannot be greater than max threshold (0.2)")]
    fn test_threshold_config_invalid_min_greater_than_max() {
        let _ = ThresholdConfig::new()
            .with_min_threshold(0.8)
            .with_max_threshold(0.2)
            .build();
    }

    #[test]
    #[should_panic(expected = "Default threshold (0.3) must be between min (0.5) and max (1)")]
    fn test_threshold_config_default_below_min() {
        let _ = ThresholdConfig::new()
            .with_min_threshold(0.5)
            .with_max_threshold(1.0)
            .with_default_threshold(0.3)
            .build();
    }

    #[test]
    #[should_panic(expected = "Default threshold (0.8) must be between min (0) and max (0.5)")]
    fn test_threshold_config_default_above_max() {
        let _ = ThresholdConfig::new()
            .with_min_threshold(0.0)
            .with_max_threshold(0.5)
            .with_default_threshold(0.8)
            .build();
    }

    #[test]
    #[should_panic(expected = "Min threshold must be between 0.0 and 1.0, got -0.1")]
    fn test_threshold_config_invalid_min_negative() {
        let _ = ThresholdConfig::new().with_min_threshold(-0.1);
    }

    #[test]
    #[should_panic(expected = "Max threshold must be between 0.0 and 1.0, got 1.1")]
    fn test_threshold_config_invalid_max_above_one() {
        let _ = ThresholdConfig::new().with_max_threshold(1.1);
    }

    #[test]
    fn test_threshold_config_default() {
        let config = ThresholdConfig::default();
        assert_eq!(config.default_threshold(), 0.3);
    }
}
