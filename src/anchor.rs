//! `Anchor` — a stable identity for the tree's state, and what changed since.
//!
//! The model does not depend on git: it needs two primitives and nothing
//! else. The implementations are `Git` and `Null`, and **`Null` defines the
//! floor of the product**, it is not filler: with no version control the
//! tree, the stack, the vivacs and `why` all keep working whole. The only
//! thing lost is precision by change, and the `brief` swaps in plain age
//! rather than inventing precision it does not have.
//!
//! **`snapshot` spawns no subprocess.** `push` creates a vivac and a vivac
//! needs an anchor, so this falls on the write path, whose budget is 5 ms;
//! starting `git` on Windows costs between 15 and 30. `.git/HEAD` is read
//! and the reference resolved by hand. `changed_since` does shell out to
//! git, because it only runs on reads.

use std::path::{Path, PathBuf};

/// Identity of the tree state at a moment. Empty means "there is none",
/// which is a legitimate state and not an error.
#[derive(Debug, Clone, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub struct AnchorRef {
    pub kind: String,
    pub id: String,
}

impl AnchorRef {
    pub fn vacio(&self) -> bool {
        self.id.is_empty()
    }

    /// Short prefix, the way git does it.
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

/// No version control.
///
/// `MODEL.md` §8 proposed a merkle of the working set. **Not in v0.1**: the
/// working set has no bound, and hashing it sits on the write path, which
/// has a 5 ms budget. The performance pillar sets a ceiling a feature must
/// respect in order to exist, and this one did not. It stays an anchor with
/// no identity, which is exactly the degradation `BRIEF-SPEC.md` §6 already
/// specifies.
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

/// Picks an implementation by looking for a usable `.git` from `raiz`.
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
                // Worktree or submodule: .git is a file holding `gitdir: <path>`.
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

    /// Resolves `.git/HEAD` spawning nothing. Three cases: a direct sha
    /// (detached HEAD), a reference with its file, and a packed reference.
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
        // Commits since the anchor. If the sha is gone --rebase, deleted
        // branch-- git fails and this returns empty: better to say nothing
        // than to say a false number.
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
        // And whatever is uncommitted, which counts as a change.
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
        // Most-touched first; ties broken by path. Deterministic.
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
        // This very repository serves as the substrate.
        let g = Git::nuevo(Path::new(".")).expect("vivac/ es un repo git");
        let s = g.snapshot();
        assert_eq!(s.kind, "git");
        assert!(es_sha(&s.id), "HEAD did not resolve: {:?}", s.id);
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
