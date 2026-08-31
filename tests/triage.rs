//! `triage` — la vista de poda.
//!
//! `BRIEF-SPEC.md` §4 la nombra: un brief que se pasa de presupuesto no debe
//! mentir por omision, la senal es que el grafo necesita poda. `MODEL.md`
//! §6.1 le manda los nodos hondos. Y `d33` le manda los rescatados, que se
//! quedan colgando de un descartado a proposito.

mod common;
use common::Caja;

fn seccion(s: &str, titulo: &str) -> bool {
    s.lines().any(|l| l.trim_start().starts_with(titulo))
}

/// Un arbol sano no tiene nada que podar, y lo dice sin secciones vacias.
#[test]
fn nada_que_podar() {
    let c = Caja::nueva("sano");
    c.ok(&["push", "Una meta", "--por", "hace falta"]);
    let s = c.ok(&["triage"]);
    assert!(s.contains("Nada que podar"), "{s}");
    for t in ["APARCADOS", "CIERRES FALSOS", "SOBREVIVIERON"] {
        assert!(!s.contains(t), "emitio {t} vacia:\n{s}");
    }
}

/// Un aparcado sale con el motivo por el que se aparco. Sin el no se puede
/// decidir nada, que es para lo que existe la vista.
#[test]
fn los_aparcados_salen_con_su_motivo() {
    let c = Caja::nueva("aparcados");
    c.ok(&["push", "Una meta", "--por", "hace falta"]);
    c.ok(&["push", "Un desvio", "--por", "salio al pasar"]);
    c.ok(&["park", "faltaba decidir el backend antes"]);
    let s = c.ok(&["triage"]);
    assert!(seccion(&s, "APARCADOS"), "{s}");
    assert!(s.contains("Un desvio"), "{s}");
    assert!(s.contains("faltaba decidir el backend"), "sin motivo:\n{s}");
    assert!(s.contains("focus <id>"), "sin la accion concreta:\n{s}");
}

/// El cierre que se vuelve falso **despues**, al colgarle un bloqueante a algo
/// ya cerrado. Es el caso medido que tardo 26 dias en detectarse, y el unico
/// que la regla de cierre no puede prevenir: cuando `done` corrio, el hallazgo
/// todavia no existia.
#[test]
fn un_cierre_falso_sale_con_su_cuenta() {
    let c = Caja::nueva("falso");
    c.ok(&["push", "Auditoria de permisos", "--por", "toca revisarla"]);
    c.ok(&["pop", "informe entregado"]);
    c.ok(&[
        "add",
        "Hallazgo sin arreglar",
        "--padre",
        "1",
        "--bloquea",
        "--por",
        "salio de la auditoria, tarde",
    ]);
    let s = c.ok(&["triage"]);
    assert!(seccion(&s, "CIERRES FALSOS"), "{s}");
    assert!(s.contains("Auditoria de permisos"), "{s}");
    assert!(s.contains("1 bloqueante"), "no dijo cuantos faltan: {s}");
}

/// Un cierre **forzado** no vuelve aqui. Fue una decision, deja rastro y el
/// arbol lo marca; repetirlo cada dia seria pedir que se vuelva a decidir lo
/// ya decidido. Es la misma exencion que hace `check`.
#[test]
fn un_cierre_forzado_no_vuelve_al_triage() {
    let c = Caja::nueva("forzado");
    c.ok(&["push", "Auditoria de permisos", "--por", "toca revisarla"]);
    c.ok(&[
        "add",
        "Hallazgo sin arreglar",
        "--padre",
        "1",
        "--bloquea",
        "--por",
        "salio de la auditoria",
    ]);
    c.ok(&[
        "done",
        "1",
        "se cierra igual, el hallazgo va aparte",
        "--forzar",
    ]);
    let s = c.ok(&["triage"]);
    assert!(
        !s.contains("CIERRES FALSOS"),
        "insiste con lo ya decidido: {s}"
    );

    // Pero el arbol lo sigue marcando: no se esconde, se deja de repetir.
    let t = c.ok(&["tree", "--todo"]);
    assert!(t.contains("CIERRE FALSO"), "el arbol dejo de marcarlo: {t}");
}

/// Lo que sobrevive a un abandono queda vivo bajo un descartado a proposito
/// (`d33`), asi que hay que volver a mirarlo. Aqui es donde se mira.
#[test]
fn el_rescatado_vuelve_a_pasar_por_delante() {
    let c = Caja::nueva("rescatado");
    c.ok(&["push", "Una meta", "--por", "hace falta"]);
    c.ok(&["push", "Elegir backend", "--por", "el store lo necesita"]);
    c.ok(&[
        "add",
        "Benchmark",
        "--padre",
        "2",
        "--por",
        "hay que medir antes",
    ]);
    c.ok(&["abandon", "2", "el backend ya no aplica", "--rescatar", "3"]);

    let s = c.ok(&["triage"]);
    assert!(seccion(&s, "SOBREVIVIERON A UN DESCARTE"), "{s}");
    assert!(s.contains("Benchmark"), "{s}");
    assert!(
        s.contains("el backend ya no aplica"),
        "no dice por que cayo su padre:\n{s}"
    );

    // Solo el borde, no la rama entera: lo que cuelga del rescatado no se
    // repite, o la vista de poda seria el arbol otra vez.
    c.ok(&[
        "add",
        "Nieto del rescatado",
        "--padre",
        "3",
        "--por",
        "cuelga",
    ]);
    let s = c.ok(&["triage"]);
    assert!(!s.contains("Nieto"), "listo la rama entera:\n{s}");
}

/// `MODEL.md` §6.1: a partir de 6 aparece en el triage, con `promote` como
/// salida. Una pila honda casi nunca es indisciplina.
#[test]
fn a_partir_de_seis_de_profundidad() {
    let c = Caja::nueva("hondo");
    for i in 1..=5 {
        c.ok(&["push", &format!("Nivel {i}"), "--por", "sigue bajando"]);
    }
    let s = c.ok(&["triage"]);
    assert!(!s.contains("A 6 O MAS"), "aviso a los cinco:\n{s}");

    c.ok(&["push", "Nivel 6", "--por", "uno mas"]);
    let s = c.ok(&["triage"]);
    assert!(seccion(&s, "A 6 O MAS DE LA RAIZ"), "{s}");
    assert!(s.contains("promote <id>"), "sin la salida:\n{s}");
    assert!(s.contains("profundidad 6"), "{s}");
}

/// La otra mitad de la audiencia. `--json` lleva las cuatro cestas, tambien
/// las vacias: un consumidor no deberia adivinar si falta una clave o si esta
/// vacia.
#[test]
fn el_json_lleva_las_cuatro_cestas() {
    let c = Caja::nueva("json");
    c.ok(&["push", "Una meta", "--por", "hace falta"]);
    let s = c.ok(&["triage", "--json"]);
    for k in ["aparcados", "hondos", "descolgados", "cierres_falsos"] {
        assert!(s.contains(k), "falta la cesta {k}:\n{s}");
    }
}
