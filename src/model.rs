//! El pliegue: de la lista de eventos al arbol.
//!
//! El indice de hijos se construye **aqui**, durante el pliegue, y no se busca
//! recorriendo todos los nodos en cada consulta. Con el spike en Python daba
//! igual; con el presupuesto del pilar de rendimiento --`why` y `tree` sobre
//! diez mil nodos por debajo de 50 ms-- un `hijos()` lineal convierte un
//! render en cuadratico. Los indices se piensan desde el modelo, no se
//! agregan cuando duele.

use crate::event::{Cuerpo, Estado, Evento, Tipo};
use std::collections::HashMap;

#[derive(Debug, Clone)]
pub struct Nodo {
    pub id: String,
    pub num: u64,
    pub tipo: Tipo,
    pub titulo: String,
    /// Por que nacio. `push` lo exige: un desvio sin motivo es el fallo que
    /// este proyecto ataca.
    pub por: String,
    pub estado: Estado,
    pub padre: Option<String>,
    /// Condicion de cierre del padre. Explicita, y por defecto **no** bloquea:
    /// forzarlo deja padres que no cierran nunca. `MODEL.md` §5.
    pub bloquea: bool,
    pub nota: String,
    pub resultado: String,
    pub refs: Vec<String>,
    pub governs: Vec<String>,
    pub abierto: String,
    pub cerrado: Option<String>,
    pub cierre_forzado: bool,
}

impl Nodo {
    pub fn alias(&self) -> String {
        format!("{}{}", self.tipo.prefijo(), self.num)
    }
}

#[derive(Debug, Default, Clone, Copy)]
pub struct Recuento {
    pub total: usize,
    pub abiertos: usize,
    pub cerrados: usize,
    pub aparcados: usize,
}

impl Recuento {
    pub fn frase(&self) -> String {
        let mut p = Vec::new();
        if self.abiertos > 0 {
            p.push(format!("{} abiertos", self.abiertos));
        }
        if self.cerrados > 0 {
            p.push(format!("{} cerrados", self.cerrados));
        }
        if self.aparcados > 0 {
            p.push(format!("{} aparcados", self.aparcados));
        }
        p.join(" / ")
    }
}

#[derive(Debug, Default)]
pub struct Arbol {
    nodos: HashMap<String, Nodo>,
    hijos: HashMap<String, Vec<String>>,
    por_num: HashMap<u64, String>,
    pub raices: Vec<String>,
    pub pila: Vec<String>,
    pub seq: u64,
    pub siguiente_num: u64,
    pub lineas_rotas: usize,
}

pub fn plegar(eventos: &[Evento], rotas: usize) -> Arbol {
    let mut a = Arbol {
        lineas_rotas: rotas,
        ..Default::default()
    };
    for e in eventos {
        a.aplicar(e.seq, &e.ts, &e.payload);
    }
    a.ordenar();
    a
}

impl Arbol {
    /// Aplica un evento.
    ///
    /// Lo usa el pliegue al arrancar y tambien `emitir` justo despues de
    /// escribir. Si el arbol en memoria no siguiera al log, cada operacion
    /// imprimiria el recuento de **antes** de hacerla --"vuelves al padre, 1
    /// abierto por debajo" del nodo que acabas de cerrar-- que es la clase de
    /// mentira pequeña que hace que despues no te fies del resto.
    pub fn aplicar(&mut self, seq: u64, ts: &str, cuerpo: &Cuerpo) {
        self.seq = self.seq.max(seq);
        match cuerpo {
            Cuerpo::NodoCreado {
                nodo,
                num,
                tipo,
                titulo,
                por,
                padre,
                bloquea,
                refs,
                governs,
            } => {
                if self.nodos.contains_key(nodo) {
                    // Creacion repetida: conmutativa, gana la primera.
                    return;
                }
                self.nodos.insert(
                    nodo.clone(),
                    Nodo {
                        id: nodo.clone(),
                        num: *num,
                        tipo: *tipo,
                        titulo: titulo.clone(),
                        por: por.clone(),
                        estado: Estado::Active,
                        padre: padre.clone(),
                        bloquea: *bloquea,
                        nota: String::new(),
                        resultado: String::new(),
                        refs: refs.clone(),
                        governs: governs.clone(),
                        abierto: crate::clock::date_of(ts).to_string(),
                        cerrado: None,
                        cierre_forzado: false,
                    },
                );
                self.por_num.insert(*num, nodo.clone());
                self.siguiente_num = self.siguiente_num.max(*num + 1);
                match padre {
                    Some(p) => self.hijos.entry(p.clone()).or_default().push(nodo.clone()),
                    None => self.raices.push(nodo.clone()),
                }
            }
            Cuerpo::EstadoCambiado {
                nodo,
                estado,
                resultado,
                forzado,
            } => {
                if let Some(n) = self.nodos.get_mut(nodo) {
                    n.estado = *estado;
                    if !resultado.is_empty() {
                        n.resultado = resultado.clone();
                    }
                    n.cierre_forzado = *forzado;
                    n.cerrado = if estado.abierto() {
                        None
                    } else {
                        Some(crate::clock::date_of(ts).to_string())
                    };
                }
            }
            Cuerpo::NodoAnotado { nodo, nota } => {
                if let Some(n) = self.nodos.get_mut(nodo) {
                    n.nota = nota.clone();
                }
            }
            Cuerpo::BloqueoCambiado { nodo, bloquea } => {
                if let Some(n) = self.nodos.get_mut(nodo) {
                    n.bloquea = *bloquea;
                }
            }
            Cuerpo::Apilado { nodo } => {
                if !self.pila.contains(nodo) {
                    self.pila.push(nodo.clone());
                }
            }
            Cuerpo::Desapilado { nodo } => {
                self.pila.retain(|x| x != nodo);
            }
            Cuerpo::Promovido { nodo } => {
                if let Some(n) = self.nodos.get_mut(nodo) {
                    n.tipo = Tipo::Goal;
                }
                // La pila se corta en el promovido: pasa a ser raiz de la
                // suya. La cadena de procedencia no se toca: de donde nacio
                // no cambia porque haya cambiado de rango.
                if let Some(i) = self.pila.iter().position(|x| x == nodo) {
                    self.pila.drain(..i);
                }
            }
        }
    }

    /// Orden estable por numero: dos renders del mismo log son identicos.
    ///
    /// Solo hace falta al plegar. En caliente los nodos nacen con numero
    /// creciente, asi que añadir al final ya deja el orden bueno.
    pub fn ordenar(&mut self) {
        let nums: std::collections::HashMap<String, u64> =
            self.nodos.iter().map(|(k, n)| (k.clone(), n.num)).collect();
        for v in self.hijos.values_mut() {
            v.sort_by_key(|id| nums.get(id).copied().unwrap_or(0));
        }
        self.raices
            .sort_by_key(|id| nums.get(id).copied().unwrap_or(0));
    }
}

impl Arbol {
    pub fn vacio(&self) -> bool {
        self.nodos.is_empty()
    }

    pub fn total(&self) -> usize {
        self.nodos.len()
    }

    pub fn nodo(&self, id: &str) -> Option<&Nodo> {
        self.nodos.get(id)
    }

    pub fn todos(&self) -> impl Iterator<Item = &Nodo> {
        self.nodos.values()
    }

    /// Resuelve lo que escriba el usuario: `7`, `t7` o el ULID entero.
    /// Solo con el numero funciona a proposito --`vivac why 7`-- porque
    /// obligar a recordar el prefijo es coste de captura sin nada a cambio.
    pub fn resolver(&self, s: &str) -> Option<&Nodo> {
        let limpio = s.trim().trim_start_matches('#');
        if let Ok(n) = limpio.parse::<u64>() {
            return self.por_num.get(&n).and_then(|id| self.nodos.get(id));
        }
        let sin_prefijo = &limpio[1..];
        if limpio.len() > 1 && sin_prefijo.chars().all(|c| c.is_ascii_digit()) {
            if let Ok(n) = sin_prefijo.parse::<u64>() {
                return self
                    .por_num
                    .get(&n)
                    .and_then(|id| self.nodos.get(id))
                    .filter(|nd| nd.tipo.prefijo() == limpio.chars().next().unwrap());
            }
        }
        self.nodos.get(limpio)
    }

    pub fn hijos(&self, id: &str) -> Vec<&Nodo> {
        self.hijos
            .get(id)
            .map(|v| v.iter().filter_map(|i| self.nodos.get(i)).collect())
            .unwrap_or_default()
    }

    pub fn raices(&self) -> Vec<&Nodo> {
        self.raices
            .iter()
            .filter_map(|i| self.nodos.get(i))
            .collect()
    }

    /// Del nodo a la raiz, invertido: raiz primero. Es el camino de `why`.
    /// El `visto` no es paranoia: un log manipulado a mano puede tener un
    /// ciclo, y colgarse seria peor que dar un camino corto.
    pub fn ancestros(&self, id: &str) -> Vec<&Nodo> {
        let mut camino = Vec::new();
        let mut visto = std::collections::HashSet::new();
        let mut cur = self.nodos.get(id);
        while let Some(n) = cur {
            if !visto.insert(&n.id) {
                break;
            }
            camino.push(n);
            cur = n.padre.as_deref().and_then(|p| self.nodos.get(p));
        }
        camino.reverse();
        camino
    }

    pub fn descendientes(&self, id: &str) -> Vec<&Nodo> {
        let mut out = Vec::new();
        let mut pila = vec![id.to_string()];
        let mut visto = std::collections::HashSet::new();
        while let Some(cur) = pila.pop() {
            for h in self.hijos.get(&cur).map(|v| v.as_slice()).unwrap_or(&[]) {
                if visto.insert(h.clone()) {
                    if let Some(n) = self.nodos.get(h) {
                        out.push(n);
                    }
                    pila.push(h.clone());
                }
            }
        }
        out.sort_by_key(|n| n.num);
        out
    }

    /// Descendientes abiertos marcados como condicion de cierre.
    ///
    /// **Transitivo a proposito**: un nieto bloqueante bloquea al abuelo. Sin
    /// eso basta interponer un nodo intermedio para saltarse la guarda sin
    /// querer, que es exactamente como se cuela un cierre falso.
    pub fn bloqueantes_abiertos(&self, id: &str) -> Vec<&Nodo> {
        self.descendientes(id)
            .into_iter()
            .filter(|n| n.bloquea && n.estado.abierto())
            .collect()
    }

    pub fn recuento(&self, id: &str) -> Recuento {
        let d = self.descendientes(id);
        Recuento {
            total: d.len(),
            abiertos: d.iter().filter(|n| n.estado == Estado::Active).count(),
            cerrados: d.iter().filter(|n| n.estado == Estado::Done).count(),
            aparcados: d.iter().filter(|n| n.estado == Estado::Suspended).count(),
        }
    }

    pub fn foco(&self) -> Option<&Nodo> {
        self.pila.last().and_then(|id| self.nodos.get(id))
    }

    pub fn profundidad_pila(&self) -> usize {
        self.pila.len()
    }
}

/// Recuentos de subarbol para todos los nodos, calculados de una vez.
///
/// Pedirle el recuento a cada nodo por separado recorre su subarbol entero, y
/// hacerlo para todo el arbol lo vuelve cuadratico: medido, `tree` sobre diez
/// mil nodos pasaba de 79 ms en un subarbol a 242 ms en el arbol completo, y
/// los 163 ms de diferencia eran esto y no el log.
///
/// Una sola pasada en post-orden deja lo mismo en tiempo lineal. Es la clase
/// de indice que el pilar de rendimiento manda pensar desde el modelo en vez
/// de agregar cuando duele.
#[derive(Debug, Default)]
pub struct Agregados {
    recuento: HashMap<String, Recuento>,
    bloqueantes: HashMap<String, usize>,
    pub profundidad_max: usize,
}

impl Agregados {
    pub fn recuento(&self, id: &str) -> Recuento {
        self.recuento.get(id).copied().unwrap_or_default()
    }

    pub fn bloqueantes(&self, id: &str) -> usize {
        self.bloqueantes.get(id).copied().unwrap_or(0)
    }
}

impl Arbol {
    pub fn agregados(&self) -> Agregados {
        let mut ag = Agregados::default();

        // Los huerfanos no cuelgan de ninguna raiz. Se recorren igual: un
        // arbol roto tiene que poder mirarse, que para eso esta `check`.
        let mut entradas: Vec<&String> = self.raices.iter().collect();
        entradas.extend(
            self.nodos
                .values()
                .filter(|n| {
                    n.padre
                        .as_ref()
                        .is_some_and(|p| !self.nodos.contains_key(p))
                })
                .map(|n| &n.id),
        );

        let mut orden: Vec<(&String, usize)> = Vec::with_capacity(self.nodos.len());
        let mut pila: Vec<(&String, usize)> = entradas.into_iter().map(|id| (id, 1)).collect();
        let mut visto = std::collections::HashSet::new();
        while let Some((id, hondo)) = pila.pop() {
            if !visto.insert(id) {
                continue;
            }
            ag.profundidad_max = ag.profundidad_max.max(hondo);
            orden.push((id, hondo));
            if let Some(hs) = self.hijos.get(id) {
                pila.extend(hs.iter().map(|h| (h, hondo + 1)));
            }
        }

        // De las hojas hacia arriba: cada padre suma lo de sus hijos mas los
        // hijos mismos.
        for (id, _) in orden.iter().rev() {
            let mut r = Recuento::default();
            let mut b = 0usize;
            for h in self.hijos.get(*id).map(|v| v.as_slice()).unwrap_or(&[]) {
                let Some(hijo) = self.nodos.get(h) else {
                    continue;
                };
                let hr = ag.recuento(h);
                r.total += hr.total + 1;
                r.abiertos += hr.abiertos + usize::from(hijo.estado == Estado::Active);
                r.cerrados += hr.cerrados + usize::from(hijo.estado == Estado::Done);
                r.aparcados += hr.aparcados + usize::from(hijo.estado == Estado::Suspended);
                b += ag.bloqueantes(h) + usize::from(hijo.bloquea && hijo.estado == Estado::Active);
            }
            ag.recuento.insert((*id).clone(), r);
            ag.bloqueantes.insert((*id).clone(), b);
        }
        ag
    }
}
