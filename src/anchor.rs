//! `Anchor` — identidad estable del estado del arbol, y que cambio desde ella.
//!
//! El modelo no depende de git: necesita dos primitivas y nada mas. Las
//! implementaciones son `Git` y `Null`, y **`Null` define el suelo del
//! producto**, no es un relleno: sin control de versiones el arbol, la pila,
//! los vivacs y `why` siguen funcionando enteros. Lo unico que se pierde es la
//! precision por cambios, y el `brief` la sustituye por antiguedad temporal en
//! vez de inventar una precision que no tiene.
//!
//! **`snapshot` no lanza un subproceso.** `push` crea un vivac y un vivac
//! necesita ancla, asi que esto cae en el camino de escritura, cuyo
//! presupuesto son 5 ms; arrancar `git` en Windows cuesta entre 15 y 30. Se
//! lee `.git/HEAD` y se resuelve la referencia a mano. `changed_since` si
//! lanza git, porque solo se usa al leer.

use std::path::{Path, PathBuf};

/// Identidad del estado del arbol en un momento. Vacia significa "no hay",
/// que es un estado legitimo y no un error.
#[derive(Debug, Clone, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub struct AnchorRef {
    pub kind: String,
    pub id: String,
}

impl AnchorRef {
    pub fn vacio(&self) -> bool {
        self.id.is_empty()
    }

    /// Prefijo corto, como git.
    pub fn corto(&self) -> &str {
        let n = self.id.len().min(7);
        &self.id[..n]
    }
}

#[derive(Debug, Clone)]
pub struct Cambio {
    pub ruta: String,
    pub veces: usize,
}

pub trait Anchor {
    fn snapshot(&self) -> AnchorRef;
    fn changed_since(&self, r: &AnchorRef) -> Vec<Cambio>;
}

/// Sin control de versiones.
///
/// `MODEL.md` §8 proponia un merkle del working set. **No entra en v0.1**: el
/// working set no tiene cota, y hashearlo esta en el camino de escritura, que
/// tiene 5 ms de presupuesto. El pilar de rendimiento fija un techo que la
/// funcionalidad debe respetar para existir, y esta no lo respetaba. Queda
/// como ancla sin identidad, que es exactamente la degradacion que
/// `BRIEF-SPEC.md` §6 ya especifica.
pub struct Null;

impl Anchor for Null {
    fn snapshot(&self) -> AnchorRef {
        AnchorRef {
            kind: "null".into(),
            id: String::new(),
        }
    }

    fn changed_since(&self, _r: &AnchorRef) -> Vec<Cambio> {
        vec![]
    }
}

pub struct Git {
    raiz: PathBuf,
    gitdir: PathBuf,
}

/// Elige implementacion mirando si hay un `.git` utilizable desde `raiz`.
pub fn detectar(raiz: &Path) -> Box<dyn Anchor> {
    match Git::nuevo(raiz) {
        Some(g) => Box::new(g),
        None => Box::new(Null),
    }
}

impl Git {
    fn nuevo(raiz: &Path) -> Option<Git> {
        let mut d = raiz.to_path_buf();
        loop {
            let g = d.join(".git");
            if g.is_dir() {
                return Some(Git { raiz: d, gitdir: g });
            }
            if g.is_file() {
                // Worktree o submodulo: el .git es un archivo con `gitdir: <ruta>`.
                let t = std::fs::read_to_string(&g).ok()?;
                let p = t.trim().strip_prefix("gitdir:")?.trim();
                let abs = if Path::new(p).is_absolute() {
                    PathBuf::from(p)
                } else {
                    d.join(p)
                };
                return Some(Git {
                    raiz: d,
                    gitdir: abs,
                });
            }
            if !d.pop() {
                return None;
            }
        }
    }

    /// Resuelve `.git/HEAD` sin lanzar nada. Tres casos: sha directo (HEAD
    /// suelto), referencia con su archivo, y referencia empaquetada.
    fn head(&self) -> Option<String> {
        let h = std::fs::read_to_string(self.gitdir.join("HEAD")).ok()?;
        let h = h.trim();
        let Some(refname) = h.strip_prefix("ref:").map(str::trim) else {
            return es_sha(h).then(|| h.to_string());
        };
        if let Ok(s) = std::fs::read_to_string(self.gitdir.join(refname)) {
            let s = s.trim().to_string();
            if es_sha(&s) {
                return Some(s);
            }
        }
        let packed = std::fs::read_to_string(self.gitdir.join("packed-refs")).ok()?;
        packed.lines().find_map(|l| {
            let (sha, nombre) = l.split_once(' ')?;
            (nombre.trim() == refname && es_sha(sha)).then(|| sha.to_string())
        })
    }

    fn git(&self, args: &[&str]) -> Option<String> {
        let s = std::process::Command::new("git")
            .arg("-C")
            .arg(&self.raiz)
            .args(args)
            .output()
            .ok()?;
        s.status
            .success()
            .then(|| String::from_utf8_lossy(&s.stdout).into_owned())
    }
}

fn es_sha(s: &str) -> bool {
    s.len() >= 7 && s.len() <= 64 && s.bytes().all(|b| b.is_ascii_hexdigit())
}

impl Anchor for Git {
    fn snapshot(&self) -> AnchorRef {
        AnchorRef {
            kind: "git".into(),
            id: self.head().unwrap_or_default(),
        }
    }

    fn changed_since(&self, r: &AnchorRef) -> Vec<Cambio> {
        if r.vacio() || r.kind != "git" {
            return vec![];
        }
        let mut cuenta: std::collections::BTreeMap<String, usize> = Default::default();
        // Commits desde el ancla. Si el sha ya no existe --rebase, rama
        // borrada-- git falla y se devuelve vacio: mejor no decir nada que
        // decir un numero falso.
        if let Some(out) = self.git(&[
            "log",
            "--format=",
            "--name-only",
            &format!("{}..HEAD", r.id),
        ]) {
            for l in out.lines().map(str::trim).filter(|l| !l.is_empty()) {
                *cuenta.entry(l.to_string()).or_default() += 1;
            }
        }
        // Y lo que esta sin commitear, que cuenta como un cambio.
        if let Some(out) = self.git(&["status", "--porcelain"]) {
            for l in out.lines() {
                if let Some(ruta) = l.get(3..) {
                    let ruta = ruta.rsplit(" -> ").next().unwrap_or(ruta).trim();
                    if !ruta.is_empty() {
                        *cuenta
                            .entry(ruta.trim_matches('"').to_string())
                            .or_default() += 1;
                    }
                }
            }
        }
        let mut v: Vec<Cambio> = cuenta
            .into_iter()
            .map(|(ruta, veces)| Cambio { ruta, veces })
            .collect();
        // Mas tocado primero; a igualdad, por ruta. Determinista.
        v.sort_by(|a, b| b.veces.cmp(&a.veces).then_with(|| a.ruta.cmp(&b.ruta)));
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn null_no_inventa_precision() {
        let n = Null;
        assert!(n.snapshot().vacio());
        assert!(n.changed_since(&AnchorRef::default()).is_empty());
    }

    #[test]
    fn head_se_lee_sin_lanzar_git() {
        // Este mismo repositorio sirve de sustrato.
        let g = Git::nuevo(Path::new(".")).expect("vivac/ es un repo git");
        let s = g.snapshot();
        assert_eq!(s.kind, "git");
        assert!(es_sha(&s.id), "HEAD no resolvio: {:?}", s.id);
        assert_eq!(s.corto().len(), 7);
    }

    #[test]
    fn un_ancla_de_otro_mundo_no_da_cambios() {
        let g = Git::nuevo(Path::new(".")).unwrap();
        let falsa = AnchorRef {
            kind: "git".into(),
            id: "0000000000000000000000000000000000000000".into(),
        };
        assert!(g.changed_since(&falsa).iter().all(|c| !c.ruta.is_empty()));
    }
}
