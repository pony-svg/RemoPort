# Plan de implementación

Cómo llevar Remoport de su estado en `8b9769c` a la arquitectura descrita en
[ARQUITECTURA.md](ARQUITECTURA.md). Siete fases. La app funciona al terminar
cada una.

## Reglas para todas las fases

1. **Una fase por tanda de trabajo.** Al terminar una fase se para y se informa.
   No se empieza la siguiente sin el visto bueno del dueño.
2. **Mover y cambiar van en commits distintos.** Un commit que mueve código no
   cambia su lógica; se revisa con `git diff --color-moved`. Un commit que
   cambia comportamiento no mueve archivos.
3. **Los tests no bajan.** La base es 120: 50 de la app, 65 del motor, 5 de
   integración. Un test que se mueve sigue existiendo; uno que se borra se
   justifica en el commit.
4. **Terminado significa estos cuatro comandos en verde**, con su salida real:

   ```bash
   cargo fmt --all -- --check
   ```
   ```bash
   touch src/main.rs remoport-rdp/src/lib.rs && cargo clippy --workspace --all-targets -- -D warnings
   ```
   ```bash
   cargo test --workspace
   ```
   ```bash
   scripts/check-boundaries.sh
   ```

   El `touch` obliga a clippy a revisar de verdad. El último comando existe
   desde la fase 0.
5. **La prueba contra el PC real solo la hace el dueño.** Cada fase lista qué
   probar. Quien implementa no puede darla por hecha: los tests no cubren el
   dibujo ni el teclado físico.
6. **Reglas del proyecto que siguen vigentes:** nunca `git push`; comprobar que
   el autor es `pony-svg <233282001+pony-svg@users.noreply.github.com>` antes
   de cada commit; nunca registrar contraseñas, texto escrito o pegado, ni
   sonido; nunca añadir al repositorio `MECHANICS.md`, `mechanics.json` ni
   `shots/`; todo texto visible pasa por `t()` o `tf()` con su fila en español.
7. **Si el plan está equivocado, se dice.** Si un paso no se puede hacer como
   está escrito, se para, se explica y se propone la alternativa. No se cambia
   de rumbo en silencio.

## Resumen

| Fase | Qué consigue | Cambia lo que ve el usuario | Tamaño |
|---|---|---|---|
| 0 | Red de seguridad: comprobación de fronteras y constructor de `ConnectConfig` | No | Pequeña |
| 1 | Teclado por código físico: arregla el numérico y AltGr | Sí | Mediana |
| 2 | `session.rs` partido por responsabilidad | No | Grande |
| 3 | Un solo dueño de los datos del PC | No | Mediana |
| 4 | Crate `remoport-core`: la frontera la vigila el compilador | No | Mediana |
| 5 | Motor: configuración agrupada, conexión por etapas, carpeta de canales | No | Mediana |
| 6 | `app.rs` en páginas | No | Mediana |

Las fases 0 a 4 van en ese orden. La 5 solo toca el motor y puede hacerse antes
o después de la 2, 3 y 4. La 6 puede esperar a que los espacios de trabajo
toquen la página de Aplicaciones.

Después de la fase 4, Workspaces + RemoteApp se construye sobre esta
estructura. No forma parte de este plan.

---

## Fase 0. Red de seguridad

**Objetivo:** poder refactorizar sin romper en silencio.

**Pasos**

1. Añadir `docs/ARQUITECTURA.md` y este plan al repositorio.
2. Crear `scripts/check-boundaries.sh`. De momento comprueba solo la regla R1,
   que ya se cumple: `cargo tree -p remoport-rdp -e normal` no contiene egui,
   eframe, cpal, wayland-client, keyring ni winit. El script falla con un
   mensaje que nombra la regla rota.
3. Añadir ese script como paso de `.github/workflows/ci.yml`, y actualizar
   `actions/checkout@v4` a `v5` (aviso de Node 20 del primer CI).
4. Dar a `ConnectConfig` un constructor con valores por defecto:
   `ConnectConfig::new(host, username, password, trust_store)`. Los cuatro
   tests del motor que la construyen a mano pasan a usarlo, evitando el
   acoplamiento de campos. La app (`src/session.rs`) mantiene la construcción
   literal campo por campo: si el motor añade un campo nuevo, el compilador debe
   obligar a la app a decidir su valor explícitamente y no pasar por alto
   ajustes de seguridad en silencio.

**Terminado cuando:** los cuatro comandos pasan, hay 120 tests o más, y añadir
un campo de prueba a `ConnectConfig` ya no obliga a tocar ningún test (se
comprueba añadiéndolo y quitándolo, sin commit).

**Prueba manual:** una conexión normal. Nada debe cambiar.

**Riesgo:** bajo.

---

## Fase 1. Teclado por código físico

**Objetivo:** que cada tecla llegue al PC por su posición real. Arregla el
teclado numérico y, si se confirma, AltGr.

**Antes de escribir código, confirmar el diagnóstico.** La prueba no bloquea.
El arreglo envía el código físico de cada tecla, así que lo que egui interprete
hoy deja de importar, y ninguna respuesta posible cambia lo que hay que
programar. Sirve de línea base: si no se hace la prueba previa, se anota en el
informe que no hay línea base externa y se continúa con la implementación.

**Diseño**

- Un módulo nuevo `src/input.rs` con:
  - la tabla completa de código físico de Linux (evdev) a scancode: bloque
    principal, teclado numérico (`55`, `71`–`83`, `96`, `98`), navegación,
    Ctrl y Alt derechos (AltGr es Alt derecho, `E0 38`), Windows, Menú,
    Impr Pant;
  - `KeyState`, el único conjunto de teclas pulsadas, con `press`, `release` y
    `release_all`;
  - los atajos propios de Remoport (Ctrl+Alt+Intro), que no llegan al PC;
  - la regla del pegado que hoy vive en `strokes_for`.
- `key_grab` deja de enviar `Command` al motor. Envía por un canal, al hilo de
  la interfaz, la tecla física, los bloqueos y la pérdida de foco, y despierta
  a la interfaz para que los procese en ese cuadro.
- Mientras el listener de Wayland funciona, es la única fuente de teclas. Los
  `egui::Event::Key` se ignoran. Sin Wayland, o si el listener falla, sigue el
  camino actual por egui y el log lo dice una vez.

**Diez puntos clave de diseño:**

1. **Autorrepetición.** Wayland no envía repeticiones: las genera cada cliente
   con `wl_keyboard.repeat_info` (tasa y retardo, desde la versión 4 de
   `wl_keyboard`). Hoy las da egui. Si la fuente pasa a ser el listener, la
   repetición se pierde, salvo que la generes tú. Si el teclado no anuncia
   `repeat_info`, usa un valor por defecto razonable y dilo en el log una vez.
   Al soltar la tecla, la repetición se detiene de inmediato: ni una pulsación más.
2. **Teclas de bloqueo.** Bloq Mayús, Bloq Num y Bloq Despl (evdev 58, 69 y 70)
   no se envían como teclas. Ya se sincronizan con `Command::LockKeys` desde
   `wl_keyboard.modifiers`; si se enviaran además, el PC las cambiaría dos veces.
3. **Modificadores.** Con la fuente física, Shift, Ctrl y Alt, izquierdos y
   derechos, salen de ella. `sync_modifiers` no puede seguir enviando en ese
   modo, o llegarían duplicados.
4. **Tecla Windows y Menú.** Hoy solo van al PC en pantalla completa; en ventana
   son del escritorio. Eso se mantiene.
5. **Despertar a la interfaz.** El hilo del listener avisa con un callback
   (`Box<dyn Fn() + Send + Sync>` que llama a `request_repaint`), creado fuera
   de `key_grab`. `key_grab` no recibe `egui::Context`; así la regla R3 se puede
   cumplir en la fase 4.
6. **Pegar llega por dos caminos.** La V física llega por el canal del
   listener; el `Paste` con el texto llega por egui. Son hilos distintos: nada
   asegura que caigan en el mismo cuadro ni en qué orden. La regla del plan
   ("la V física espera al `Paste` de egui de ese cuadro") acepta el `Paste`
   antes o después de la V con un límite de tiempo de 150 ms (tiempo imperceptible
   para el tipeo humano pero suficiente para absorber la latencia entre hilos
   y cuadros de la interfaz). Pasado el límite se envía la V sola. Si la V se
   suelta mientras espera, la suelta sale después de su pulsación, nunca antes.
7. **Una sola fuente incluye Copiar, Cortar y Pegar.** `Event::Copy`,
   `Event::Cut` y `Event::Paste` no envían las pulsaciones físicas de C, X o V
   cuando la fuente Wayland está activa, ya que llegan directamente por el
   listener físico. El texto de `Paste` se mantiene para emitir `Command::Paste`.
8. **Entrar y salir con teclas pulsadas.** Al recibir el foco
   (`wl_keyboard.enter`), las teclas que ya estaban pulsadas no se envían, y
   su suelta posterior tampoco (evitando que el PC reciba eventos desemparejados).
   Al perder el foco (`leave`) o cuando la pantalla remota pierde el teclado,
   se suelta todo lo enviado y la repetición se detiene. `KeyState` en `src/input.rs`
   es el único dueño del conjunto de teclas pulsadas, tanto para Wayland como
   para el respaldo por egui.
9. **El reloj de la repetición.** El bucle del listener Wayland calcula el
   tiempo de espera de poll con base en el próximo instante de repetición
   (`RepeatTracker` en `src/input.rs`), generando `WaylandKeyEvent::Repeat`
   por el canal de eventos para asegurar una cadencia uniforme y precisa.
10. **Códigos que se parecen.** Se distingue `*` del numérico (evdev 55, `0x37`
    sin extender) de Impr Pant (evdev 99, `0x37` extendido). Teclas complejas
    como Pausa (evdev 119) requieren secuencias multibyte no representables en
    un `Scancode` simple y no producen scancode.

**Puntos delicados**

- **Nunca dos fuentes a la vez.** Si ambas envían, cada tecla se escribe dos
  veces. Tiene que haber un test que lo fije.
- **Pegar.** Hoy egui convierte Ctrl+V en un evento `Paste` con el texto y se
  queda la pulsación; la app envía `Command::Paste` y el motor retiene la V
  hasta que el PC acepta el texto. Con teclas físicas la V llega por otro
  camino. La regla: con Ctrl pulsado y portapapeles compartido, la V física
  espera al `Paste` de egui de ese cuadro; si no llega, se envía la V sola.
  Hay que probar los dos sentidos del portapapeles.
- **Foco.** El listener recibe teclas siempre que la ventana tiene el foco,
  también con un diálogo o la barra abiertos. Solo se envían al PC si la
  pantalla remota tiene el teclado.
- **Nada de lo escrito va al log.** Solo combinaciones reservadas y recuentos,
  como ahora.

**Tests nuevos:**
- tabla completa: por ejemplo `83 → 0x53`, `55 → 0x37`, `96 → E0 1C`, `98 → E0 35`,
  `100 → E0 38`, `97 → E0 1D`, `74 → 0x4A`, `78 → 0x4E`;
- 58, 69 y 70 no producen tecla;
- una sola fuente: con el listener activo, un `egui::Event::Key` no envía nada;
- repetición: tecla mantenida repite según tasa y retardo, y al soltarla no
  sale ni una repetición más;
- regla del pegado y atajo propio Ctrl+Alt+Intro.

**Arreglos posteriores (revisión previa a subir):**
- **Modificadoras y bloqueos sin autorrepetición:** `is_repeatable_key` en `remoport-core` excluye Shift (`42`, `54`), Ctrl (`29`, `97`), Alt/AltGr (`56`, `100`), Windows (`125`, `126`) y los bloqueos (`58`, `69`, `70`) en `RepeatTracker::press`, deteniendo también cualquier repetición previa activa.
- **Texto pegado protegido desde la UI:** `egui::Event::Paste` se envuelve de inmediato en `ClipboardText`, `InputAction::Paste` lleva `ClipboardText` sin copiarlo con `clone()`, y `pending_paste` solo conserva el instante.
- **Suelta única de la `V` tras pegar:** `KeyState` registra `V` (`0x2F`) como pulsada al emitir `InputAction::Paste` para emitir su suelta exactamente una vez y siempre después: al soltar la `V` o perder el foco, justo detrás del `Paste` si ya se soltó mientras esperaba, o al vencer el límite de 150 ms si no llegó pulsación física.
- **Cola FIFO tras `Ctrl+V` rápido:** mientras `pending_v` espera `Event::Paste` en `KeyState`, toda tecla física posterior (incluida la suelta de `V` o `Ctrl`) espera en `KeyQueue` (cuyo `Debug` solo expone el conteo) y se procesa en orden al resolverse; `handle_leave` la descarta limpiamente y suelta las teclas activas en el PC.
- **Espera acotada con Ctrl+V mantenido:** cada tecla de la cola se procesa con el instante en que llegó, no con el de la resolución. Antes, mantener Ctrl+V sin texto encadenaba una espera de 150 ms por cada repetición de la `V`: con 2 s pulsado, la suelta de Ctrl llegaba al PC 4,5 s tarde. Ahora una `V` de la cola tiene la misma ventana que tendría sin cola, y ninguna tecla espera más de una ventana y un fotograma.
- **El pegado nunca sale sin Ctrl:** en cada fotograma la app procesa los eventos de egui antes que las teclas físicas. Si Ctrl y `V` caían en el mismo fotograma, el texto salía antes que Ctrl y el PC escribía una «v». Ahora, si Ctrl no está pulsada en el PC, el texto espera el Ctrl físico durante la misma ventana de 150 ms y sale justo detrás. Si no llega ningún Ctrl, porque fue una tecla Pegar o porque Ctrl ya estaba pulsada al recibir el foco, Remoport pulsa y suelta Ctrl alrededor del pegado. Si se pierde el foco mientras el texto espera, el texto se descarta.

**Terminado cuando:** los cuatro comandos pasan con más de 120 tests,
`src/key_grab.rs` no nombra `Command`, `Commander` ni `egui`, y el informe
incluye la lista de prueba manual de la fase 1 del plan, con estos añadidos:
- mantener Retroceso: borra seguido y para al soltar;
- Bloq Mayús y Bloq Num aquí: el PC los sigue y no se desfasan;
- tecla Windows en ventana: no llega al PC; en pantalla completa sí.

**Prueba manual (el dueño)**

1. Numérico con Bloq Num activado y desactivado: dígitos, `.` `*` `+` `-` `/`,
   Intro. En el Bloc de notas y en Excel.
2. `@` con AltGr+Q y otros símbolos de AltGr.
3. Mayúsculas, `ñ`, tildes.
4. Mantener pulsado Retroceso: borra seguido y para al soltar.
5. Bloq Mayús y Bloq Num aquí: el PC los sigue y no se desfasan.
6. Copiar y pegar en los dos sentidos.
7. Tecla Windows y Menú: en ventana no llegan al PC (son del escritorio); en
   pantalla completa sí van al PC.
8. Pantalla completa: tecla Windows, Alt+Tab, salir con Ctrl+Alt+Intro.
9. Cambiar a otra ventana con una tecla pulsada y volver: no debe quedar
   ninguna tecla atascada en el PC.
10. Abrir Editar PC o escribir en el formulario de acceso: nada de eso debe
    llegar al PC.

**Riesgo:** medio. Es la única fase que cambia el comportamiento.

---

## Fase 2. `session.rs` por responsabilidad

**Objetivo:** que cada pieza de la sesión se pueda leer, cambiar y probar sola.
Sin cambio de comportamiento. Todo sigue dentro del binario.

**Pasos, un commit o más por cada uno**

1. `src/session/display.rs`: una estructura con los 11 campos de tamaño y
   escala, y las funciones puras (`desktop_size`, `picture_rect`,
   `remote_point`, `fits_window`, `screen_region`, `pointer_rect`). Sus tests
   se mudan y dejan de construir la sesión entera.
2. `src/session/signin.rs`: los 12 campos de acceso y llavero y su lógica.
3. `src/session/controller.rs`: fase, sesión del motor, reconexión
   (`worth_retrying`, `reconnect_delay`), y el manejo de eventos del motor.
4. `src/session/screen.rs`: el búfer de imagen y el puntero remoto.
5. `src/session/view/`: formulario de acceso, pregunta de identidad, barras y
   pantalla remota. `remote_screen` (275 líneas) se divide en colocar, pintar y
   recoger la entrada.

`display`, `signin`, `controller` e `input` no usan egui, salvo los tipos de
geometría (`Rect`, `Vec2`, `Pos2`), que en la fase 4 vendrán de `emath`.

**Terminado cuando:** los cuatro comandos pasan; `ActiveSession` tiene unos
diez campos o menos; `grep -n 'egui::' src/session/{display,signin,controller}.rs`
solo muestra tipos de geometría; el script de fronteras comprueba eso.

**Prueba manual:** la lista de regresión completa, al final de este documento.

**Riesgo:** medio. Es mucho movimiento. Lo contiene la regla de commits
separados y que las funciones puras ya tienen tests.

---

## Fase 3. Un solo dueño de los datos del PC

**Objetivo:** que añadir una opción toque un solo sitio del diálogo.

**Pasos**

1. `AddEditPcDialog` edita un borrador `DeviceBookmark` en lugar de 23 campos
   paralelos. Abrir es clonar; guardar es validar y devolver el borrador.
2. `model.rs` se divide en `domain.rs` (tipos y reglas, sin textos) y
   `store.rs` (rutas, lectura, escritura, permisos).
3. Las etiquetas (`display_label`) pasan a la interfaz. `Appearance` deja de
   vivir en `theme`: el tema depende del ajuste, no al revés.
4. Un test nuevo carga una copia del `state.json` actual y comprueba que se lee
   igual. Es la garantía de que el formato guardado no cambia.

**Terminado cuando:** los cuatro comandos pasan y un campo de prueba añadido a
`DeviceBookmark` solo exige tocar el modelo y la línea del diálogo que lo
muestra.

**Prueba manual:** editar un PC, cambiar una opción de cada pestaña, guardar,
cerrar la app, abrirla y comprobar que todo sigue. El PC que ya existe debe
aparecer igual que antes.

**Riesgo:** medio-bajo. El peligro real es el formato guardado; lo cubre el
test del paso 4.

---

## Fase 4. Crate `remoport-core`

**Objetivo:** que el compilador impida que la lógica dependa de la interfaz.

**Pasos**

1. Crear `remoport-core` en el workspace. Dependencias: `remoport-rdp`, serde,
   serde_json, chrono, uuid, zeroize, emath. Nada de egui, eframe, cpal,
   wayland ni keyring.
2. Mover allí `domain`, `store`, `logging` y `session/{controller, display,
   input, signin}`.
3. Definir el puerto `SecretStore` en el núcleo. `credentials.rs` pasa a
   `src/platform/keyring.rs` y lo implementa.
4. El núcleo deja de producir texto para el usuario. Donde hoy guarda una
   cadena traducida (el motivo de fin de sesión, el error de acceso) guarda un
   tipo, y la vista lo traduce. `i18n` se queda en el binario.
5. Reordenar el binario en `src/ui/` y `src/platform/` (`audio`,
   `wayland_keys`, `keyring`). `platform/wayland_keys` recibe los manejadores
   de ventana desde `main.rs`, no un `eframe::Frame`.
6. Ampliar `scripts/check-boundaries.sh` con las reglas R2 a R6.

**Terminado cuando:** los cuatro comandos pasan y el script comprueba R1 a R6.

**Prueba manual:** la lista de regresión completa.

**Riesgo:** medio. El paso 4 no es un simple movimiento: cambia cadenas por
tipos. Va en commits propios.

---

## Fase 5. Motor por etapas

**Objetivo:** que añadir un canal (RAIL) no alargue una función de 220 líneas.

**Pasos**

1. Agrupar `ConnectConfig`: destino, credenciales, pantalla, lo compartido,
   seguridad. El constructor de la fase 0 absorbe el cambio.
2. Dividir `connect()` en etapas con nombre: abrir transporte, asegurar e
   identificar, firmar, registrar canales.
3. Carpeta `channels/` para `clipboard`, `sound`, `microphone`, `drive`, `gfx`
   y el control de pantalla. Cada canal ofrece su propio registro.
4. `session.rs` del motor en `session/{run, commands, output}.rs`.
5. `Event::Progress` pasa a llamarse `Event::Log`, que es lo que es.

**Terminado cuando:** los cuatro comandos pasan, y `connect()` (hoy 220 líneas)
y `run()` (hoy 155) quedan por debajo de 120. `drive.rs` no se toca en esta
fase.

**Prueba manual:** la lista de regresión completa. Esta fase toca la conexión y
la seguridad: identidad, NLA y puerta de enlace se prueban con cuidado.

**Riesgo:** medio. El orden de los pasos de conexión es de seguridad: la
identidad se comprueba antes de enviar ninguna credencial, y eso no puede
cambiar. `tests/identity_flow.rs` lo vigila.

---

## Fase 6. `app.rs` en páginas

**Objetivo:** una página por archivo.

**Pasos:** `ui/shell.rs` (barra lateral, cabecera, navegación),
`ui/pages/{favorites, devices, apps, settings}.rs`, `ui/widgets.rs` (tarjetas,
chips, botones). Solo movimiento.

**Terminado cuando:** los cuatro comandos pasan y ningún archivo de `ui/pages/`
conoce a otro.

**Prueba manual:** recorrer las cuatro páginas, buscar, filtrar, cambiar de
vista y de tema, añadir, editar y borrar un PC.

**Riesgo:** bajo.

---

## Lista de regresión completa

La hace el dueño contra el PC real cuando una fase la pide.

1. Conectar con la contraseña del llavero. El log dice identidad recordada.
2. Imagen: `Graphics: GFX + AVC444`.
3. Cambiar el tamaño de la ventana; entrar y salir de pantalla completa.
4. Sonido de un video; grabar con el micrófono.
5. Copiar del PC y pegar aquí; copiar aquí y pegar en el PC.
6. Abrir la carpeta compartida; en solo lectura, crear un archivo debe fallar.
7. Mayúsculas, `ñ`, tecla Windows y Alt+Tab en pantalla completa.
8. Puntero del PC visible y con su forma.
9. Desconectar: la app sale de pantalla completa y vuelve a la lista.

Sin probar todavía, de antes de este plan: carpeta con "Permitir cambios", dos
pantallas, y cambio de escala con la sesión abierta.
