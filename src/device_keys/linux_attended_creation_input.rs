use super::attended_terminal::Terminal;
use std::time::{Duration, Instant};
use zeroize::Zeroizing;

#[path = "linux_generated_secret.rs"]
mod generated_secret;
pub(crate) use generated_secret::ENCODED_LENGTH;

type Result<T> = std::result::Result<T, ()>;

const _: [(); ENCODED_LENGTH] = [(); super::attended_terminal::ENCODED_LENGTH];

pub(crate) struct ConfirmedGeneratedSecret {
    secret: Zeroizing<Vec<u8>>,
}

impl ConfirmedGeneratedSecret {
    pub(crate) fn as_bytes(&self) -> &[u8] {
        &self.secret
    }
}

const GENERATED_ACKNOWLEDGEMENT: &[u8] = b"CREATE IN TRUSTED TERMINAL";
const GENERATED_DISCLOSURE: &[u8] = b"Generated device key unlock secret: ";
const GENERATED_CONFIRM_PROMPT: &[u8] = b"\nRe-enter generated secret: ";
const GENERATED_NOTICE: &[u8] = b"Create a device credential only in a trusted, unrecorded terminal.\n\
    Terminal recording, screen sharing and scrollback can retain the generated secret; clearing the screen cannot erase those copies.\n\
    Retain the secret in a separate trusted private place before confirming it. Losing it requires a new credential and revocation of this one; there is no secret reset.\n\
    Type CREATE IN TRUSTED TERMINAL to continue: ";

pub(crate) fn read_generated_confirmed() -> Result<ConfirmedGeneratedSecret> {
    // This reader has the same single-threaded and blockable-signal boundary
    // as the historical terminal experiment.
    let mut terminal = Terminal::open()?;
    let result = (|| {
        let acknowledgement =
            terminal.read_line(GENERATED_NOTICE, Instant::now() + Duration::from_secs(60))?;
        if acknowledgement.as_slice() != GENERATED_ACKNOWLEDGEMENT {
            return Err(());
        }
        terminal.check_active()?;
        let secret = generated_secret::generate()?;
        let mut disclosure = Zeroizing::new(Vec::with_capacity(
            GENERATED_DISCLOSURE.len() + secret.len() + GENERATED_CONFIRM_PROMPT.len(),
        ));
        disclosure.extend_from_slice(GENERATED_DISCLOSURE);
        disclosure.extend_from_slice(&secret);
        disclosure.extend_from_slice(GENERATED_CONFIRM_PROMPT);
        let deadline = Instant::now() + Duration::from_secs(180);
        let confirmation = terminal.read_line(&disclosure, deadline)?;
        if !generated_secret::confirmation_matches(&secret, &confirmation) {
            return Err(());
        }
        Ok(secret)
    })();
    terminal.finish()?;
    result.map(|secret| ConfirmedGeneratedSecret { secret })
}
