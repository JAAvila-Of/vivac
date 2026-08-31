# Los tres pilares

**Gobiernan por definición.** Toda decisión de diseño se justifica contra ellos, y si los
contradice no entra. No son aspiraciones: son criterios de rechazo.

## Cómo arbitran entre sí

Van a chocar. Cuando choquen:

| Pilar | Poder |
|---|---|
| **Seguridad** | **veto**. Puede matar una funcionalidad entera, sin negociación |
| **Rendimiento** | **presupuesto**. Fija un techo que la funcionalidad debe respetar para existir |
| **DX** | **juez**. Decide entre las opciones que ya pasaron los otros dos |

Ejemplo real de choque, y por qué el orden importa: la búsqueda semántica quiere un modelo
de embeddings. Eso es una llamada de red (lenta, y pide una clave) o un modelo local
(pesado). Seguridad no quiere claves; rendimiento no quiere red en el camino de escritura.
**Conclusión forzada: el embedding nunca va en el camino de escritura, y la herramienta
tiene que funcionar entera sin él.** Ninguna discusión de DX puede revertir eso.

Segundo choque, resuelto el 2026-08-31 mientras se escribía el port: el modelo pedía que el
`actor` de cada evento fuera `git config user.email`. Es dato personal. **Seguridad vetó y
el modelo se corrigió**: el actor es un identificador opaco generado en `init`.

---

## Pilar 1 — DX

Quienes usan esto son desarrolladores. Tiene que ser intuitivo para ellos y **visualmente
atractivo**: TUI con buen diseño, no un volcado de texto.

### Dos audiencias, y es la bifurcación de diseño

| Quién | Qué hace | Qué necesita |
|---|---|---|
| **El agente** (ejecutor) | escribe: abre nodos, los cierra, anota | **CLI**: silenciosa, scriptable, códigos de salida, salida parseable |
| **El mantenedor** (humano) | lee: navega, entiende, decide | **TUI**: el árbol, el camino `why`, filtros, navegación |

**Regla con dientes: todo lo que el agente necesita hacer tiene que poder hacerse sin la
TUI.** Si una función sólo existe en la interfaz interactiva, un agente no puede usarla y el
proyecto pierde a la mitad de sus usuarios. La TUI nunca es el único camino a nada.

### La captura no puede costar más que perder el hilo

Es la tesis original y sigue siendo el criterio de DX que decide todo lo demás. Si registrar
un nodo cuesta más que un comando corto, no se registra, y sin registro no hay árbol.

Medido el 2026-08-30, y es la razón de que este proyecto exista con esta forma: `focus` y
`pop` se llamaron **9 veces en 170 minutos** porque se apoyaban en las costuras naturales
del trabajo (no podés cerrar un largo sin decir cómo quedó). En esos mismos 170 minutos,
una operación de guardado que pedía un juicio —«¿esto fue relevante?»— se llamó **0 veces**,
con un protocolo declarado obligatorio, porque ese juicio compite con el trabajo y pierde
bajo carga.

**Corolario de diseño: las operaciones se cuelgan de las costuras del trabajo, nunca de un
juicio de relevancia.**

### Reglas visuales

- **El significado nunca se codifica sólo en color.** `[x]`, `[~]`, `*` y
  `<== CIERRE FALSO` se leen en blanco y negro. El color refuerza, no informa.
- Degradar sin romperse: sin color cuando no hay tty, funcionando por ssh, y en Windows
  Terminal además de cmd.exe.
- Nada de spinners ni animación en el camino del agente.

---

## Pilar 2 — Rendimiento

El servidor tiene que ser **rápido**. Guarda y devuelve los datos de una IA agéntica, o sea
que está en el camino crítico de cada turno: lo que tarde, lo paga el usuario esperando.

### Presupuestos

**A medir y corregir, no verdades.** Los números medidos contra ellos están en el `README`.

| Operación | Techo | Por qué |
|---|---|---|
| escribir un nodo (`add`, `done`, `note`) | **p99 < 5 ms** | está en el camino crítico del turno del agente |
| `why` / `tree` sobre 10 000 nodos | **< 50 ms** | es lectura interactiva; por encima se siente |
| búsqueda de texto | **< 100 ms** | idem |
| búsqueda semántica | fuera del camino crítico | ver el arbitraje de arriba |

### Almacenamiento

SQLite, y hay que exprimirlo en vez de pelearlo:

- **WAL** para que leer no bloquee escribir.
- **FTS5** para el texto. Es lo que va a resolver la mayoría de las búsquedas reales.
- **Vectorial** (`sqlite-vec` o equivalente) para lo semántico, y **siempre opcional**: el
  producto tiene que ser completo sin ello.
- Los índices se piensan desde el modelo, no se agregan cuando duele: las consultas que
  importan son *ancestros de un nodo* y *descendientes abiertos de un nodo*, y las dos son
  recursivas. Medir con un árbol de verdad, no con diez nodos de juguete.

Hasta que el tamaño lo pida, el almacén es un log append-only que se pliega en memoria.
Medido el 2026-08-31: aguanta el presupuesto de escritura hasta el orden del millar de
nodos, y a diez mil se pasa. Ése es el disparador de SQLite, y ahora es un número.

### Anti-features

- **Sin demonio** para el camino local: es latencia de arranque y una cosa más que se cae.
- **Sin red en el camino de escritura.** Nunca.

---

## Pilar 3 — Seguridad

Esto guarda las decisiones de un desarrollo que muy probablemente sea privado. **El árbol es
un mapa de dónde un sistema es débil y todavía no está arreglado.**

### Guardas duras

- **Nunca se guardan claves ni secretos.** Guarda de redacción en el momento de escribir:
  prefijos conocidos (`sqa_`, `squ_`, `ghp_`, `sk-`, `AKIA`, cabeceras PEM) y cadenas de
  entropía alta. Ante la duda, rechazar la escritura y decir por qué; nunca guardar callando.
- **Nunca se guardan datos personales del usuario.** Ni correo, ni nombre, ni ruta de casa.
- **Nunca se guarda el contenido de los archivos.** Sólo rutas, referencias y prosa sobre lo
  que se decidió. Es la guarda más fuerte de todas porque acota el radio de daño de una fuga
  a «qué se estaba haciendo», nunca a «cuál es el código».
- **Nada de telemetría.** El binario no llama a casa. Jamás.

### Cifrado

- **Local**: cifrado en reposo opcional.
- **Cloud / colaboración de equipo**: cifrado **obligatorio**, y la pregunta que hay que
  contestar antes de escribir una línea de esa parte es **quién tiene la clave**. Si la tiene
  el servidor, no es privacidad, es una promesa.
- Identidad para el modo equipo: un identificador opaco, nunca correo ni nombre.
