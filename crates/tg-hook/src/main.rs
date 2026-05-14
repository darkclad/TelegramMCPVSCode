//! Claude Code Stop hook that bridges a Claude Code session to Telegram:
//! sends a wakeup message on Claude's stop, blocks waiting for the user's
//! reply, then returns the reply to Claude as the next turn.

use tg_hook::cli::CliArgs;
use tg_hook::stop_input::StopInput;

fn main() -> anyhow::Result<()> {
    let _cli = CliArgs::parse_env()?;
    let _input = StopInput::from_stdin()?;
    anyhow::bail!("tg-hook: not implemented yet");
}
