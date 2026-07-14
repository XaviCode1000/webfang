//! Interactive wizard for guided CLI prompting.
//!
//! Phase 1: TtyDetector trait, StdTtyDetector implementation, WizardPrompt enum.

use std::io::{self, IsTerminal};

/// Trait for detecting terminal capabilities.
pub trait TtyDetector: Send + Sync {
    /// Check if stdin is connected to a terminal.
    fn is_terminal(&self, stream: &dyn IsTty) -> bool;

    /// Check if stdout is connected to a terminal.
    fn stdout_is_terminal(&self) -> bool;

    /// Check if stderr is connected to a terminal.
    fn stderr_is_terminal(&self) -> bool;
}

/// Abstraction for checking if a stream is a TTY.
pub trait IsTty {
    fn is_tty(&self) -> bool;
}

impl IsTty for io::Stdin {
    fn is_tty(&self) -> bool {
        io::stdin().is_terminal()
    }
}

/// Standard TTY detector using std::io::IsTerminal.
pub struct StdTtyDetector;

impl StdTtyDetector {
    pub fn new() -> Self {
        Self
    }
}

impl Default for StdTtyDetector {
    fn default() -> Self {
        Self::new()
    }
}

impl TtyDetector for StdTtyDetector {
    fn is_terminal(&self, stream: &dyn IsTty) -> bool {
        stream.is_tty()
    }

    fn stdout_is_terminal(&self) -> bool {
        io::stdout().is_terminal()
    }

    fn stderr_is_terminal(&self) -> bool {
        io::stderr().is_terminal()
    }
}

/// Prompt types for the interactive wizard.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WizardPrompt {
    /// Confirmation prompt (yes/no).
    Confirmation,
    /// Single selection from a list.
    SingleSelect,
    /// Multiple selection from a list.
    MultiSelect,
    /// Free-form text input.
    Text,
    /// Password input (hidden).
    Password,
    /// Custom prompt with validation.
    Custom,
}

impl WizardPrompt {
    /// Get the prompt type name for display.
    pub fn type_name(&self) -> &'static str {
        match self {
            WizardPrompt::Confirmation => "Confirmation",
            WizardPrompt::SingleSelect => "SingleSelect",
            WizardPrompt::MultiSelect => "MultiSelect",
            WizardPrompt::Text => "Text",
            WizardPrompt::Password => "Password",
            WizardPrompt::Custom => "Custom",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_wizard_prompt_type_name() {
        assert_eq!(WizardPrompt::Confirmation.type_name(), "Confirmation");
        assert_eq!(WizardPrompt::SingleSelect.type_name(), "SingleSelect");
        assert_eq!(WizardPrompt::MultiSelect.type_name(), "MultiSelect");
        assert_eq!(WizardPrompt::Text.type_name(), "Text");
        assert_eq!(WizardPrompt::Password.type_name(), "Password");
        assert_eq!(WizardPrompt::Custom.type_name(), "Custom");
    }

    #[test]
    fn test_wizard_prompt_equality() {
        assert_eq!(WizardPrompt::Confirmation, WizardPrompt::Confirmation);
        assert_ne!(WizardPrompt::Confirmation, WizardPrompt::Text);
    }

    #[test]
    fn test_std_tty_detector_default() {
        // StdTtyDetector is a zero-sized type (ZST), which is valid
        let _detector = StdTtyDetector;
        assert!(std::mem::size_of::<StdTtyDetector>() == 0);
    }
}
