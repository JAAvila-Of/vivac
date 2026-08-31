//! El almacen: un directorio, dos archivos.
//!
//! ```text
//! .vivac/
//!   events    log append-only, JSON por linea   <- FUENTE DE VERDAD
//!   config    project_id y actor opaco
//! ```
//!
//! No hay `index.db` y no hay `state`. `ROADMAP.md` §4 los deja fuera de
//! Tier 0 a proposito: con decenas de nodos plegar el log en memoria es
//! instantaneo, y SQLite entra cuando el pilar de rendimiento lo pida --diez
//! mil nodos, FTS5-- no antes. Guardar la pila aparte seria la segunda sede
//! del mismo estado.

use crate::{clock, id};
use serde::{Deserialize, Serialize};
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

pub const DIR: &str = ".vivac";
pub const LOG: &str = "events";
pub const CONFIG: &str = "config";

#[derive(Debug, Serialize, Deserialize)]
pub struct Config {
    pub version: u32,
    pub project_id: String,
    /// Identificador opaco de esta instalacion. **No lleva correo ni nombre**:
    /// el pilar de seguridad lo prohibe, y veta a `MODEL.md` §3.4.
    pub actor: String,
}

impl Config {
    fn nueva() -> Config {
        Config {
            version: 1,
            project_id: id::ulid(),
            actor: format!("a_{}", &id::ulid()[..12]),
        }
    }
}

pub struct Store {
    pub raiz: PathBuf,
    pub config: Config,
}

/// Sube desde `desde` buscando un `.vivac/`. Sin demonio y sin variable de
/// entorno: la misma regla que git, que ya esta en los dedos de todo el mundo.
pub fn buscar_raiz(desde: &Path) -> Option<PathBuf> {
    let mut d = desde.to_path_buf();
    loop {
        if d.join(DIR).is_dir() {
            return Some(d);
        }
        if !d.pop() {
            return None;
        }
    }
}

impl Store {
    pub fn abrir(raiz: PathBuf) -> std::io::Result<Store> {
        let p = raiz.join(DIR).join(CONFIG);
        let config = match fs::read_to_string(&p) {
            Ok(s) => serde_json::from_str(&s).map_err(std::io::Error::other)?,
            Err(_) => {
                // Un `.vivac/` sin config es de una version anterior o de un
                // borrado a medias. Se completa en vez de fallar: el arbol,
                // que es lo que importa, esta en `events`.
                let c = Config::nueva();
                escribir_config(&raiz, &c)?;
                c
            }
        };
        Ok(Store { raiz, config })
    }

    pub fn crear(raiz: &Path) -> std::io::Result<Store> {
        let d = raiz.join(DIR);
        fs::create_dir_all(&d)?;
        let config = Config::nueva();
        escribir_config(raiz, &config)?;
        if !d.join(LOG).exists() {
            File::create(d.join(LOG))?;
        }
        Ok(Store {
            raiz: raiz.to_path_buf(),
            config,
        })
    }

    pub fn log(&self) -> PathBuf {
        self.raiz.join(DIR).join(LOG)
    }
}

fn escribir_config(raiz: &Path, c: &Config) -> std::io::Result<()> {
    let mut f = File::create(raiz.join(DIR).join(CONFIG))?;
    f.write_all(serde_json::to_string_pretty(c)?.as_bytes())?;
    f.write_all(b"\n")
}

impl Store {
    /// Lee el log entero. Una linea ilegible **no aborta**: se cuenta y se
    /// sigue. Un log a medio escribir tiene que poder leerse, porque si no la
    /// herramienta que guarda el hilo es la que lo pierde.
    pub fn leer(&self) -> std::io::Result<(Vec<crate::event::Evento>, usize)> {
        let f = match File::open(self.log()) {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok((vec![], 0)),
            Err(e) => return Err(e),
        };
        let mut eventos = Vec::new();
        let mut rotas = 0usize;
        for linea in BufReader::new(f).lines() {
            let linea = linea?;
            if linea.trim().is_empty() {
                continue;
            }
            match serde_json::from_str(&linea) {
                Ok(e) => eventos.push(e),
                Err(_) => rotas += 1,
            }
        }
        Ok((eventos, rotas))
    }

    /// Añade eventos al final. Una linea por evento, sin reescribir nada.
    ///
    /// Es el camino critico del turno del agente: presupuesto p99 < 5 ms.
    /// Por eso no hay `fsync` --en Windows cuesta mas que el presupuesto
    /// entero-- y por eso se abre en modo `append`, que hace atomica cada
    /// escritura de una linea y quita la necesidad de un cerrojo.
    pub fn escribir(
        &self,
        cuerpo: Vec<crate::event::Cuerpo>,
        desde_seq: u64,
    ) -> std::io::Result<()> {
        let mut buf = String::with_capacity(256 * cuerpo.len());
        for (i, c) in cuerpo.into_iter().enumerate() {
            let e = crate::event::Evento {
                seq: desde_seq + i as u64 + 1,
                id: id::ulid(),
                ts: clock::now_rfc3339(),
                actor: self.config.actor.clone(),
                lane: "main".into(),
                payload: c,
            };
            buf.push_str(&serde_json::to_string(&e).map_err(std::io::Error::other)?);
            buf.push('\n');
        }
        let mut f = OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.log())?;
        f.write_all(buf.as_bytes())
    }
}

impl Store {
    /// Escribe eventos ya construidos, con su instante original. Solo lo usa
    /// `import`: un arbol que viene de otro sitio conserva sus fechas, porque
    /// si no la migracion aplasta la unica linea de tiempo que tenia.
    pub fn escribir_crudo(&self, eventos: &[crate::event::Evento]) -> std::io::Result<()> {
        let mut buf = String::with_capacity(256 * eventos.len());
        for e in eventos {
            buf.push_str(&serde_json::to_string(e).map_err(std::io::Error::other)?);
            buf.push('\n');
        }
        let mut f = OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.log())?;
        f.write_all(buf.as_bytes())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn busca_hacia_arriba() {
        let tmp = std::env::temp_dir().join(format!("vivac-t-{}", id::ulid()));
        let hondo = tmp.join("a").join("b").join("c");
        fs::create_dir_all(&hondo).unwrap();
        assert!(buscar_raiz(&hondo).is_none());
        Store::crear(&tmp).unwrap();
        assert_eq!(buscar_raiz(&hondo).unwrap(), tmp);
        fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn el_actor_no_lleva_datos_personales() {
        let c = Config::nueva();
        assert!(c.actor.starts_with("a_"));
        assert!(!c.actor.contains('@'));
        assert_ne!(c.actor, whoami_ish());
    }

    fn whoami_ish() -> String {
        std::env::var("USERNAME")
            .or_else(|_| std::env::var("USER"))
            .unwrap_or_default()
    }
}
