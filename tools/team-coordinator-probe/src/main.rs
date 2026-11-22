// These candidates remain outside the product's runtime and SQLite process.
#[allow(dead_code)]
#[path = "../../../src/team_authority/mod.rs"]
mod team_authority;

#[cfg(target_os = "linux")]
#[allow(dead_code)]
#[path = "../../../src/team_authority/signed/durable.rs"]
mod durable;

#[cfg(all(target_os = "linux", test))]
#[path = "../../../src/output.rs"]
mod output;

#[allow(dead_code)]
#[path = "../../../src/team_authority/signed/approval.rs"]
mod approval;

#[cfg_attr(not(test), allow(dead_code))]
mod registration;

// Reuse the product's strict proof parser and verifier without its key providers.
#[allow(dead_code)]
#[path = "../../../src/device_keys/proof.rs"]
mod enrollment_proof;

mod failure {
    #[derive(Debug)]
    pub(crate) struct Failure(pub(crate) String);

    impl std::fmt::Display for Failure {
        fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            formatter.write_str(&self.0)
        }
    }

    impl std::error::Error for Failure {}
}

fn refused(message: &str) -> failure::Failure {
    failure::Failure(message.to_owned())
}

fn main() {
    eprintln!("This isolated coordinator probe exposes no network or account commands.");
    std::process::exit(2);
}
