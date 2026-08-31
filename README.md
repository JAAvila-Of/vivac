# vivac

**Un árbol donde cada nodo sabe de cuál nació.** Sirve para contestar *«¿por qué
estamos acá?»* meses después, cuando ya nadie se acuerda.

```
$ vivac why 11

  Por que estamos aca  ->  t11
  ------------------------------------------------------------------

  g1    vivac 0.1 publicable
        Un sistema de procedencia del trabajo que conteste "por que
        estamos aca" meses despues.
        (7 abiertos / 4 cerrados por debajo)
        |
        v
  t8    Portar a Rust en el repo publico
        Cuando el formato deje de moverse, no antes.
        (3 abiertos por debajo)
        |
        v
  t11   Guarda de redaccion al escribir
        Pilar de seguridad. Va ANTES de cualquier modo cloud.

        ^^^ estas aca

  En paralelo, sin cerrar (2):
      t9     TUI para el mantenedor
      t10    Migrar de JSON a SQLite

  t8 no cierra hasta que cierren (1):
      t11    Guarda de redaccion al escribir
```

## El problema

Al desarrollar con una IA agéntica, el trabajo engendra más trabajo. A los tres
saltos perdiste el hilo de lo que ibas a hacer originalmente.

No es falta de memoria: normalmente está todo escrito. **Es falta de
procedencia.** Lo escrito no dice *de qué* nació, y sin esa arista no hay forma
de reconstruir por qué estás donde estás.

Medido en un compilador real: el camino entre la meta y el trabajo del día tenía
**seis niveles**, repartidos en un tracker de 8 853 líneas ordenado
cronológicamente, 52 documentos de plan y 21 issues. La estructura era temporal,
que es exactamente lo contrario de la procedencia.

Las bitácoras, las ADR y las issues guardan el **nodo**. Ninguna guarda la
**arista**. Por eso podés tenerlo todo escrito y aun así no poder contestar de
dónde salió una cosa.

## Cómo se usa

Hay dos audiencias, y la herramienta se parte en dos por ellas.

**El agente escribe.** La captura se cuelga de las costuras del trabajo: abrís
un nodo cuando empezás, lo cerrás cuando terminás. La arista de procedencia se
crea sola, sin que nadie tenga que acordarse de declararla.

```sh
vivac push "Arreglar el adaptador de cache" --por "el bug de sesiones lo necesita"
vivac push "Falta un test de expiración" --por "no hay como reproducir el bug" --bloquea
vivac pop "reproducido: expira a los 300s, no a los 3600"
vivac pop "adaptador arreglado"
```

**El mantenedor lee.**

```sh
vivac brief         dónde estás, qué gobierna este punto y qué NO tocar ahora
vivac why 11        el camino desde la raíz, narrado
vivac tree          el árbol, con los cierres falsos marcados
vivac open          los frentes abiertos, cada uno con su linaje
vivac stack         la pila de foco
vivac parked        NO TOCAR AHORA
```

**Y hay paradas seguras.** Un vivac es la parada a mitad de ascensión: estado
coherente, con la pila congelada y la identidad del código en ese momento.
`push`, `pop` y `park` dejan una sin que se la pida nadie.

```sh
vivac save "antes de tocar el adaptador" --luego "extraer el validador"
vivac restore v14   reconstruye la pila y dice qué cambió desde entonces
```

`restore` **no toca el árbol de trabajo, nunca**. Mezclar navegación de contexto
con manipulación del árbol da un gestor de ramas peor que git.

Todo lo que el agente necesita hacer se puede hacer sin interfaz interactiva, y
todos los comandos de lectura aceptan `--json`.

## Las dos aristas

Es la distinción que sostiene el modelo, y salió de sembrar dos árboles reales y
ponerlos uno al lado del otro:

|                | Pregunta que contesta | Cuándo se crea |
|---|---|---|
| **nació de**   | ¿de dónde salió esto? | sola, en cada `push` |
| **`--bloquea`**| ¿esto impide cerrar a su padre? | explícita |

Un lote de issues cerrado con un hallazgo abierto debajo es **correcto**: el lote
terminó y el hallazgo es otra cosa. Una auditoría marcada `DONE` con sus
hallazgos abiertos es un **marcador falso** — uno así tardó 26 días en
detectarse. Misma forma, veredicto opuesto.

Por eso `vivac done` **rehúsa** cerrar con condiciones abiertas y lista lo que
falta. Es la única regla del modelo que rechaza una operación, y se gana ese
privilegio porque el caso que previene está medido.

```
$ vivac done 8

  t8 NO puede cerrar: 1 condicion(es) de cierre abierta(s)

      t11    Guarda de redaccion al escribir

  Una corrida cierra con sus hallazgos, no con su informe.
  Cerrarlo igual deja rastro:  vivac done 8 --forzar
```

## Lo que nunca guarda

Un árbol de procedencia es un mapa de dónde un sistema es débil y todavía no
está arreglado. Eso obliga a algunas cosas, y no son negociables:

- **Ni claves ni secretos.** Hay una guarda de redacción en el momento de
  escribir. Ante la duda rechaza y dice por qué; nunca guarda callando.
- **Ni datos personales.** Ni correo, ni nombre, ni ruta de casa. El `actor` de
  cada evento es un identificador opaco.
- **Ni el contenido de los archivos.** Sólo rutas, referencias y prosa sobre lo
  que se decidió. Acota el radio de daño de una fuga a *qué se estaba haciendo*,
  nunca a *cuál es el código*.
- **Nada de telemetría.** El binario no llama a casa.

Las tres reglas salen de los [pilares](docs/PILARES.md), que gobiernan por
definición: **seguridad veta, rendimiento presupuesta, DX juzga.**

## Estado

**Tier 0 completo.** El árbol, las dos aristas, la regla de cierre, la guarda de
redacción, el `brief` con presupuesto de tokens, los hooks de sesión, los vivacs
y el `Anchor` con implementaciones `Git` y `Null`. 46 tests, de los que 11 son el
contrato de la especificación del brief ejecutado sobre el binario.

El `brief` es determinista por contrato: mismo log, mismo `--now`, mismos bytes.
La espina —el camino de la raíz al foco— **nunca se trunca**: si no cabe en el
presupuesto, sale igual y el aviso dice que lo que sobra es árbol, no render.

Medido en esta máquina, descontando el arranque del proceso:

| nodos | `push` | `brief` | `tree` |
|---|---|---|---|
| 100 | ~5 ms | ~5 ms | ~5 ms |
| 1 000 | ~11 ms | ~10 ms | ~13 ms |
| 10 000 | ~54 ms | ~63 ms | ~95 ms |

El presupuesto de escritura es p99 < 5 ms y el de lectura < 50 ms sobre 10 000
nodos. En el orden del centenar de nodos se cumple, y de ahí para arriba se
degrada linealmente con el tamaño del log, que se lee entero en cada llamada.
**Ahí entra SQLite**, y ahora tiene un número en vez de una corazonada.

No están todavía: TUI, búsqueda, invalidación en cascada, modo equipo.

## Hooks

```sh
vivac hooks     imprime lo que hay que pegar en .claude/settings.json
```

`SessionStart` inyecta el brief en el contexto del agente; `Stop` deja una parada
automática. Los dos callan y salen con 0 donde no hay `.vivac/`, así que se
pueden dejar en la configuración global sin molestar en otros proyectos.

## Instalación

```sh
cargo install --path .
vivac init
```

Sin demonio, sin servidor y sin red. El almacén es `.vivac/`, dos archivos.

## Licencia

`MIT OR Apache-2.0`, a elección de quien lo use. El texto de cada una está en
[`LICENSE-MIT`](LICENSE-MIT) y [`LICENSE-APACHE`](LICENSE-APACHE).
