//! `abandon` con rescate — `MODEL.md` §6, con la salvedad de `d33`.
//!
//! El rescate **no reparenta**. La invariante 11 dice que una cosa nace de un
//! sitio, y el esquema hace inmutable el `spawns` a proposito: viaja dentro
//! del evento de creacion. Un nodo rescatado se queda donde nacio, vivo bajo
//! un padre abandonado. Estos tests son lo que sostiene esa promesa.

mod common;
use common::Caja;

/// g1 > t2 > t3 > (t4, t5). Abandonar t2 pone en juego a los otros cuatro.
fn rama(nombre: &str) -> Caja {
    let c = Caja::nueva(nombre);
    c.ok(&[
        "push",
        "Migrar autenticacion",
        "--por",
        "el proveedor cierra",
    ]);
    c.ok(&[
        "push",
        "Elegir backend de cache",
        "--por",
        "el token store lo necesita",
    ]);
    c.ok(&[
        "add",
        "Benchmark de serializacion",
        "--padre",
        "2",
        "--por",
        "hay que medir antes",
    ]);
    c.ok(&[
        "add",
        "Limpiar imports muertos",
        "--padre",
        "3",
        "--por",
        "salio al pasar",
    ]);
    c
}

/// Sin `--cascada` no cae nada, y la lista de lo que caeria sale entera.
/// Abandonar tiene que costar lo mismo que `pop`, pero no en silencio.
#[test]
fn sin_cascada_no_cae_nada() {
    let c = rama("sincascada");
    let (s, cod) = c.correr(&["abandon", "2", "el backend ya no aplica"]);
    assert_eq!(cod, 1, "tenia que rechazarlo:\n{s}");
    for t in ["Elegir backend de cache", "Benchmark", "Limpiar imports"] {
        assert!(s.contains(t), "no listo {t}:\n{s}");
    }
    assert!(s.contains("--rescatar"), "no ofrecio el rescate:\n{s}");

    // Y nada se movio.
    let t = c.ok(&["tree", "--todo"]);
    assert!(!t.contains("[!]"), "abandono algo sin confirmacion:\n{t}");
}

/// Rescatar un nodo rescata su descendencia. Salvar al padre y dejar morir a
/// los hijos seria un rescate a medias que nadie pidio.
#[test]
fn el_rescate_arrastra_la_descendencia() {
    let c = rama("arrastra");
    let s = c.ok(&["abandon", "2", "el backend ya no aplica", "--rescatar", "3"]);
    assert!(s.contains("Rescatados"), "{s}");

    let t = c.ok(&["tree", "--todo"]);
    assert!(t.contains("[!] t2"), "t2 tenia que caer:\n{t}");
    assert!(t.contains("[ ] t3"), "t3 tenia que sobrevivir:\n{t}");
    assert!(t.contains("[ ] t4"), "t4 cayo con su padre rescatado:\n{t}");
}

/// Si se rescata todo lo abierto no queda nada que confirmar, y `--cascada`
/// deja de hacer falta: solo se confirma lo que cae sin haberse nombrado.
#[test]
fn rescatarlo_todo_no_pide_cascada() {
    let c = rama("todo");
    let (_, cod) = c.correr(&["abandon", "2", "ya no aplica", "--rescatar", "3"]);
    assert_eq!(cod, 0, "pidio cascada sin tener nada que llevarse");
}

/// **La promesa del producto.** El rescatado sigue naciendo de un nodo
/// abandonado, y `why` lo dice en vez de esconderlo. Reparentar habria hecho
/// que este camino mintiera.
#[test]
fn el_rescatado_sigue_naciendo_de_donde_nacio() {
    let c = rama("linaje");
    c.ok(&["abandon", "2", "el backend ya no aplica", "--rescatar", "3"]);

    let w = c.ok(&["why", "3"]);
    assert!(
        w.contains("Elegir backend de cache"),
        "borro el origen:\n{w}"
    );
    assert!(w.contains("abandonado"), "no dice que el origen cayo:\n{w}");
    assert!(
        w.contains("el backend ya no aplica"),
        "perdio el motivo del abandono:\n{w}"
    );

    // Y el almacen sigue sano: un vivo bajo un abandonado es una forma
    // legitima del arbol, no corrupcion.
    let (_, cod) = c.correr(&["check"]);
    assert_eq!(cod, 0, "check lo tomo por roto");
}

/// La pila es el camino al foco y no puede cruzar un nodo abandonado. El
/// rescatado sigue vivo, pero fuera del camino.
#[test]
fn la_pila_no_cruza_un_abandonado() {
    let c = rama("pila");
    c.ok(&["focus", "3"]);
    c.ok(&["abandon", "2", "ya no aplica", "--rescatar", "3"]);
    let s = c.ok(&["stack"]);
    assert!(
        !s.contains("Elegir backend de cache"),
        "un abandonado sigue en la pila:\n{s}"
    );
    assert!(
        !s.contains("Benchmark"),
        "el rescatado quedo en un camino que ya no existe:\n{s}"
    );
}

/// Rescatar algo que no cuelga del abandonado no tiene sentido, y decirlo es
/// mas barato que dejarlo pasar: la CLI del agente no ignora nada.
#[test]
fn no_se_rescata_lo_que_no_cuelga() {
    let c = rama("ajeno");
    let (s, cod) = c.correr(&[
        "abandon",
        "2",
        "ya no aplica",
        "--cascada",
        "--rescatar",
        "1",
    ]);
    assert_eq!(cod, 2, "dejo pasar un rescate imposible:\n{s}");
    assert!(s.contains("no cuelga"), "{s}");

    let (s, cod) = c.correr(&[
        "abandon",
        "2",
        "ya no aplica",
        "--cascada",
        "--rescatar",
        "2",
    ]);
    assert_eq!(cod, 2, "se rescato a si mismo:\n{s}");
}
