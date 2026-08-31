//! Las operaciones que escriben. Todas pasan por la guarda de redaccion.
//!
//! La captura se cuelga de las costuras del trabajo, nunca de un juicio de
//! relevancia. Es lo unico que se midio de verdad: en 170 minutos reales,
//! `push`/`pop` --que no se pueden evitar sin dejar el trabajo a medias-- se
//! llamaron nueve veces, y la operacion que preguntaba "¿esto merece
//! guardarse?" se llamo cero, con un protocolo declarado obligatorio.

use crate::args::Args;
use crate::event::{Cuerpo, Estado, Tipo};
use crate::fallo::{Fallo, R};
use crate::model::{plegar, Arbol};
use crate::store::Store;
use crate::{id, redact};

pub struct Ctx {
    pub store: Store,
    pub arbol: Arbol,
}

impl Ctx {
    pub fn cargar(store: Store) -> Result<Ctx, Fallo> {
        let (eventos, rotas) = store.leer()?;
        let arbol = plegar(&eventos, rotas);
        Ok(Ctx { store, arbol })
    }

    /// Escribe y **despues aplica en memoria**, para que lo que se imprima a
    /// continuacion sea el estado de despues de la operacion y no el de antes.
    fn emitir(&mut self, cuerpos: Vec<Cuerpo>) -> R {
        self.store.escribir(cuerpos.clone(), self.arbol.seq)?;
        let ts = crate::clock::now_rfc3339();
        for c in &cuerpos {
            let seq = self.arbol.seq + 1;
            self.arbol.aplicar(seq, &ts, c);
        }
        Ok(())
    }

    fn resolver(&self, s: &str) -> Result<&crate::model::Nodo, Fallo> {
        self.arbol
            .resolver(s)
            .ok_or_else(|| Fallo::uso(format!("No existe el nodo {s}.")))
    }
}

/// Ningun texto llega al log sin pasar por aqui.
fn guardar_texto(campos: &[(&str, &str)]) -> R {
    match redact::revisar_campos(campos) {
        Some(h) => Err(Fallo::Redaccion(Box::new(h))),
        None => Ok(()),
    }
}

fn tipo_de(a: &Args, por_defecto: Tipo) -> Result<Tipo, Fallo> {
    match a.opt("tipo") {
        None => Ok(por_defecto),
        Some(s) => Tipo::desde(s)
            .ok_or_else(|| Fallo::uso(format!("Tipo desconocido: {s}. Son: {}", Tipo::TODOS))),
    }
}

/// Crea un nodo. Devuelve el evento y el numero de alias asignado.
fn nacer(
    ctx: &Ctx,
    titulo: &str,
    por: &str,
    tipo: Tipo,
    padre: Option<String>,
    a: &Args,
) -> Result<(Cuerpo, u64, String), Fallo> {
    let refs = a.lista("ref");
    let governs = a.lista("governs");
    let mut campos: Vec<(&str, &str)> = vec![("titulo", titulo), ("por", por)];
    campos.extend(refs.iter().map(|r| ("ref", r.as_str())));
    campos.extend(governs.iter().map(|g| ("governs", g.as_str())));
    guardar_texto(&campos)?;

    let nodo = id::ulid();
    let num = ctx.arbol.siguiente_num.max(1);
    Ok((
        Cuerpo::NodoCreado {
            nodo: nodo.clone(),
            num,
            tipo,
            titulo: titulo.to_string(),
            por: por.to_string(),
            padre,
            bloquea: a.tiene("bloquea"),
            refs,
            governs,
        },
        num,
        nodo,
    ))
}

/// `push` — abrir un desvio. Es **la** operacion: la arista de procedencia se
/// crea aqui sola, sin que nadie tenga que acordarse de declararla.
pub fn push(ctx: &mut Ctx, a: &Args) -> R {
    let titulo = a
        .libre(0)
        .ok_or_else(|| Fallo::uso("uso: vivac push \"<titulo>\" --por \"<motivo>\""))?;
    let por = a.opt("por").ok_or_else(|| {
        Fallo::uso(
            "Falta --por. Un desvio sin motivo es exactamente el fallo que\n  \
             esto existe para atacar: dentro de un mes nadie sabra por que.",
        )
    })?;

    let padre = ctx.arbol.foco().map(|n| n.id.clone());
    let tipo = tipo_de(
        a,
        if padre.is_none() {
            Tipo::Goal
        } else {
            Tipo::Task
        },
    )?;
    let (ev, num, nodo) = nacer(ctx, titulo, por, tipo, padre, a)?;
    ctx.emitir(vec![ev, Cuerpo::Apilado { nodo }])?;

    let hondo = ctx.arbol.profundidad_pila() + 1;
    println!("  {}{}  {}", tipo.prefijo(), num, titulo);
    if a.tiene("bloquea") {
        println!("        bloquea el cierre de su padre");
    }
    // §6.1: intervencion, nunca bloqueo. Una pila honda casi nunca es
    // indisciplina: es que el objetivo raiz cambio y nadie volvio a enraizar.
    if hondo >= 4 {
        if let Some(raiz) = ctx.arbol.raices().first() {
            println!();
            println!(
                "  Estas a {hondo} niveles de {} \"{}\".",
                raiz.alias(),
                raiz.titulo
            );
            println!("  ¿Sigue siendo un desvio, o el objetivo real cambio?");
            println!("  Si cambio:  vivac promote");
        }
    }
    Ok(())
}

/// `pop` — cerrar el foco y volver al padre con contexto.
pub fn pop(ctx: &mut Ctx, a: &Args) -> R {
    let foco = ctx
        .arbol
        .foco()
        .ok_or_else(|| {
            Fallo::uso(
                "La pila esta vacia. Abrir algo:  vivac push \"<titulo>\" --por \"<motivo>\"",
            )
        })?
        .clone();
    let resultado = a.libre(0).unwrap_or("");
    guardar_texto(&[("resultado", resultado)])?;
    cerrar(ctx, &foco, resultado, a.tiene("forzar"), true)?;
    match ctx.arbol.nodo(foco.padre.as_deref().unwrap_or("")) {
        Some(p) => {
            let r = ctx.arbol.recuento(&p.id);
            println!("  volves a {}  {}", p.alias(), p.titulo);
            let f = r.frase();
            if !f.is_empty() {
                println!("        ({f} por debajo)");
            }
        }
        None => println!("  pila vacia"),
    }
    Ok(())
}

/// `park` — lo que produce NO TOCAR AHORA, y sin ello esa seccion sale
/// siempre vacia. No lo frena la regla de cierre: aparcar no afirma que algo
/// termino, y si aparcar costara mas que ignorar, nadie aparcaria.
pub fn park(ctx: &mut Ctx, a: &Args) -> R {
    let (nodo, motivo) = match a.libre(0).and_then(|s| ctx.arbol.resolver(s)) {
        Some(n) => (n.clone(), a.libre(1).unwrap_or("")),
        None => {
            let f = ctx
                .arbol
                .foco()
                .ok_or_else(|| Fallo::uso("uso: vivac park [<id>] [\"<motivo>\"]"))?
                .clone();
            (f, a.libre(0).unwrap_or(""))
        }
    };
    guardar_texto(&[("motivo", motivo)])?;
    let mut evs = vec![Cuerpo::EstadoCambiado {
        nodo: nodo.id.clone(),
        estado: Estado::Suspended,
        resultado: motivo.to_string(),
        forzado: false,
    }];
    if ctx.arbol.pila.contains(&nodo.id) {
        evs.push(Cuerpo::Desapilado {
            nodo: nodo.id.clone(),
        });
    }
    ctx.emitir(evs)?;
    println!("  {}  {}  -> aparcado", nodo.alias(), nodo.titulo);
    println!("        aparece en:  vivac parked");
    Ok(())
}

/// La regla de cierre. `MODEL.md` §7, y es la **unica** regla del modelo que
/// rechaza una operacion del usuario.
///
/// Se gana ese privilegio porque el caso que previene esta medido: una
/// auditoria marcada DONE con sus hallazgos abiertos tardo 26 dias en
/// detectarse. Sin esto, el modelo deja cometer el mismo error otra vez.
fn cerrar(
    ctx: &mut Ctx,
    n: &crate::model::Nodo,
    resultado: &str,
    forzar: bool,
    desapilar: bool,
) -> R {
    if !forzar {
        let pend = ctx.arbol.bloqueantes_abiertos(&n.id);
        if !pend.is_empty() {
            let mut m = format!(
                "  {} NO puede cerrar: {} condicion(es) de cierre abierta(s)\n",
                n.alias(),
                pend.len()
            );
            for c in &pend {
                m.push_str(&format!("\n      {:<6} {}", c.alias(), c.titulo));
            }
            m.push_str(&format!(
                "\n\n  Una corrida cierra con sus hallazgos, no con su informe.\n  \
                 Cerrarlo igual deja rastro:  vivac done {} --forzar",
                n.num
            ));
            return Err(Fallo::Modelo(m));
        }
    }
    let mut evs = vec![Cuerpo::EstadoCambiado {
        nodo: n.id.clone(),
        estado: Estado::Done,
        resultado: resultado.to_string(),
        forzado: forzar,
    }];
    if desapilar && ctx.arbol.pila.contains(&n.id) {
        evs.push(Cuerpo::Desapilado { nodo: n.id.clone() });
    }
    ctx.emitir(evs)?;
    println!(
        "  {}  {}  -> {}",
        n.alias(),
        n.titulo,
        if forzar {
            "cerrado A LA FUERZA"
        } else {
            "cerrado"
        }
    );
    if forzar {
        println!("        queda registrado como cierre falso en todo render");
    }
    Ok(())
}

pub fn done(ctx: &mut Ctx, a: &Args) -> R {
    let s = a
        .libre(0)
        .ok_or_else(|| Fallo::uso("uso: vivac done <id> [\"<resultado>\"] [--forzar]"))?;
    let n = ctx.resolver(s)?.clone();
    let resultado = a.libre(1).unwrap_or("");
    guardar_texto(&[("resultado", resultado)])?;
    cerrar(ctx, &n, resultado, a.tiene("forzar"), true)
}

/// `add` — un nodo sin tocar la pila. Es como entra un arbol que ya existia
/// en otra parte, y como se cuelga un hallazgo de algo que no es el foco.
pub fn add(ctx: &mut Ctx, a: &Args) -> R {
    let titulo = a.libre(0).ok_or_else(|| {
        Fallo::uso("uso: vivac add \"<titulo>\" [--padre N] [--por \"<motivo>\"]")
    })?;
    let padre = match a.opt("padre") {
        Some(p) => Some(ctx.resolver(p)?.id.clone()),
        None => ctx.arbol.foco().map(|n| n.id.clone()),
    };
    let tipo = tipo_de(
        a,
        if padre.is_none() {
            Tipo::Goal
        } else {
            Tipo::Task
        },
    )?;
    let (ev, num, _) = nacer(ctx, titulo, &a.opt_o("por"), tipo, padre.clone(), a)?;
    ctx.emitir(vec![ev])?;
    let donde = match padre.and_then(|p| ctx.arbol.nodo(&p).map(|n| n.alias())) {
        Some(al) => format!(" bajo {al}"),
        None => " (raiz)".into(),
    };
    println!("  {}{}  {}{}", tipo.prefijo(), num, titulo, donde);
    if a.tiene("bloquea") {
        println!("        bloquea el cierre de su padre");
    }
    Ok(())
}

pub fn note(ctx: &mut Ctx, a: &Args) -> R {
    let (n, nota) = match (a.libre(0), a.libre(1)) {
        (Some(s), Some(t)) => (ctx.resolver(s)?.clone(), t),
        (Some(t), None) => {
            let f = ctx
                .arbol
                .foco()
                .ok_or_else(|| Fallo::uso("uso: vivac note [<id>] \"<nota>\""))?;
            (f.clone(), t)
        }
        _ => return Err(Fallo::uso("uso: vivac note [<id>] \"<nota>\"")),
    };
    guardar_texto(&[("nota", nota)])?;
    ctx.emitir(vec![Cuerpo::NodoAnotado {
        nodo: n.id.clone(),
        nota: nota.to_string(),
    }])?;
    println!("  {} anotado", n.alias());
    Ok(())
}

pub fn block(ctx: &mut Ctx, a: &Args) -> R {
    let s = a
        .libre(0)
        .ok_or_else(|| Fallo::uso("uso: vivac block <id> [--off]"))?;
    let n = ctx.resolver(s)?.clone();
    let Some(padre) = n.padre.as_ref().and_then(|p| ctx.arbol.nodo(p)) else {
        return Err(Fallo::uso(format!(
            "{} es raiz: no hay padre al que bloquear.",
            n.alias()
        )));
    };
    let bloquea = !a.tiene("off");
    let (pa, pt) = (padre.alias(), padre.titulo.clone());
    ctx.emitir(vec![Cuerpo::BloqueoCambiado {
        nodo: n.id.clone(),
        bloquea,
    }])?;
    let verbo = if bloquea { "bloquea" } else { "ya no bloquea" };
    println!("  {} {} el cierre de {pa}  {pt}", n.alias(), verbo);
    Ok(())
}

/// `promote` — el foco pasa a ser meta propia y la pila se corta ahi.
///
/// La cadena de procedencia **se conserva**: de donde nacio no cambia porque
/// haya cambiado de rango. Sin esta operacion, la advertencia de profundidad
/// no tiene salida y se acaba ignorando.
pub fn promote(ctx: &mut Ctx, a: &Args) -> R {
    let n = match a.libre(0) {
        Some(s) => ctx.resolver(s)?.clone(),
        None => ctx
            .arbol
            .foco()
            .ok_or_else(|| Fallo::uso("uso: vivac promote [<id>]"))?
            .clone(),
    };
    ctx.emitir(vec![Cuerpo::Promovido { nodo: n.id.clone() }])?;
    println!("  {}  {}  -> meta propia", n.alias(), n.titulo);
    if let Some(p) = n.padre.as_ref().and_then(|p| ctx.arbol.nodo(p)) {
        println!("        sigue naciendo de {}  {}", p.alias(), p.titulo);
    }
    Ok(())
}

/// `abandon` — descartar. Cuesta lo mismo que `pop` a proposito: si abandonar
/// fuera mas caro que ignorar, nadie abandonaria y a los tres meses el arbol
/// seria ruido.
///
/// La cascada **no** es el defecto. `MODEL.md` §6 la quiere con confirmacion y
/// lista delante, y una CLI no interactiva no puede confirmar nada: lo que
/// hace es enseñar lo que caeria y pedir `--cascada` explicito.
pub fn abandon(ctx: &mut Ctx, a: &Args) -> R {
    let n = match a.libre(0).and_then(|s| ctx.arbol.resolver(s)) {
        Some(n) => n.clone(),
        None => ctx
            .arbol
            .foco()
            .ok_or_else(|| Fallo::uso("uso: vivac abandon [<id>] \"<motivo>\""))?
            .clone(),
    };
    let motivo = a
        .libres
        .iter()
        .rev()
        .find(|s| ctx.arbol.resolver(s).is_none());
    let motivo = motivo.map(|s| s.as_str()).unwrap_or("");
    guardar_texto(&[("motivo", motivo)])?;

    let vivos: Vec<_> = ctx
        .arbol
        .descendientes(&n.id)
        .into_iter()
        .filter(|d| d.estado.abierto())
        .collect();
    if !vivos.is_empty() && !a.tiene("cascada") {
        let mut m = format!(
            "  {} tiene {} descendiente(s) abierto(s):\n",
            n.alias(),
            vivos.len()
        );
        for d in &vivos {
            m.push_str(&format!("\n      {:<6} {}", d.alias(), d.titulo));
        }
        m.push_str(&format!(
            "\n\n  Abandonarlo todo:        vivac abandon {} --cascada\n  \
             Salvar algo primero:     vivac promote <id>",
            n.num
        ));
        return Err(Fallo::Modelo(m));
    }

    let mut evs = vec![Cuerpo::EstadoCambiado {
        nodo: n.id.clone(),
        estado: Estado::Abandoned,
        resultado: motivo.to_string(),
        forzado: false,
    }];
    if ctx.arbol.pila.contains(&n.id) {
        evs.push(Cuerpo::Desapilado { nodo: n.id.clone() });
    }
    let caen = vivos.len();
    for d in vivos {
        evs.push(Cuerpo::EstadoCambiado {
            nodo: d.id.clone(),
            estado: Estado::Abandoned,
            resultado: format!("en cascada desde {}", n.alias()),
            forzado: false,
        });
        if ctx.arbol.pila.contains(&d.id) {
            evs.push(Cuerpo::Desapilado { nodo: d.id.clone() });
        }
    }
    ctx.emitir(evs)?;
    println!("  {}  {}  -> abandonado", n.alias(), n.titulo);
    if caen > 0 {
        println!("        y {caen} descendiente(s) con el");
    }
    Ok(())
}

/// `focus` — volver a entrar en un nodo que ya existe.
///
/// Sin esto la pila solo sirve dentro de una sesion: al dia siguiente el log
/// tiene el arbol entero y la pila vacia, y no hay forma de decir "estoy en
/// esto" sin abrir un nodo nuevo, que es justo la basura que se quiere evitar.
/// La pila pasa a ser el camino desde la raiz hasta el nodo, que es lo que
/// significa estar trabajando en el.
pub fn focus(ctx: &mut Ctx, a: &Args) -> R {
    let s = a
        .libre(0)
        .ok_or_else(|| Fallo::uso("uso: vivac focus <id> [--reabrir]"))?;
    let n = ctx.resolver(s)?.clone();

    if !n.estado.abierto() && !a.tiene("reabrir") {
        // Aparcar es "quiza vuelva", asi que volver es la operacion normal y
        // no pide permiso. Cerrar afirma que algo termino: deshacerlo tiene
        // que ser deliberado.
        if n.estado != Estado::Suspended {
            return Err(Fallo::Modelo(format!(
                "  {} esta {}. Volver a el deshace esa afirmacion.\n\n  \
                 Si de verdad no estaba terminado:  vivac focus {} --reabrir",
                n.alias(),
                n.estado.palabra(n.tipo),
                n.num
            )));
        }
    }

    let camino: Vec<String> = ctx
        .arbol
        .ancestros(&n.id)
        .iter()
        .map(|p| p.id.clone())
        .collect();
    let mut evs: Vec<Cuerpo> = ctx
        .arbol
        .pila
        .iter()
        .filter(|id| !camino.contains(id))
        .map(|id| Cuerpo::Desapilado { nodo: id.clone() })
        .collect();
    if !n.estado.abierto() {
        evs.push(Cuerpo::EstadoCambiado {
            nodo: n.id.clone(),
            estado: Estado::Active,
            resultado: String::new(),
            forzado: false,
        });
    }
    for id in &camino {
        if !ctx.arbol.pila.contains(id) {
            evs.push(Cuerpo::Apilado { nodo: id.clone() });
        }
    }
    let revivido = !n.estado.abierto();
    ctx.emitir(evs)?;
    if revivido {
        println!("  {} vuelve a estar abierto", n.alias());
    }
    crate::render::stack(&ctx.arbol, a)
}
