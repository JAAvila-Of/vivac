use std::time::{Duration, Instant};
use zeroize::Zeroizing;

#[path = "../../../../../src/device_keys/linux_attended_terminal.rs"]
mod attended_terminal;
#[path = "../../../../../src/device_keys/linux_attended_creation_input.rs"]
mod creation_input;
pub(crate) use attended_terminal::read_existing_unlock;
use attended_terminal::Terminal;
pub(crate) use creation_input::{
    read_generated_confirmed, ConfirmedGeneratedSecret, ENCODED_LENGTH,
};

type Result<T> = std::result::Result<T, ()>;

const NEW_PROMPT: &[u8] = b"New device key passphrase: ";
const CONFIRM_PROMPT: &[u8] = b"Confirm device key passphrase: ";

pub fn read_confirmed() -> Result<Zeroizing<Vec<u8>>> {
    // This experiment is single-threaded and handles only the listed blockable
    // signals. SIGKILL and SIGSTOP cannot provide terminal-restoration guarantees.
    let deadline = Instant::now() + Duration::from_secs(15);
    let mut terminal = Terminal::open()?;
    let result = (|| {
        let password = terminal.read_line(NEW_PROMPT, deadline)?;
        let confirmation = terminal.read_line(CONFIRM_PROMPT, deadline)?;
        if password != confirmation {
            return Err(());
        }
        Ok(password)
    })();
    terminal.finish()?;
    result
}
