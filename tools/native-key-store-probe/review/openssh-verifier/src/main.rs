use std::io::Read;

mod failure {
    #[derive(Debug)]
    pub struct Failure;
}

mod device_keys {
    use crate::failure::Failure;

    fn refused(_: &str) -> Failure {
        Failure
    }

    mod proof {
        include!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../../src/device_keys/proof.rs"
        ));
    }

    pub fn verify(input: &[u8]) -> Result<(), Failure> {
        let document: proof::Proof = serde_json::from_slice(input).map_err(|_| Failure)?;
        document.verify(proof::current_time()?)
    }
}

fn main() {
    if run().is_err() {
        eprintln!("Possession proof refused.");
        std::process::exit(1);
    }
    println!("Possession proof verified by the shared implementation.");
}

fn run() -> Result<(), ()> {
    let arguments: Vec<_> = std::env::args_os().skip(1).collect();
    if arguments.len() != 1 {
        return Err(());
    }
    let file = std::fs::File::open(&arguments[0]).map_err(|_| ())?;
    let mut input = Vec::new();
    file.take(8193).read_to_end(&mut input).map_err(|_| ())?;
    if input.len() > 8192 {
        return Err(());
    }
    device_keys::verify(&input).map_err(|_| ())
}
