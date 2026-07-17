//! Shell completion generation via clap_complete.

use std::io::{self, Write};

use clap::CommandFactory;
use clap_complete::Shell;

/// Generate shell completion script for the given shell.
/// Writes to stdout.
pub fn generate_completions<A>(shell: Shell) -> io::Result<()>
where
    A: CommandFactory,
{
    let mut cmd = A::command();
    let bin_name = cmd.get_name().to_string();
    let mut writer = io::BufWriter::new(io::stdout());
    clap_complete::generate(shell, &mut cmd, &bin_name, &mut writer);
    writer.flush()
}
