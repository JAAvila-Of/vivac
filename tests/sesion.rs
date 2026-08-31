//! Los dos hooks de sesion, contra el binario.
//!
//! `f35`: Claude Code no tiene evento de fin de sesion. `Stop` es lo mas
//! cercano y corre **en cada turno**, asi que la parada automatica tiene que
//! saber cuando no hay nada que parar.

mod common;
use common::Caja;

fn cuantos(vivacs: &str, kind: &str) -> usize {
    vivacs.lines().filter(|l| l.contains(kind)).count()
}

/// Cuarenta turnos no son cuarenta paradas. Una parada que se repite identica
/// no es una parada: es log.
#[test]
fn un_stop_por_turno_no_deja_una_parada_por_turno() {
    let c = Caja::nueva("turnos");
    c.ok(&["push", "Una meta", "--por", "hace falta"]);
    for _ in 0..5 {
        c.ok(&["session", "end", "--hook"]);
    }
    let v = c.ok(&["vivacs"]);
    assert_eq!(cuantos(&v, "auto"), 1, "una parada por turno:\n{v}");
}

/// Pero en cuanto el arbol cambia, la siguiente parada si vale.
#[test]
fn una_parada_nueva_cuando_algo_cambio() {
    let c = Caja::nueva("cambio");
    c.ok(&["push", "Una meta", "--por", "hace falta"]);
    c.ok(&["session", "end", "--hook"]);
    c.ok(&["note", "1", "paso algo"]);
    c.ok(&["session", "end", "--hook"]);
    let v = c.ok(&["vivacs"]);
    assert_eq!(cuantos(&v, "auto"), 2, "se comio la parada buena:\n{v}");
}

/// Sin pila no hay largo que cerrar.
#[test]
fn sin_pila_no_hay_parada() {
    let c = Caja::nueva("sinpila");
    c.ok(&["session", "end", "--hook"]);
    let v = c.ok(&["vivacs"]);
    assert_eq!(cuantos(&v, "auto"), 0, "invento una parada vacia:\n{v}");
}

/// El brief va dentro del sobre que el agente lee, y nada suelto fuera: lo
/// que no va en el sobre, el agente no lo ve.
#[test]
fn el_hook_de_inicio_va_en_su_sobre() {
    let c = Caja::nueva("sobre");
    c.ok(&["push", "Una meta", "--por", "hace falta"]);
    let s = c.ok(&["session", "start", "--hook"]);
    assert_eq!(s.lines().filter(|l| !l.trim().is_empty()).count(), 1);
    assert!(s.contains("hookSpecificOutput"), "{s}");
    assert!(s.contains("SessionStart"), "{s}");
    assert!(s.contains("additionalContext"), "{s}");
    assert!(s.contains("Una meta"), "el sobre iba vacio:\n{s}");
}

/// **Un hook que falla en cada directorio sin arbol se desactiva a los dos
/// dias.** Los dos callan y salen con 0 donde no hay `.vivac/`, que es lo que
/// permite dejarlos en la configuracion global.
#[test]
fn callan_donde_no_hay_arbol() {
    let c = Caja::vacia("sinarbol");
    for args in [["session", "start", "--hook"], ["session", "end", "--hook"]] {
        let (s, cod) = c.correr(&args);
        assert_eq!(cod, 0, "{args:?} fallo fuera de un arbol:\n{s}");
        assert_eq!(s.trim(), "", "{args:?} hablo de mas:\n{s}");
    }
}
