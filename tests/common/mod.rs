//! La caja de arena que comparten los tests de integracion.
//!
//! Sin dependencias: `CARGO_BIN_EXE_vivac` lo da cargo y el store es un
//! directorio temporal. Cada prueba siembra su propio arbol, porque uno
//! compartido haria que el orden de ejecucion importara.

use std::path::PathBuf;
use std::process::Command;

const BIN: &str = env!("CARGO_BIN_EXE_vivac");

pub struct Caja(pub PathBuf);

impl Caja {
    /// Un directorio sin `.vivac/`. Para probar que la herramienta calla
    /// donde no la han sembrado.
    pub fn vacia(nombre: &str) -> Caja {
        let d = std::env::temp_dir().join(format!(
            "vivac-v-{nombre}-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&d).unwrap();
        Caja(d)
    }

    pub fn nueva(nombre: &str) -> Caja {
        let d = std::env::temp_dir().join(format!(
            "vivac-t-{nombre}-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&d).unwrap();
        let c = Caja(d);
        c.ok(&["init"]);
        c
    }

    pub fn correr(&self, args: &[&str]) -> (String, i32) {
        let o = Command::new(BIN)
            .current_dir(&self.0)
            .args(args)
            .output()
            .unwrap();
        (
            String::from_utf8_lossy(&o.stdout).into_owned() + &String::from_utf8_lossy(&o.stderr),
            o.status.code().unwrap_or(-1),
        )
    }

    pub fn ok(&self, args: &[&str]) -> String {
        let (s, c) = self.correr(args);
        assert_eq!(c, 0, "`vivac {}` fallo con {c}:\n{s}", args.join(" "));
        s
    }
}

impl Drop for Caja {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).ok();
    }
}
