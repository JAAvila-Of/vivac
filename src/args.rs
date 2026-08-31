//! Analisis de argumentos a mano.
//!
//! No entra `clap`. El pilar de seguridad quiere pocas dependencias que
//! auditar y el de rendimiento paga el arranque del proceso en cada llamada
//! --no hay demonio-- asi que la superficie es esta: posicionales, `--clave
//! valor` y banderas. Cabe en cuarenta lineas y el texto de ayuda se escribe
//! en castellano sin pelearse con nadie.

use std::collections::HashMap;

#[derive(Debug, Default)]
pub struct Args {
    pub libres: Vec<String>,
    opts: HashMap<String, Vec<String>>,
}

impl Args {
    pub fn parse<I: IntoIterator<Item = String>>(it: I) -> Args {
        let v: Vec<String> = it.into_iter().collect();
        let mut a = Args::default();
        let mut i = 0;
        while i < v.len() {
            if let Some(k) = v[i].strip_prefix("--") {
                let (k, inline) = match k.split_once('=') {
                    Some((k, val)) => (k, Some(val.to_string())),
                    None => (k, None),
                };
                let val = inline.or_else(|| {
                    v.get(i + 1).filter(|n| !n.starts_with("--")).map(|n| {
                        i += 1;
                        n.clone()
                    })
                });
                a.opts.entry(k.to_string()).or_default().extend(val);
            } else {
                a.libres.push(v[i].clone());
            }
            i += 1;
        }
        a
    }

    pub fn tiene(&self, k: &str) -> bool {
        self.opts.contains_key(k)
    }

    pub fn opt(&self, k: &str) -> Option<&str> {
        self.opts.get(k).and_then(|v| v.last()).map(|s| s.as_str())
    }

    pub fn opt_o(&self, k: &str) -> String {
        self.opt(k).unwrap_or_default().to_string()
    }

    /// Repetible: `--ref a --ref b`.
    pub fn lista(&self, k: &str) -> Vec<String> {
        self.opts.get(k).cloned().unwrap_or_default()
    }

    pub fn libre(&self, i: usize) -> Option<&str> {
        self.libres.get(i).map(|s| s.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> Args {
        Args::parse(s.split_whitespace().map(String::from))
    }

    #[test]
    fn posicionales_y_opciones() {
        let a = p("titulo --por motivo --bloquea --ref uno --ref dos");
        assert_eq!(a.libre(0), Some("titulo"));
        assert_eq!(a.opt("por"), Some("motivo"));
        assert!(a.tiene("bloquea"));
        assert_eq!(a.lista("ref"), vec!["uno", "dos"]);
    }

    #[test]
    fn bandera_pegada_a_otra_bandera() {
        // `--bloquea --por x`: `--bloquea` no se come el `--por`.
        let a = p("--bloquea --por x");
        assert!(a.tiene("bloquea"));
        assert_eq!(a.opt("bloquea"), None);
        assert_eq!(a.opt("por"), Some("x"));
    }

    #[test]
    fn igual() {
        let a = p("--tipo=decision");
        assert_eq!(a.opt("tipo"), Some("decision"));
    }
}
