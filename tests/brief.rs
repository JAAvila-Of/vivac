//! El contrato de test de `BRIEF-SPEC.md` §10, sobre el binario de verdad.
//!
//! No usa ninguna dependencia: `CARGO_BIN_EXE_vivac` lo da cargo, y el store
//! es un directorio temporal. Cada prueba siembra su propio arbol, porque un
//! arbol compartido haria que el orden de ejecucion importara.

use std::path::PathBuf;
use std::process::Command;

const BIN: &str = env!("CARGO_BIN_EXE_vivac");

struct Caja(PathBuf);

impl Caja {
    fn nueva(nombre: &str) -> Caja {
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

    fn correr(&self, args: &[&str]) -> (String, i32) {
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

    fn ok(&self, args: &[&str]) -> String {
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

/// Un arbol con una de cada cosa, para que ninguna seccion salga vacia.
fn poblado(nombre: &str) -> Caja {
    let c = Caja::nueva(nombre);
    c.ok(&[
        "push",
        "Migrar autenticacion a OIDC",
        "--por",
        "el proveedor viejo cierra",
    ]);
    c.ok(&[
        "add",
        "Sin dependencias con licencia contagiosa",
        "--padre",
        "1",
        "--tipo",
        "constraint",
        "--por",
        "politica de la empresa",
    ]);
    c.ok(&[
        "push",
        "Elegir backend de cache",
        "--por",
        "el token store lo necesita",
        "--governs",
        "src/cache/**",
    ]);
    c.ok(&[
        "decide",
        "Usar token store distribuido",
        "--razon",
        "un solo nodo no aguanta",
        "--alternativa",
        "JWT sin revocacion",
    ]);
    c.ok(&[
        "add",
        "El volumen de tokens cabe en un nodo?",
        "--padre",
        "3",
        "--tipo",
        "question",
        "--bloquea",
        "--por",
        "decide el backend",
    ]);
    c.ok(&[
        "add",
        "Actualizar tests de integracion",
        "--padre",
        "3",
        "--por",
        "el backend cambia lo que hay que montar",
    ]);
    c.ok(&[
        "park",
        "6",
        "faltaba decidir el backend antes de tocar los tests",
    ]);
    c.ok(&[
        "flag",
        "4",
        "suspect",
        "--por",
        "asumia Redis, y no hay Redis en staging",
    ]);
    c.ok(&[
        "save",
        "antes de tocar el adaptador",
        "--luego",
        "extraer el validador",
    ]);
    c
}

fn seccion(brief: &str, titulo: &str) -> bool {
    brief.lines().any(|l| l.trim() == titulo)
}

/// §10.1 — Mismo log, mismo `--now`, dos ejecuciones, mismos bytes.
#[test]
fn determinismo() {
    let c = poblado("det");
    let a = c.ok(&["brief", "--now", "2026-09-15T10:00:00Z"]);
    let b = c.ok(&["brief", "--now", "2026-09-15T10:00:00Z"]);
    assert_eq!(a, b);
    assert!(
        a.contains("2026-09-15"),
        "el --now manda sobre el reloj:\n{a}"
    );
}

/// §10.2 — Con el presupuesto apretado, la espina sale entera y avisa.
///
/// Es la regla mas dura de la especificacion: si la espina no cabe, el
/// presupuesto esta mal, no el brief. Sin ella el brief no responde la
/// pregunta 1 y no tiene razon de existir.
#[test]
fn la_espina_nunca_se_trunca() {
    let c = poblado("espina");
    let espina = |b: &str| {
        assert!(
            b.contains("Migrar autenticacion a OIDC"),
            "falta la raiz:\n{b}"
        );
        assert!(b.contains("Elegir backend de cache"), "falta el foco:\n{b}");
        assert!(b.contains("<== AQUI"), "falta el marcador:\n{b}");
    };

    // Apretado pero alcanzable: cabe recortando, y lo dice.
    let b = c.ok(&["brief", "--budget", "200", "--now", "2026-09-15T10:00:00Z"]);
    espina(&b);
    assert!(b.contains("recortados"), "recorto sin decirlo:\n{b}");

    // Imposible: ni quitando todo lo truncable cabe. La espina sale igual, y
    // el aviso dice que lo que sobra es arbol, no render.
    let b = c.ok(&["brief", "--budget", "40", "--now", "2026-09-15T10:00:00Z"]);
    espina(&b);
    assert!(b.contains("excede el presupuesto"), "no aviso:\n{b}");
}

/// §10.3 — Al bajar el presupuesto las secciones caen de abajo arriba, nunca
/// salteado.
#[test]
fn orden_de_truncado() {
    let c = poblado("trunc");
    let entero = c.ok(&["brief", "--budget", "5000", "--now", "2026-09-15T10:00:00Z"]);
    assert!(seccion(&entero, "ULTIMO VIVAC"), "{entero}");
    assert!(seccion(&entero, "NO TOCAR AHORA"), "{entero}");
    assert!(seccion(&entero, "MARCADO"), "{entero}");

    // El vivac es la seccion 9 y cae antes que la 7 y la 6.
    let apretado = c.ok(&["brief", "--budget", "150", "--now", "2026-09-15T10:00:00Z"]);
    assert!(
        !seccion(&apretado, "ULTIMO VIVAC"),
        "deberia haber caido:\n{apretado}"
    );

    // Y las no truncables aguantan: invariantes y preguntas bloqueantes.
    assert!(seccion(&apretado, "INVARIANTES"), "{apretado}");
    assert!(seccion(&apretado, "BLOQUEA"), "{apretado}");
}

/// §10.5 — Una decision superada no se renderiza nunca.
#[test]
fn superada_ausente() {
    let c = poblado("sup");
    c.ok(&[
        "decide",
        "Usar sesiones en base de datos",
        "--razon",
        "mas simple",
        "--supersedes",
        "4",
    ]);
    let b = c.ok(&["brief", "--budget", "5000", "--now", "2026-09-15T10:00:00Z"]);
    assert!(b.contains("Usar sesiones en base de datos"), "{b}");
    assert!(
        !b.contains("Usar token store distribuido"),
        "la superada sigue ahi:\n{b}"
    );
}

/// §10.7 — Pila vacia produce §8, nunca una salida vacia.
#[test]
fn estado_inicial() {
    let c = Caja::nueva("inicial");
    let b = c.ok(&["brief"]);
    assert!(b.contains("Sin foco activo"), "{b}");
    assert!(b.contains("vivac push"), "sin accion concreta:\n{b}");

    c.ok(&["push", "Una meta", "--por", "hace falta"]);
    c.ok(&["park", "sin terminar"]);
    let b = c.ok(&["brief"]);
    assert!(b.contains("Sin foco activo"), "{b}");
    assert!(
        b.contains("OBJETIVOS ABIERTOS") || b.contains("focus"),
        "{b}"
    );
}

/// §10.8 — Ninguna seccion vacia emite encabezado.
#[test]
fn sin_encabezados_huecos() {
    let c = Caja::nueva("hueco");
    c.ok(&["push", "Sola", "--por", "no cuelga nada de ella"]);
    let b = c.ok(&["brief"]);
    for t in [
        "INVARIANTES",
        "BLOQUEA",
        "NO TOCAR AHORA",
        "MARCADO",
        "DECISIONES VIGENTES",
    ] {
        assert!(!seccion(&b, t), "salio {t} vacia:\n{b}");
    }
}

/// §10.9 — Ninguna bandera se renderiza sin su motivo, porque no se puede
/// levantar sin el.
#[test]
fn motivo_obligatorio() {
    let c = Caja::nueva("motivo");
    c.ok(&["push", "Algo", "--por", "hace falta"]);
    let (s, cod) = c.correr(&["flag", "1", "suspect"]);
    assert_eq!(cod, 2, "una bandera sin motivo tiene que fallar:\n{s}");
    assert!(s.contains("--por"), "{s}");
}

/// §10.6 — Sin control de versiones no hay lineas de diff, y se dice.
#[test]
fn degradacion_sin_ancla() {
    let c = poblado("null");
    let s = c.ok(&["restore", "v1"]);
    assert!(s.contains("Sin ancla"), "{s}");
    assert!(!s.contains("cambios desde"), "invento un diff:\n{s}");
}

/// La guarda de redaccion vale tambien aqui: no hay puerta trasera por
/// `decide` ni por `flag`.
#[test]
fn la_guarda_cubre_las_operaciones_nuevas() {
    let c = Caja::nueva("guarda");
    c.ok(&["push", "Algo", "--por", "hace falta"]);
    let (_, cod) = c.correr(&[
        "decide",
        "Rotar",
        "--razon",
        "usar ghp_16C7e42F292c6912E7710c838347Ae178B4a",
    ]);
    assert_eq!(cod, 3, "decide dejo pasar una credencial");
    let (_, cod) = c.correr(&["flag", "1", "review", "--por", "ver /home/unnombre/.config"]);
    assert_eq!(cod, 3, "flag dejo pasar una ruta personal");
}

/// `f30` — una decision vigente no es un hijo pendiente. Sale en su seccion y
/// en ninguna otra: listarla dos veces llena el brief de cosas que no hay que
/// hacer, que es lo contrario de para lo que existe.
#[test]
fn una_decision_no_es_un_frente() {
    let c = poblado("dec");
    let b = c.ok(&["brief", "--budget", "5000", "--now", "2026-09-15T10:00:00Z"]);
    assert_eq!(
        b.matches("Usar token store distribuido").count(),
        1,
        "la decision sale mas de una vez:\n{b}"
    );

    // Y esa unica vez esta debajo de DECISIONES VIGENTES, no de NACIO DE AQUI.
    let hasta = b.find("DECISIONES VIGENTES").expect("falta la seccion");
    assert!(
        b.find("Usar token store distribuido").unwrap() > hasta,
        "sale antes de su seccion, o sea como hijo pendiente:\n{b}"
    );

    let o = c.ok(&["open"]);
    assert!(
        !o.contains("Usar token store distribuido"),
        "open la lista como frente:\n{o}"
    );
    assert!(
        o.contains("1 decision vigente"),
        "open la desaparecio sin decirlo:\n{o}"
    );
}

/// `q26` — cerrar un padre no puede hacer invisibles a sus hijos abiertos.
///
/// El caso salio del arbol del propio proyecto: `t8` cerro con `t9`, `t10` y
/// `f21` abiertos por debajo, y el brief mostro 3 de los 6 frentes. Listarlos
/// seria traer el arbol entero; contarlos no.
#[test]
fn lo_abierto_bajo_un_cerrado_se_cuenta() {
    let c = Caja::nueva("hondo");
    c.ok(&["push", "La meta", "--por", "hace falta"]);
    c.ok(&["push", "Una rama", "--por", "la meta la necesita"]);
    c.ok(&[
        "add",
        "Un hallazgo",
        "--padre",
        "2",
        "--por",
        "salio al pasar",
    ]);
    // Cierra con un hijo abierto que no bloquea: es correcto, y es el caso.
    c.ok(&["pop", "rama terminada"]);

    let b = c.ok(&["brief", "--budget", "5000", "--now", "2026-09-15T10:00:00Z"]);
    assert!(
        b.contains("+ 1 mas abajo"),
        "no aviso de lo que quedo debajo:\n{b}"
    );
    assert!(
        !b.contains("Un hallazgo"),
        "lo listo en vez de contarlo; eso trae el arbol entero:\n{b}"
    );
    assert!(
        b.contains("vivac open"),
        "conto sin decir donde mirar:\n{b}"
    );
}
